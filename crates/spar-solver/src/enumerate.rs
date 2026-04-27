//! Overlay-aware multi-objective candidate ranking (Track E commit 5/8).
//!
//! Extends the v0.7.x [`crate::milp`] surface with a *ranking* mode used
//! by `spar moves enumerate` to score hypothetical bindings. Where
//! [`crate::milp::solve_milp`] picks one optimal binding, this module
//! takes a [`BindingOverlay`] that describes a *single* candidate move
//! and produces a [`CandidateRank`] whose components capture the same
//! axes the verify-pipeline already exposes:
//!
//! - **Max response time** across threads bound (under the overlay) to
//!   any candidate processor — derived from the existing RTA pass via
//!   message parsing, mirroring the slack-extraction loop in commit 4.
//! - **Total CPU load** as the worst-case sum of `wcet/period` over
//!   threads visible under the overlay.
//! - **Total power** read from `Spar_Power::Power_Budget` (and the
//!   compatible `SEI::PowerBudget` / `Physical_Properties::Power_Budget`
//!   keys upstream of `weight_power`).
//! - **Total weight** read from `Weight_Properties::Weight` and the
//!   compatible upstream keys.
//!
//! The aggregate `score` is a normalised, weighted sum (lower = better)
//! computed from the boolean flags in [`EnumerationObjective`]. Each
//! enabled objective contributes `1/k` where `k` is the count of
//! enabled objectives; an empty objective set yields `score = 0` which
//! lets callers pass `Default::default()` as a "no preference" probe.
//!
//! # Why not invoke MILP here?
//!
//! The v0.8.0 commit-5 promise is *consistent multi-objective ranking*
//! using the same machinery the verify-pipeline runs — not full
//! MILP-driven enumeration. Running MILP per candidate would double
//! solver work for the candidate-loop case and add a HiGHS dependency
//! on the rank path. We keep the existing `solve_milp` for fresh
//! allocation and reuse RTA + property-accessor lookups for ranking.
//! Full MILP-driven enumeration lands in commit 6/8 if the empirical
//! cost/benefit makes the case.

use serde::Serialize;

use spar_analysis::{AnalysisDiagnostic, AnalysisRunner, Severity};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, PropertyExpr};
use spar_hir_def::overlay::{BindingOverlay, actual_processor_binding_with_overlay};
use spar_hir_def::properties::PropertyMap;

/// Multi-objective rank specification for a hypothetical binding.
///
/// All four fields are independent boolean flags. Setting more than one
/// produces a *balanced* score — each enabled objective contributes
/// `1/k` of the total, where `k` is the number of enabled flags.
///
/// `Default` returns all-false; the resulting score is always zero,
/// which makes `EnumerationObjective::default()` a reasonable
/// "no preference" probe in tests that want to exercise the
/// rank-extraction plumbing without picking a metric.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EnumerationObjective {
    /// Score by maximum response time across overlay-bound threads.
    /// Lower = better; deadline misses produce a negative
    /// `max_response_ns` and a large positive score contribution.
    pub minimize_max_response: bool,
    /// Score by aggregate CPU utilization (sum of `wcet/period`
    /// across bound threads).
    pub minimize_total_load: bool,
    /// Score by total power consumption read from
    /// `Spar_Power::Power_Budget` (with upstream fallbacks via
    /// `SEI::PowerBudget` / `Physical_Properties::Power_Budget`).
    pub minimize_total_power: bool,
    /// Score by total weight read from `Weight_Properties::Weight`
    /// (with upstream `SEI::GrossWeight` / `Weight` fallbacks).
    pub minimize_total_weight: bool,
}

impl EnumerationObjective {
    /// All four objectives enabled, weighted equally.
    pub fn balanced() -> Self {
        Self {
            minimize_max_response: true,
            minimize_total_load: true,
            minimize_total_power: true,
            minimize_total_weight: true,
        }
    }

    /// Single-objective: maximum response time only.
    pub fn max_response() -> Self {
        Self {
            minimize_max_response: true,
            ..Self::default()
        }
    }

    /// Single-objective: total CPU utilization only.
    pub fn total_load() -> Self {
        Self {
            minimize_total_load: true,
            ..Self::default()
        }
    }

    /// Single-objective: total power consumption only.
    pub fn total_power() -> Self {
        Self {
            minimize_total_power: true,
            ..Self::default()
        }
    }

    /// Single-objective: total weight only.
    pub fn total_weight() -> Self {
        Self {
            minimize_total_weight: true,
            ..Self::default()
        }
    }

    /// Number of enabled flags (1..=4 for the named modes, 0 for `Default`).
    pub fn enabled_count(&self) -> u32 {
        u32::from(self.minimize_max_response)
            + u32::from(self.minimize_total_load)
            + u32::from(self.minimize_total_power)
            + u32::from(self.minimize_total_weight)
    }
}

/// Multi-objective rank produced for a candidate hypothetical binding.
///
/// `score` is the aggregate (lower = better) computed from the per-axis
/// values and the enabled flags in [`EnumerationObjective`]. The raw
/// per-axis values stay accessible so callers can render a verbose
/// breakdown, sort by a different axis post-hoc, or feed the values
/// into the JSON shape consumed by the v0.9.0 MCP tool surface.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CandidateRank {
    /// Worst-case response time observed across overlay-bound threads,
    /// in picoseconds. `None` when RTA produced no usable Info-level
    /// "response time" diagnostic for any thread under the overlay.
    /// `Some(negative)` is the deadline-miss sentinel — cleared to a
    /// large positive contribution by the score aggregator.
    pub max_response_ns: Option<i64>,
    /// Sum of `wcet_ps / period_ps` across overlay-bound threads.
    /// Range `0.0..=N` (N = thread count); typical "healthy" models
    /// stay under the processor-count.
    pub total_load: f64,
    /// Sum of per-component `Spar_Power::Power_Budget` (or fallback
    /// keys) in milliwatts. `None` when no power property is set on
    /// any of the overlay-visited components.
    pub total_power_mw: Option<u64>,
    /// Sum of per-component `Weight_Properties::Weight` (or fallback
    /// keys) in grams. `None` when no weight property is set on any
    /// of the overlay-visited components.
    pub total_weight_g: Option<u64>,
    /// Aggregate score (lower = better). Zero when no objective flag
    /// is enabled or when the model carries no rankable data.
    pub score: f64,
}

/// Rank a hypothetical binding under an overlay against a multi-objective
/// specification.
///
/// Walks every thread in the instance, computes the effective binding
/// via [`actual_processor_binding_with_overlay`], and aggregates:
///
/// - response times via the existing RTA pass (one full pass per call);
/// - utilization from declared `Period` / `Compute_Execution_Time`;
/// - power from `Spar_Power::Power_Budget` (or compatible fallbacks);
/// - weight from `Weight_Properties::Weight` (or compatible fallbacks).
///
/// The score is a sum of normalised per-axis contributions, each
/// scaled by `1/k` where `k` is [`EnumerationObjective::enabled_count`].
/// Per-axis normalisation is by-design conservative (the goal is
/// stable ordering across models, not absolute magnitudes):
///
/// - `max_response_ns`: divided by 1 ms (the conventional deadline
///   floor in the analysis suite); deadline misses contribute 10.0.
/// - `total_load`: passed through unchanged (already normalised in
///   `[0.0, 1.0]`-per-CPU range).
/// - `total_power_mw`: divided by 1000 (mW → W).
/// - `total_weight_g`: divided by 1000 (g → kg).
///
/// Ties on score sort downstream by FQN, mirroring commit 4's existing
/// candidate-sort behaviour.
///
/// # Determinism
///
/// The function is deterministic for a fixed (`instance`, `overlay`,
/// `objective`) triple — no randomness, no time-dependent inputs, no
/// hash-iteration-order dependence.
pub fn rank_candidate(
    instance: &SystemInstance,
    overlay: &BindingOverlay,
    objective: &EnumerationObjective,
) -> CandidateRank {
    // Run the analysis suite once per call. We need RTA's diagnostic
    // stream to derive the max-response and we want a single,
    // well-defined production path so the rank output stays in lock-step
    // with what `spar moves verify` reports.
    let analysis_diags = run_all_analyses(instance);

    // Identify the candidate target processor — the value the overlay
    // adds, or (degenerate case) None. When the overlay carries
    // multiple moves, we use the union of all moved-to targets when
    // scanning RTA messages so multi-component plans rank correctly.
    let target_names: Vec<String> = overlay
        .moves
        .values()
        .map(|&idx| instance.component(idx).name.as_str().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let max_response_ns = compute_max_response_ns(&analysis_diags, &target_names);

    // Overlay-aware utilisation aggregate. We sum over every thread
    // whose effective binding (under the overlay) is one of the
    // candidate targets.
    let total_load = compute_total_load(instance, overlay, &target_names);

    // Power & weight aggregate over overlay-visited components.
    let total_power_mw = compute_total_power_mw(instance, overlay);
    let total_weight_g = compute_total_weight_g(instance, overlay);

    let score = aggregate_score(
        objective,
        max_response_ns,
        total_load,
        total_power_mw,
        total_weight_g,
    );

    CandidateRank {
        max_response_ns,
        total_load,
        total_power_mw,
        total_weight_g,
        score,
    }
}

/// Run every registered analysis pass against the instance.
///
/// Mirrors the helper in `spar-cli::moves` so callers in either crate
/// surface the same diagnostic stream.
fn run_all_analyses(instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
    let mut runner = AnalysisRunner::new();
    runner.register_all();
    runner.run_all(instance)
}

/// Compute `max(response_time)` in picoseconds across threads on a set
/// of candidate processor names.
///
/// Walks the analysis-diagnostic stream looking for RTA messages of the
/// form `"thread '...' on processor '<target>': response time <X> <=
/// deadline <Y>"` (Info, parsed for the response-time value) or
/// `"thread '...' on processor '<target>' misses deadline"` (Error,
/// returns the negative-sentinel). Returns:
///
/// - `Some(picoseconds)` (positive) — the max response across matched
///   threads.
/// - `Some(-1)` — a deadline miss was found on any matched thread.
/// - `None` — no RTA diagnostic mentions any candidate target (e.g.,
///   the model has no RTA-visible threads).
fn compute_max_response_ns(diags: &[AnalysisDiagnostic], target_names: &[String]) -> Option<i64> {
    if target_names.is_empty() {
        return None;
    }
    let mut max_response: Option<i64> = None;
    let mut had_miss = false;
    let mut had_match = false;

    for d in diags {
        if d.analysis != "rta" && d.analysis != "RtaAnalysis" {
            continue;
        }
        let msg_lower = d.message.to_ascii_lowercase();
        let mentions_target = target_names
            .iter()
            .any(|t| msg_lower.contains(&format!("on processor '{t}'")));
        if !mentions_target {
            continue;
        }
        had_match = true;

        if d.severity == Severity::Error && msg_lower.contains("misses deadline") {
            had_miss = true;
            continue;
        }

        if let Some((rt_ps, _dl_ps)) = parse_response_deadline(&d.message) {
            let rt_signed = rt_ps as i64;
            max_response = Some(max_response.map_or(rt_signed, |cur| cur.max(rt_signed)));
        }
    }

    if had_miss {
        return Some(-1);
    }
    if !had_match {
        return None;
    }
    max_response
}

/// Sum of `wcet/period` for threads whose overlay-effective processor
/// binding resolves to one of the candidate target names.
fn compute_total_load(
    instance: &SystemInstance,
    overlay: &BindingOverlay,
    target_names: &[String],
) -> f64 {
    if target_names.is_empty() {
        return 0.0;
    }
    let mut total = 0.0_f64;
    for (idx, comp) in instance.all_components() {
        if comp.category != ComponentCategory::Thread {
            continue;
        }
        let Some(bound_idx) = actual_processor_binding_with_overlay(instance, idx, Some(overlay))
        else {
            continue;
        };
        let bound_name = instance
            .component(bound_idx)
            .name
            .as_str()
            .to_ascii_lowercase();
        if !target_names.iter().any(|t| t == &bound_name) {
            continue;
        }
        let props = instance.properties_for(idx);
        let period_ps =
            spar_analysis::property_accessors::get_timing_property(props, "Period").unwrap_or(0);
        let wcet_ps = spar_analysis::property_accessors::get_execution_time(props).unwrap_or(0);
        if period_ps == 0 || wcet_ps == 0 {
            continue;
        }
        total += wcet_ps as f64 / period_ps as f64;
    }
    total
}

/// Sum every overlay-visited component's `Power_Budget` in milliwatts.
///
/// Reads the canonical `Spar_Power::Power_Budget` first, falling back
/// to the upstream `SEI::PowerBudget` / `Physical_Properties::Power_Budget`
/// keys for compatibility with models authored before commit 5/8 added
/// the spar-defined property set.
fn compute_total_power_mw(instance: &SystemInstance, overlay: &BindingOverlay) -> Option<u64> {
    let mut total: u64 = 0;
    let mut had_any = false;

    let visited = overlay_visited_components(overlay);
    for idx in visited {
        let props = instance.properties_for(idx);
        if let Some(mw) = read_power_budget_mw(props) {
            total = total.saturating_add(mw);
            had_any = true;
        }
    }
    if had_any { Some(total) } else { None }
}

/// Sum every overlay-visited component's `Weight` in grams.
///
/// Reads `Weight_Properties::Weight` first, falling back to the
/// upstream `SEI::GrossWeight`, `SEI::Weight`, and unqualified `Weight`
/// keys per the v0.7.x weight_power convention.
fn compute_total_weight_g(instance: &SystemInstance, overlay: &BindingOverlay) -> Option<u64> {
    let mut total: u64 = 0;
    let mut had_any = false;

    let visited = overlay_visited_components(overlay);
    for idx in visited {
        let props = instance.properties_for(idx);
        if let Some(g) = read_weight_g(props) {
            total = total.saturating_add(g);
            had_any = true;
        }
    }
    if had_any { Some(total) } else { None }
}

/// Components an overlay touches: each moved component plus each
/// distinct target it points at. Sorted by raw-arena order via the
/// [`ComponentInstanceIdx`] hash so the iteration is deterministic
/// for a fixed overlay.
fn overlay_visited_components(overlay: &BindingOverlay) -> Vec<ComponentInstanceIdx> {
    let mut set: std::collections::BTreeSet<ComponentInstanceIdx> =
        std::collections::BTreeSet::new();
    for (&from, &to) in &overlay.moves {
        set.insert(from);
        set.insert(to);
    }
    set.into_iter().collect()
}

/// Read a component's power budget in milliwatts.
///
/// Prefers `Spar_Power::Power_Budget` (introduced in commit 5/8); falls
/// back to the legacy `SEI::PowerBudget` / `Physical_Properties::*`
/// keys used by the v0.7.x weight_power analysis. Accepts unit suffixes
/// `mW` (default), `W`, `kW`, `uW`.
pub fn read_power_budget_mw(props: &PropertyMap) -> Option<u64> {
    // Typed path: Spar_Power::Power_Budget — registered as Time so the
    // value lowers to picoseconds on the typed path. We accept either
    // a typed numeric (treated as mW directly) or fall through to the
    // raw-string path for the canonical "<n> <unit>" shape.
    if let Some(expr) = props.get_typed("Spar_Power", "Power_Budget")
        && let Some(mw) = numeric_from_expr(expr)
    {
        return Some(mw);
    }
    let raw = props
        .get("Spar_Power", "Power_Budget")
        .or_else(|| props.get("SEI", "PowerBudget"))
        .or_else(|| props.get("Physical_Properties", "PowerBudget"))
        .or_else(|| props.get("Physical_Properties", "Power_Budget"))
        .or_else(|| props.get("", "PowerBudget"))
        .or_else(|| props.get("", "Power_Budget"))?;
    parse_power_value_mw(raw)
}

/// Read a component's weight in grams.
fn read_weight_g(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = props.get_typed("Weight_Properties", "Weight")
        && let Some(g) = numeric_from_expr(expr)
    {
        return Some(g);
    }
    let raw = props
        .get("Weight_Properties", "Weight")
        .or_else(|| props.get("SEI", "GrossWeight"))
        .or_else(|| props.get("SEI", "Weight"))
        .or_else(|| props.get("Physical_Properties", "GrossWeight"))
        .or_else(|| props.get("Physical_Properties", "Weight"))
        .or_else(|| props.get("", "GrossWeight"))
        .or_else(|| props.get("", "Weight"))?;
    parse_weight_value_g(raw)
}

/// Try to extract an integer numeric from a typed property expression.
///
/// Handles the [`PropertyExpr::Integer`] form (with an optional unit
/// name we ignore here — unit reconciliation is the caller's concern)
/// and the [`PropertyExpr::Real`] form (parsed from its stored string
/// representation).
fn numeric_from_expr(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(n, _) => u64::try_from(*n).ok(),
        PropertyExpr::Real(s, _) => {
            let f = s.parse::<f64>().ok()?;
            if !f.is_finite() || f < 0.0 {
                None
            } else {
                Some(f as u64)
            }
        }
        _ => None,
    }
}

/// Parse a power value (mW). Accepts `mW`, `kW`, `uW`, `W` suffixes;
/// bare numbers are interpreted as milliwatts.
///
/// Ordering matters: prefix-disambiguating suffixes (`mW`, `kW`, `uW`)
/// must be checked before bare `W` so `"500 uW"` is not misread as
/// `"500 u" + "W"`.
fn parse_power_value_mw(s: &str) -> Option<u64> {
    let s = s.trim();
    let s = s.trim_start_matches('=').trim();
    for &(suffix, factor) in &[
        ("mW", 1.0_f64),
        ("kW", 1_000_000.0),
        ("uW", 0.001),
        ("W", 1_000.0),
    ] {
        if let Some(num_str) = s.strip_suffix(suffix) {
            let num = num_str.trim().parse::<f64>().ok()?;
            if !num.is_finite() || num < 0.0 {
                return None;
            }
            return Some((num * factor) as u64);
        }
    }
    let num = s.parse::<f64>().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some(num as u64)
}

/// Parse a weight value (grams). Accepts `kg`, `g`, `mg`, `lb` suffixes;
/// bare numbers are interpreted as kilograms (matching the legacy
/// `weight_power.rs` convention).
fn parse_weight_value_g(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(suffix, factor_g) in &[
        ("kg", 1_000.0_f64),
        ("mg", 0.001),
        ("g", 1.0),
        ("lb", 453.592),
    ] {
        if let Some(num_str) = s.strip_suffix(suffix) {
            let num = num_str.trim().parse::<f64>().ok()?;
            if !num.is_finite() || num < 0.0 {
                return None;
            }
            return Some((num * factor_g) as u64);
        }
    }
    // Bare number: interpret as kilograms, matching the existing
    // `parse_weight_value` convention in `spar-analysis::weight_power`.
    let num = s.parse::<f64>().ok()?;
    if !num.is_finite() || num < 0.0 {
        return None;
    }
    Some((num * 1_000.0) as u64)
}

/// Parse `"response time <X> <= deadline <Y>"` from an RTA Info-level
/// diagnostic. Returns `(rt_ps, dl_ps)` or `None`.
fn parse_response_deadline(msg: &str) -> Option<(u64, u64)> {
    let after_rt = msg.find("response time ")?;
    let rest = &msg[after_rt + "response time ".len()..];
    let le_pos = rest.find("<=")?;
    let rt_str = rest[..le_pos].trim();
    let after_dl = rest[le_pos + 2..].find("deadline ")?;
    let dl_rest = &rest[le_pos + 2 + after_dl + "deadline ".len()..];
    let dl_str = dl_rest.trim_end_matches(['.', ',']).trim();
    Some((parse_format_time_ps(rt_str)?, parse_format_time_ps(dl_str)?))
}

/// Inverse of `spar-analysis::rta::format_time`: parse `"<n> <unit>"`
/// back into picoseconds. Recognises `ps`, `us`, `ms`, `sec` / `s`.
fn parse_format_time_ps(s: &str) -> Option<u64> {
    let s = s.trim();
    let space = s.rfind(char::is_whitespace)?;
    let num_str = s[..space].trim();
    let unit = s[space..].trim();
    let scale: u64 = match unit {
        "ps" => 1,
        "us" => 1_000_000,
        "ms" => 1_000_000_000,
        "sec" | "s" => 1_000_000_000_000,
        _ => return None,
    };
    if let Ok(n) = num_str.parse::<u64>() {
        return Some(n.saturating_mul(scale));
    }
    let f = num_str.parse::<f64>().ok()?;
    if !f.is_finite() || f.is_sign_negative() {
        return None;
    }
    Some((f * scale as f64) as u64)
}

/// Aggregate the per-axis values into a single score under the
/// objective spec. Lower = better. See the [`rank_candidate`] doc
/// comment for normalisation rationale.
fn aggregate_score(
    objective: &EnumerationObjective,
    max_response_ns: Option<i64>,
    total_load: f64,
    total_power_mw: Option<u64>,
    total_weight_g: Option<u64>,
) -> f64 {
    let k = objective.enabled_count();
    if k == 0 {
        return 0.0;
    }
    let weight = 1.0 / f64::from(k);

    let mut score = 0.0_f64;

    if objective.minimize_max_response {
        let contribution = match max_response_ns {
            None => 0.0,
            Some(n) if n < 0 => 10.0, // deadline-miss sentinel
            Some(n) => (n as f64) / 1_000_000_000.0, // ns ÷ 1 ms (in ps)
        };
        score += weight * contribution;
    }
    if objective.minimize_total_load {
        score += weight * total_load;
    }
    if objective.minimize_total_power {
        let contribution = total_power_mw.map_or(0.0, |mw| (mw as f64) / 1_000.0);
        score += weight * contribution;
    }
    if objective.minimize_total_weight {
        let contribution = total_weight_g.map_or(0.0, |g| (g as f64) / 1_000.0);
        score += weight * contribution;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::ComponentInstance;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    fn empty_props() -> PropertyMap {
        PropertyMap::new()
    }

    fn props_with_string(set: &str, name: &str, value: &str) -> PropertyMap {
        let mut p = PropertyMap::new();
        p.add(PropertyValue {
            name: PropertyRef {
                property_set: if set.is_empty() {
                    None
                } else {
                    Some(Name::new(set))
                },
                property_name: Name::new(name),
            },
            value: value.to_string(),
            typed_expr: None,
            is_append: false,
        });
        p
    }

    fn make_instance_with_components(
        names: &[(&str, ComponentCategory)],
    ) -> (SystemInstance, Vec<ComponentInstanceIdx>) {
        let mut components: Arena<ComponentInstance> = Arena::default();
        let root = components.alloc(ComponentInstance {
            name: Name::new("root"),
            category: ComponentCategory::System,
            type_name: Name::new("Root"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
            parent: None,
            children: Vec::new(),
            features: Vec::new(),
            connections: Vec::new(),
            flows: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            array_index: None,
            in_modes: Vec::new(),
        });
        let mut child_idx = Vec::new();
        for (n, c) in names {
            let idx = components.alloc(ComponentInstance {
                name: Name::new(n),
                category: *c,
                type_name: Name::new("T"),
                impl_name: None,
                package: Name::new("Pkg"),
                parent: Some(root),
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: None,
                in_modes: Vec::new(),
            });
            child_idx.push(idx);
        }
        components[root].children = child_idx.clone();
        let instance = SystemInstance {
            root,
            components,
            features: Arena::default(),
            connections: Arena::default(),
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        };
        let mut all = vec![root];
        all.extend(child_idx);
        (instance, all)
    }

    #[test]
    fn objective_default_score_is_zero() {
        // No flags enabled → score must be 0 regardless of input.
        let s = aggregate_score(
            &EnumerationObjective::default(),
            Some(1_000_000),
            0.5,
            Some(500),
            Some(1000),
        );
        assert_eq!(s, 0.0);
    }

    #[test]
    fn objective_balanced_aggregates_all_four() {
        // Balanced objective: weight = 0.25; result mixes all four axes.
        let s = aggregate_score(
            &EnumerationObjective::balanced(),
            Some(1_000_000_000), // 1 ms in ps → contribution 1.0
            0.4,                 // load 0.4
            Some(2_000),         // 2000 mW → 2.0 W
            Some(1_500),         // 1500 g → 1.5 kg
        );
        // 0.25*(1.0 + 0.4 + 2.0 + 1.5) = 0.25 * 4.9 = 1.225
        assert!((s - 1.225).abs() < 1e-9, "score = {s}");
    }

    #[test]
    fn objective_max_response_deadline_miss_inflates_score() {
        let s = aggregate_score(
            &EnumerationObjective::max_response(),
            Some(-1), // deadline miss sentinel → 10.0 contribution
            0.0,
            None,
            None,
        );
        assert_eq!(s, 10.0);
    }

    #[test]
    fn enabled_count_reflects_flags() {
        assert_eq!(EnumerationObjective::default().enabled_count(), 0);
        assert_eq!(EnumerationObjective::max_response().enabled_count(), 1);
        assert_eq!(EnumerationObjective::balanced().enabled_count(), 4);
    }

    #[test]
    fn parse_power_value_mw_handles_units() {
        assert_eq!(parse_power_value_mw("100 mW"), Some(100));
        assert_eq!(parse_power_value_mw("2 W"), Some(2_000));
        assert_eq!(parse_power_value_mw("1.5 kW"), Some(1_500_000));
        assert_eq!(parse_power_value_mw("500 uW"), Some(0));
        assert_eq!(parse_power_value_mw("250"), Some(250)); // bare = mW
        assert_eq!(parse_power_value_mw("not-a-number"), None);
        assert_eq!(parse_power_value_mw("-5 W"), None);
    }

    #[test]
    fn parse_weight_value_g_handles_units() {
        assert_eq!(parse_weight_value_g("100 g"), Some(100));
        assert_eq!(parse_weight_value_g("1 kg"), Some(1_000));
        assert_eq!(parse_weight_value_g("500 mg"), Some(0));
        // Bare number: interpret as kg.
        assert_eq!(parse_weight_value_g("2"), Some(2_000));
    }

    #[test]
    fn read_power_budget_uses_spar_power_first() {
        let p = props_with_string("Spar_Power", "Power_Budget", "300 mW");
        assert_eq!(read_power_budget_mw(&p), Some(300));
    }

    #[test]
    fn read_power_budget_falls_back_to_sei() {
        let p = props_with_string("SEI", "PowerBudget", "1 W");
        assert_eq!(read_power_budget_mw(&p), Some(1_000));
    }

    #[test]
    fn read_power_budget_returns_none_when_absent() {
        assert_eq!(read_power_budget_mw(&empty_props()), None);
    }

    #[test]
    fn rank_candidate_empty_overlay_has_score_zero_for_default_objective() {
        // Degenerate case: empty overlay + default (no flags) objective
        // → score zero, no rankable data emitted.
        let (instance, _idxs) = make_instance_with_components(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
        ]);
        let overlay = BindingOverlay::new();
        let rank = rank_candidate(&instance, &overlay, &EnumerationObjective::default());
        assert_eq!(rank.score, 0.0);
        assert_eq!(rank.max_response_ns, None);
        assert_eq!(rank.total_load, 0.0);
        assert_eq!(rank.total_power_mw, None);
        assert_eq!(rank.total_weight_g, None);
    }

    #[test]
    fn rank_candidate_reads_power_from_overlay_visited() {
        // Overlay touches two components, both with Spar_Power::Power_Budget.
        // Total power must be the sum.
        let (mut instance, idxs) = make_instance_with_components(&[
            ("t1", ComponentCategory::Thread),
            ("cpu1", ComponentCategory::Processor),
            ("cpu2", ComponentCategory::Processor),
        ]);
        let (t1, cpu1, cpu2) = (idxs[1], idxs[2], idxs[3]);

        // Set Power_Budget on t1 (50 mW) and cpu2 (1 W).
        instance
            .property_maps
            .entry(t1)
            .or_default()
            .add(PropertyValue {
                name: PropertyRef {
                    property_set: Some(Name::new("Spar_Power")),
                    property_name: Name::new("Power_Budget"),
                },
                value: "50 mW".to_string(),
                typed_expr: None,
                is_append: false,
            });
        instance
            .property_maps
            .entry(cpu2)
            .or_default()
            .add(PropertyValue {
                name: PropertyRef {
                    property_set: Some(Name::new("Spar_Power")),
                    property_name: Name::new("Power_Budget"),
                },
                value: "1 W".to_string(),
                typed_expr: None,
                is_append: false,
            });
        let _ = cpu1;

        let mut overlay = BindingOverlay::new();
        overlay.add_move(t1, cpu2);

        let rank = rank_candidate(&instance, &overlay, &EnumerationObjective::total_power());
        // 50 mW (t1) + 1000 mW (cpu2) = 1050 mW
        assert_eq!(rank.total_power_mw, Some(1_050));
        // total_power normalised: 1050 / 1000 = 1.05
        assert!((rank.score - 1.05).abs() < 1e-9, "score = {}", rank.score);
    }
}
