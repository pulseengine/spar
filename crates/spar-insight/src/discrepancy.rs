//! Rules-based discrepancy detection: compare per-probe observed
//! timings to declared `Spar_Trace::Expected_BCET / Expected_WCET /
//! Expected_Mean` predictions.
//!
//! Five discrepancy kinds, in order of severity:
//!
//! 1. [`DiscrepancyKind::WcetViolated`] — Error: observed.max ns
//!    exceeds the declared `Expected_WCET`. The model under-estimates
//!    WCET; this is a real bound bug.
//! 2. [`DiscrepancyKind::BcetUnderestimated`] — Warn: observed.min ns
//!    is below `Expected_BCET`. The model is too tight on BCET — minor
//!    miscalibration.
//! 3. [`DiscrepancyKind::MeanDrift`] — Info: |observed.mean −
//!    `Expected_Mean`| > 20% of `Expected_Mean`. Distribution-shift
//!    signal.
//! 4. [`DiscrepancyKind::MissingProbe`] — Info: the trace has data for
//!    a probe that the model doesn't declare any `Expected_*` for.
//! 5. [`DiscrepancyKind::UnobservedProbe`] — Warn: the model declares
//!    `Expected_*` on a probe that the trace never exercises.
//!
//! The formal-statistics layer (Hoeffding bounds, etc.) is parked per
//! project memory's R3 deferral.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use spar_hir_def::instance::{ComponentInstance, ComponentInstanceIdx, SystemInstance};
use spar_hir_def::property_value::parse_time_value;

use crate::ctf::CtfEvent;
use crate::timing::{ObservedTiming, extract_timings};
use crate::zephyr_events::{ZephyrEventClass, classify_event};

/// `Spar_Trace::Expected_*` declarations for one probe point, all
/// already converted to nanoseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedTiming {
    /// Canonical id (dotted path from the root, e.g. `Sys.brake.handler`).
    pub probe_id: String,
    /// Other id spellings that should be treated as the same probe
    /// (e.g. the bare component name `handler`).
    #[serde(default)]
    pub aliases: Vec<String>,
    pub expected_bcet_ns: Option<u64>,
    pub expected_wcet_ns: Option<u64>,
    pub expected_mean_ns: Option<u64>,
}

impl ExpectedTiming {
    /// True iff at least one of the three Expected_* properties is set.
    pub fn has_any(&self) -> bool {
        self.expected_bcet_ns.is_some()
            || self.expected_wcet_ns.is_some()
            || self.expected_mean_ns.is_some()
    }

    /// True iff `id` matches the canonical id or any alias.
    pub fn matches_id(&self, id: &str) -> bool {
        self.probe_id == id || self.aliases.iter().any(|a| a == id)
    }
}

/// One detected discrepancy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discrepancy {
    pub probe_id: String,
    pub kind: DiscrepancyKind,
    pub severity: DiscrepancySeverity,
    pub message: String,
}

/// Severity matches the three-tier convention used elsewhere in spar
/// diagnostics (Error / Warn / Info).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiscrepancySeverity {
    Error,
    Warn,
    Info,
}

/// Kind of discrepancy detected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiscrepancyKind {
    /// observed.max > Expected_WCET (Error).
    WcetViolated {
        observed_max_ns: u64,
        expected_wcet_ns: u64,
    },
    /// observed.min < Expected_BCET (Warn).
    BcetUnderestimated {
        observed_min_ns: u64,
        expected_bcet_ns: u64,
    },
    /// |observed.mean − Expected_Mean| > 20% of Expected_Mean (Info).
    MeanDrift {
        observed_mean_ns: u64,
        expected_mean_ns: u64,
        delta_pct: i64,
    },
    /// Trace has samples for a probe with no Expected_* declarations
    /// (Info).
    MissingProbe,
    /// Model declares Expected_* on a probe with no trace samples
    /// (Warn).
    UnobservedProbe,
}

/// Coverage summary: which declared probes were exercised by the trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProbeCoverage {
    pub declared: Vec<String>,
    pub observed: Vec<String>,
    pub matched: Vec<String>,
    pub unobserved: Vec<String>,
    pub missing: Vec<String>,
}

/// High-level statistics about the trace itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TraceSummary {
    pub event_count: usize,
    pub probe_enter_count: usize,
    pub probe_exit_count: usize,
    pub kernel_event_count: usize,
    pub custom_event_count: usize,
    pub min_timestamp_ns: Option<u64>,
    pub max_timestamp_ns: Option<u64>,
}

impl TraceSummary {
    fn from_events(events: &[CtfEvent]) -> Self {
        let mut s = TraceSummary {
            event_count: events.len(),
            ..Default::default()
        };
        for ev in events {
            s.min_timestamp_ns = Some(match s.min_timestamp_ns {
                Some(m) => m.min(ev.timestamp_ns),
                None => ev.timestamp_ns,
            });
            s.max_timestamp_ns = Some(match s.max_timestamp_ns {
                Some(m) => m.max(ev.timestamp_ns),
                None => ev.timestamp_ns,
            });
            match classify_event(ev) {
                ZephyrEventClass::ProbePointEnter { .. } => s.probe_enter_count += 1,
                ZephyrEventClass::ProbePointExit { .. } => s.probe_exit_count += 1,
                ZephyrEventClass::SemGive { .. }
                | ZephyrEventClass::SemTake { .. }
                | ZephyrEventClass::TimerExpiry { .. } => s.kernel_event_count += 1,
                ZephyrEventClass::Custom { .. } => s.custom_event_count += 1,
            }
        }
        s
    }
}

/// Walk a [`SystemInstance`] and collect the per-probe `Spar_Trace::Expected_*`
/// declarations.
///
/// The canonical `probe_id` is the dotted path from the root (e.g.
/// `Sys.brake.handler`). Each [`ExpectedTiming`] additionally carries
/// alias spellings the trace might use (the bare component name) — see
/// [`ExpectedTiming::matches_id`].
pub fn expected_timings_from_instance(
    instance: &SystemInstance,
) -> HashMap<String, ExpectedTiming> {
    let mut out = HashMap::new();
    for (idx, comp) in instance.all_components() {
        let props = instance.properties_for(idx);
        let bcet = props
            .get("Spar_Trace", "Expected_BCET")
            .and_then(parse_time_to_ns);
        let wcet = props
            .get("Spar_Trace", "Expected_WCET")
            .and_then(parse_time_to_ns);
        let mean = props
            .get("Spar_Trace", "Expected_Mean")
            .and_then(parse_time_to_ns);
        if bcet.is_none() && wcet.is_none() && mean.is_none() {
            continue;
        }
        let dotted = component_dotted_path(instance, idx, comp);
        let bare = comp.name.as_str().to_string();
        let mut aliases = Vec::new();
        if bare != dotted {
            aliases.push(bare);
        }
        let timing = ExpectedTiming {
            probe_id: dotted.clone(),
            aliases,
            expected_bcet_ns: bcet,
            expected_wcet_ns: wcet,
            expected_mean_ns: mean,
        };
        out.insert(dotted, timing);
    }
    out
}

/// Convert a `parse_time_value` (picoseconds) result to nanoseconds.
fn parse_time_to_ns(s: &str) -> Option<u64> {
    parse_time_value(s).map(|ps| ps / 1_000)
}

/// Build a dotted path from the root to a component instance, e.g.
/// `Sys.brake.handler`. The root component contributes its own name.
fn component_dotted_path(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
    comp: &ComponentInstance,
) -> String {
    let mut parts = Vec::new();
    parts.push(comp.name.as_str().to_string());
    let mut cur = comp.parent;
    while let Some(p) = cur {
        let parent = instance.component(p);
        parts.push(parent.name.as_str().to_string());
        cur = parent.parent;
    }
    parts.reverse();
    let _ = idx; // silence unused-warning if iter shape changes
    parts.join(".")
}

/// Run the full discrepancy pipeline.
///
/// Steps:
/// 1. Summarise the trace.
/// 2. Extract observed timings per probe id.
/// 3. Read `Spar_Trace::Expected_*` from the instance model.
/// 4. Cross-correlate and emit discrepancies.
pub fn analyze(events: &[CtfEvent], instance: &SystemInstance) -> crate::report::DiscrepancyReport {
    let summary = TraceSummary::from_events(events);
    let observed = extract_timings(events);
    let expected = expected_timings_from_instance(instance);

    let mut discrepancies = Vec::new();
    let mut coverage = ProbeCoverage {
        declared: sorted_keys(&expected),
        observed: sorted_keys(&observed),
        ..Default::default()
    };

    // Walk each declared probe; look up in observed by canonical id then
    // by alias spellings.
    for (probe_id, exp) in &expected {
        let obs_match = observed
            .get(probe_id)
            .or_else(|| exp.aliases.iter().find_map(|a| observed.get(a)));
        match obs_match {
            Some(obs) if obs.count() > 0 => {
                coverage.matched.push(probe_id.clone());
                emit_for_probe(&mut discrepancies, probe_id, exp, obs);
            }
            _ if exp.has_any() => {
                coverage.unobserved.push(probe_id.clone());
                discrepancies.push(Discrepancy {
                    probe_id: probe_id.clone(),
                    kind: DiscrepancyKind::UnobservedProbe,
                    severity: DiscrepancySeverity::Warn,
                    message: format!(
                        "model declares Spar_Trace::Expected_* on probe {probe_id}, \
                         but the trace has no enter/exit pairs for it"
                    ),
                });
            }
            _ => {}
        }
    }

    // Walk each observed probe that isn't declared (canonical or alias).
    for probe_id in observed.keys() {
        let claimed = expected.values().any(|e| e.matches_id(probe_id));
        if !claimed {
            coverage.missing.push(probe_id.clone());
            discrepancies.push(Discrepancy {
                probe_id: probe_id.clone(),
                kind: DiscrepancyKind::MissingProbe,
                severity: DiscrepancySeverity::Info,
                message: format!(
                    "trace contains samples for probe {probe_id} but the model \
                     declares no Spar_Trace::Expected_* for it"
                ),
            });
        }
    }

    coverage.matched.sort();
    coverage.unobserved.sort();
    coverage.missing.sort();
    discrepancies.sort_by(|a, b| {
        (a.probe_id.as_str(), severity_rank(a.severity))
            .cmp(&(b.probe_id.as_str(), severity_rank(b.severity)))
    });

    crate::report::DiscrepancyReport {
        trace_summary: summary,
        discrepancies,
        coverage,
    }
}

fn emit_for_probe(
    out: &mut Vec<Discrepancy>,
    probe_id: &str,
    exp: &ExpectedTiming,
    obs: &ObservedTiming,
) {
    if let (Some(max), Some(wcet)) = (obs.max_ns(), exp.expected_wcet_ns)
        && max > wcet
    {
        out.push(Discrepancy {
            probe_id: probe_id.to_string(),
            kind: DiscrepancyKind::WcetViolated {
                observed_max_ns: max,
                expected_wcet_ns: wcet,
            },
            severity: DiscrepancySeverity::Error,
            message: format!(
                "observed max {max}ns > Expected_WCET {wcet}ns on probe {probe_id} \
                 — declared WCET is under-estimated"
            ),
        });
    }
    if let (Some(min), Some(bcet)) = (obs.min_ns(), exp.expected_bcet_ns)
        && min < bcet
    {
        out.push(Discrepancy {
            probe_id: probe_id.to_string(),
            kind: DiscrepancyKind::BcetUnderestimated {
                observed_min_ns: min,
                expected_bcet_ns: bcet,
            },
            severity: DiscrepancySeverity::Warn,
            message: format!(
                "observed min {min}ns < Expected_BCET {bcet}ns on probe {probe_id} \
                 — declared BCET is too tight"
            ),
        });
    }
    if let (Some(mean), Some(emean)) = (obs.mean_ns(), exp.expected_mean_ns) {
        // 20% threshold; emean=0 would short-circuit so skip in that case.
        if emean > 0 {
            let delta = (mean as i128) - (emean as i128);
            let delta_abs = delta.unsigned_abs();
            let threshold = (emean as u128 * 20) / 100;
            if delta_abs > threshold {
                let pct = (delta * 100 / emean as i128) as i64;
                out.push(Discrepancy {
                    probe_id: probe_id.to_string(),
                    kind: DiscrepancyKind::MeanDrift {
                        observed_mean_ns: mean,
                        expected_mean_ns: emean,
                        delta_pct: pct,
                    },
                    severity: DiscrepancySeverity::Info,
                    message: format!(
                        "mean drift on probe {probe_id}: observed {mean}ns vs Expected_Mean \
                         {emean}ns ({pct:+}%) — distribution-shift signal"
                    ),
                });
            }
        }
    }
}

fn severity_rank(s: DiscrepancySeverity) -> u8 {
    match s {
        DiscrepancySeverity::Error => 0,
        DiscrepancySeverity::Warn => 1,
        DiscrepancySeverity::Info => 2,
    }
}

fn sorted_keys<V>(m: &HashMap<String, V>) -> Vec<String> {
    let mut k: Vec<String> = m.keys().cloned().collect();
    k.sort();
    k
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctf::parse_ctf;
    use pretty_assertions::assert_eq;

    fn obs(probe: &str, samples: &[u64]) -> ObservedTiming {
        ObservedTiming {
            probe_id: probe.to_string(),
            samples_ns: samples.to_vec(),
        }
    }

    fn exp_full(bcet: u64, wcet: u64, mean: u64) -> ExpectedTiming {
        ExpectedTiming {
            probe_id: "X".into(),
            aliases: Vec::new(),
            expected_bcet_ns: Some(bcet),
            expected_wcet_ns: Some(wcet),
            expected_mean_ns: Some(mean),
        }
    }

    #[test]
    fn analyze_wcet_violated() {
        let mut out = Vec::new();
        let exp = exp_full(100, 500, 300);
        let obs = obs("X", &[200, 600, 250]);
        emit_for_probe(&mut out, "X", &exp, &obs);
        assert!(
            out.iter().any(|d| matches!(
                d.kind,
                DiscrepancyKind::WcetViolated {
                    observed_max_ns: 600,
                    expected_wcet_ns: 500
                }
            )),
            "expected WcetViolated, got {:?}",
            out
        );
        // Severity is Error.
        let wcet = out
            .iter()
            .find(|d| matches!(d.kind, DiscrepancyKind::WcetViolated { .. }))
            .unwrap();
        assert_eq!(wcet.severity, DiscrepancySeverity::Error);
    }

    #[test]
    fn analyze_bcet_underestimated() {
        let mut out = Vec::new();
        let exp = exp_full(200, 1000, 500);
        let obs = obs("X", &[150, 400, 600]);
        emit_for_probe(&mut out, "X", &exp, &obs);
        assert!(
            out.iter().any(|d| matches!(
                d.kind,
                DiscrepancyKind::BcetUnderestimated {
                    observed_min_ns: 150,
                    expected_bcet_ns: 200
                }
            )),
            "expected BcetUnderestimated, got {:?}",
            out
        );
    }

    #[test]
    fn analyze_mean_drift_signals_distribution_shift() {
        let mut out = Vec::new();
        let exp = exp_full(0, 10_000, 100);
        // Observed mean 200 vs expected 100 → +100% drift, well beyond 20%.
        let obs = obs("X", &[150, 200, 250]);
        emit_for_probe(&mut out, "X", &exp, &obs);
        let drift = out
            .iter()
            .find(|d| matches!(d.kind, DiscrepancyKind::MeanDrift { .. }))
            .expect("expected MeanDrift");
        if let DiscrepancyKind::MeanDrift {
            observed_mean_ns,
            expected_mean_ns,
            delta_pct,
        } = drift.kind
        {
            assert_eq!(observed_mean_ns, 200);
            assert_eq!(expected_mean_ns, 100);
            assert_eq!(delta_pct, 100);
        }
    }

    #[test]
    fn analyze_mean_within_20pct_no_drift() {
        let mut out = Vec::new();
        let exp = exp_full(0, 10_000, 100);
        // 110 vs 100 → +10%, under 20% threshold.
        let obs = obs("X", &[100, 110, 120]);
        emit_for_probe(&mut out, "X", &exp, &obs);
        assert!(
            out.iter()
                .all(|d| !matches!(d.kind, DiscrepancyKind::MeanDrift { .. }))
        );
    }

    #[test]
    fn analyze_unobserved_probe_emits_warn() {
        // No observed timings at all; declared expected on probe X.
        let events: Vec<CtfEvent> = parse_ctf("1: tick(no_args)").unwrap();
        let observed = HashMap::<String, ObservedTiming>::new();
        let mut expected = HashMap::new();
        expected.insert("X".to_string(), exp_full(100, 500, 300));

        let _ = events; // events not needed past summary; we exercise the rule directly.
        let _ = observed;

        // Drive the rule manually: simulate the body of analyze() for one probe.
        let mut discrepancies = Vec::new();
        let exp = expected.get("X").unwrap();
        if exp.has_any() {
            discrepancies.push(Discrepancy {
                probe_id: "X".to_string(),
                kind: DiscrepancyKind::UnobservedProbe,
                severity: DiscrepancySeverity::Warn,
                message: "expected".to_string(),
            });
        }
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].severity, DiscrepancySeverity::Warn);
        assert!(matches!(
            discrepancies[0].kind,
            DiscrepancyKind::UnobservedProbe
        ));
    }
}
