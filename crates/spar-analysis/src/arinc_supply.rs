//! ARINC-653 partition-supply-derived inner response-time analysis
//! (REQ-RTA-ARINC-SUPPLY-001, issue #331).
//!
//! Threads bound *inside* an ARINC-653 partition do not run against
//! wall-clock — they run against the partition's **supply**. A periodic
//! resource `Γ = (Π, Θ)` (period `Π`, budget `Θ`, parsed today for the
//! `ARINC-WINDOW-UTILIZATION` gate) delivers, in the worst case, no CPU
//! at all for a contiguous *blackout* of `2(Π−Θ)` before the budget
//! following it. This pass turns that supply model into a per-inner-task
//! worst-case response-time bound, reusing the Lean-verified jittered +
//! blocking RTA recurrence unchanged.
//!
//! ## The two paths (see [`analyze_task`])
//!
//! **(b) Supply-derived release jitter — jitter-encoded path.** Inject
//! `J_i = 2(Π−Θ)` as the release jitter of every inner task and emit the
//! busy-window `w`-form
//!
//! ```text
//! w_i = fixed point of  w = C_i + B_i + Σ_{hp j} ⌈(w + J_j)/T_j⌉·C_j
//! R_i = 2(Π−Θ) + w_i
//! ```
//!
//! This is sound under the **side condition `w_i ≤ Θ`**: the whole busy
//! window fits inside the single contiguous budget that follows the
//! worst-case blackout. We obtain `w_i` and the side-condition test in
//! one call: [`scheduling_verified::compute_response_time_jittered_blocking`]
//! with `deadline = Θ`, `jitter = 0` (pulled out, added back as
//! `2(Π−Θ)`), and each higher-priority task carrying jitter `2(Π−Θ)`
//! returns `Converged(w) ⟺ w ≤ Θ` and `Diverged ⟺ w > Θ` — the divergence
//! bound *is* the side condition.
//!
//! **(c) Non-preemptive blocking.** The cooperative run-to-completion
//! inner scheduler lets one already-dispatched lower-priority poll
//! segment block a release: `B_i = max_{lp} C_j^seg`, carried in the
//! recurrence's existing `blocking` slot (a pure additive constant).
//!
//! **(d) Demand-vs-supply fallback.** When `w_i > Θ` the jitter encoding
//! is no longer guaranteed sound; fall through to the linear supply lower
//! bound
//!
//! ```text
//! lsbf_Γ(t) = ⌊(Θ/Π)·(t − 2(Π−Θ))⌋   for t > 2(Π−Θ),  else 0
//! rbf_i(t)  = C_i + B_i + Σ_{hp j} ⌈(t + J_j^extra)/T_j⌉·C_j
//! R_i       = min { t ∈ (0, D_i] : rbf_i(t) ≤ lsbf_Γ(t) }
//! ```
//!
//! No supply-derived jitter appears in `rbf` — the `sbf` already carries
//! the blackout. If no admissible `t` exists (or the resource is
//! ill-formed) the set is **rejected with a hard Error**, never a
//! silently-wrong number. The v1 fallback is scoped to `J^extra = 0`;
//! the extra-jitter + fallback combination is **declined** with the same
//! hard Error rather than analyzed with the jitter dropped.
//!
//! ## Soundness boundary (carry in docs — the honesty caveats)
//!
//! - The emitted bound is only as sound as its inputs: `C_i`, `C_j^seg`,
//!   and the `Θ` sizing must come from a **sound static WCET bound**, not
//!   measured high-water-marks (cycle-counter observations are not
//!   bounds). Pair with a synth-emitted static cycle bound (synth#778).
//! - An unqualified proof assistant carries **no tool-qualification
//!   credit by itself**: the machine-checked recurrence is a safety-case
//!   input, not a certification claim.
//! - PROVEN (Lean, 0 sorry): recurrence monotonicity + least-fixed-point
//!   convergence *with the constant blocking term*; `lsbf_Γ` monotone and
//!   zero below the blackout; and *conditional* fallback soundness —
//!   given the supply guarantee, `rbf(t) ≤ lsbf_Γ(t)` implies demand ≤
//!   supply at `t`. ASSUMED (the OS's obligation, the theorem's
//!   hypothesis — NOT proven here): that the partition scheduler actually
//!   delivers supply ≥ `lsbf_Γ(t)`, i.e. that `lsbf_Γ` is a sound lower
//!   bound of the real supply. spar does not formalize the periodic-
//!   resource supply-bound function or prove `lsbf_Γ ≤ supply`.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::property_accessors::{
    get_execution_time, get_execution_time_or_exec, get_processor_binding, get_timing_property,
};
use crate::scheduling_verified::{self, RtaResult, ceil_div, interference_jittered};
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

// ── Pure supply-model core (unit-agnostic: feed raw µs or ps) ────────

/// Worst-case contiguous blackout of the periodic resource `Γ = (Π, Θ)`:
/// `2(Π−Θ)`. This is the supply-derived release jitter injected into the
/// recurrence — note it is *twice* the naive `Π−Θ` window gap.
#[inline]
pub fn derived_jitter(period: u64, budget: u64) -> u64 {
    2u64.saturating_mul(period.saturating_sub(budget))
}

/// Linear supply lower bound `lsbf_Γ(t) = ⌊(Θ/Π)·(t − 2(Π−Θ))⌋`, `0`
/// below the blackout. Integer floor (conservative). Requires `Π > 0`.
#[inline]
pub fn lsbf(period: u64, budget: u64, t: u64) -> u64 {
    debug_assert!(period > 0);
    let blackout = derived_jitter(period, budget);
    if t <= blackout {
        return 0;
    }
    // ⌊ Θ·(t − blackout) / Π ⌋
    budget.saturating_mul(t - blackout) / period
}

/// Request-bound function on the fallback path:
/// `rbf_i(t) = C_i + B_i + Σ_{hp j} ⌈(t + J_j^extra)/T_j⌉·C_j`.
///
/// `hp` items are `(period, exec, extra_jitter)`. The supply-derived
/// jitter is deliberately absent here — the supply bound already carries
/// the blackout; reusing it would double-count.
#[inline]
pub fn rbf(exec: u64, blocking: u64, hp: &[(u64, u64, u64)], t: u64) -> u64 {
    let mut total = exec.saturating_add(blocking);
    for &(period, c, extra_j) in hp {
        total = total.saturating_add(interference_jittered(period, c, extra_j, t));
    }
    total
}

/// Outcome of the per-inner-task supply-derived analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyRtaOutcome {
    /// Jitter-encoded path (`w_i ≤ Θ`): `R = 2(Π−Θ) + w_i ≤ D_i`.
    JitterPath { response: u64, busy_window: u64 },
    /// Demand-vs-supply fallback (`w_i > Θ`): `R = min t (rbf ≤ lsbf) ≤ D_i`.
    FallbackPath { response: u64 },
    /// Schedulable check FAILED on the jitter path: `R = 2(Π−Θ) + w_i > D_i`.
    DeadlineMiss { response: u64 },
    /// Fallback found no admissible `t ∈ (0, D_i]` with `rbf ≤ lsbf`.
    Unschedulable,
    /// Ill-formed periodic resource: `Π = 0`, `Θ = 0`, or `Θ > Π`.
    IllFormed,
    /// v1 decline: extra release jitter present *and* the fallback path
    /// was taken. The fallback is scoped to `J^extra = 0`; this
    /// combination is declined rather than analyzed with jitter dropped.
    DeclinedExtraJitter,
}

impl SupplyRtaOutcome {
    /// Whether this outcome is a hard Error (unschedulable / ill-formed /
    /// declined), as opposed to an emitted bound.
    pub fn is_error(&self) -> bool {
        matches!(
            self,
            SupplyRtaOutcome::DeadlineMiss { .. }
                | SupplyRtaOutcome::Unschedulable
                | SupplyRtaOutcome::IllFormed
                | SupplyRtaOutcome::DeclinedExtraJitter
        )
    }
}

/// Analyze one inner task under partition `Γ = (period, budget)`.
///
/// - `exec` = `C_i`, `deadline` = `D_i`, `blocking` = `B_i`.
/// - `own_extra_jitter` = `J_i^extra` (sensor-side etc.); v1 fixtures use 0.
/// - `hp` = higher-priority inner tasks as `(period, exec, extra_jitter)`.
/// - `isr` = periodic ISR interference `(period, exec)` on the same CPU.
///
/// See the module docs for the two-path dispatch. Unit-agnostic: pass µs
/// or ps consistently.
///
/// The explicit-scalar signature (rather than a params struct) keeps the
/// golden + fixture unit tests readable as tabled `(Π,Θ,C,D,B,J,hp)` rows.
#[allow(clippy::too_many_arguments)]
pub fn analyze_task(
    period: u64,
    budget: u64,
    exec: u64,
    deadline: u64,
    blocking: u64,
    own_extra_jitter: u64,
    hp: &[(u64, u64, u64)],
    isr: &[(u64, u64)],
) -> SupplyRtaOutcome {
    // Ill-formed periodic resource — same rejection class as the
    // ARINC-WINDOW-UTILIZATION overcommit gate.
    if period == 0 || budget == 0 || budget > period {
        return SupplyRtaOutcome::IllFormed;
    }

    let blackout = derived_jitter(period, budget);

    // ── (b)+(c): jitter-encoded w-form. Passing `deadline = Θ` makes the
    // recurrence's Converged/Diverged distinction *be* the `w_i ≤ Θ`
    // side condition. Each hp task carries jitter `blackout + its extra`. ─
    let hp_jittered: Vec<(u64, u64, u64)> = hp
        .iter()
        .map(|&(t, c, extra)| (t, c, blackout.saturating_add(extra)))
        .collect();

    match scheduling_verified::compute_response_time_jittered_blocking(
        exec,
        budget, // divergence bound Θ — this encodes the side condition
        0,      // own jitter pulled out; added back as `blackout + own_extra`
        blocking,
        &hp_jittered,
        isr,
    ) {
        RtaResult::Converged(w) => {
            // w ≤ Θ held → jitter encoding sound. R = 2(Π−Θ) + J^extra + w.
            let response = blackout.saturating_add(own_extra_jitter).saturating_add(w);
            if response <= deadline {
                SupplyRtaOutcome::JitterPath {
                    response,
                    busy_window: w,
                }
            } else {
                SupplyRtaOutcome::DeadlineMiss { response }
            }
        }
        RtaResult::Diverged => {
            // w > Θ → jitter encoding no longer guaranteed sound.
            // v1 fallback is scoped to J^extra = 0: decline if any extra
            // release jitter is present rather than drop it (which would
            // make the fallback's demand an under-estimate → unsound).
            let any_extra = own_extra_jitter > 0 || hp.iter().any(|&(_, _, e)| e > 0);
            if any_extra {
                return SupplyRtaOutcome::DeclinedExtraJitter;
            }
            fallback_demand_vs_supply(period, budget, exec, deadline, blocking, hp)
        }
    }
}

/// Fallback (d): `R_i = min { t ∈ (0, D_i] : rbf_i(t) ≤ lsbf_Γ(t) }`.
///
/// `lsbf` is 0 up to the blackout `2(Π−Θ)`, so feasibility requires
/// `D_i > 2(Π−Θ)`. Above the blackout `lsbf` is linear-increasing and
/// `rbf` is a non-decreasing staircase (jumps only at hp release points),
/// so the earliest `t` meeting a demand plateau `V` is closed-form:
/// `t = blackout + ⌈V·Π/Θ⌉`. Re-evaluating `rbf` there and iterating is a
/// monotone fixed point (bounded by `D_i`) that converges in at most one
/// step per hp release point — the same shape as the RTA recurrence.
fn fallback_demand_vs_supply(
    period: u64,
    budget: u64,
    exec: u64,
    deadline: u64,
    blocking: u64,
    hp: &[(u64, u64, u64)],
) -> SupplyRtaOutcome {
    let blackout = derived_jitter(period, budget);
    // lsbf(t) = 0 for t ≤ blackout, and rbf ≥ exec > 0, so no feasible t
    // can lie at or below the blackout.
    if deadline <= blackout {
        return SupplyRtaOutcome::Unschedulable;
    }

    // Start at the blackout edge; climb to meet growing demand.
    let mut t = blackout;
    // Convergence bound: at most (#release points below D_i) + 1 steps;
    // cap defensively at hp.len()+2 iterations (each raises the plateau).
    for _ in 0..=(hp.len() + 2) {
        let demand = rbf(exec, blocking, hp, t);
        // Minimal t' meeting this demand from supply:
        //   ⌊Θ·(t'−blackout)/Π⌋ ≥ demand  ⟺  t' ≥ blackout + ⌈demand·Π/Θ⌉
        let t_next = blackout.saturating_add(ceil_div(demand.saturating_mul(period), budget));
        if t_next > deadline {
            return SupplyRtaOutcome::Unschedulable;
        }
        // Did demand grow across the step? If the plateau at t_next is no
        // higher, supply now meets demand — t_next is the least feasible t.
        if rbf(exec, blocking, hp, t_next) <= lsbf(period, budget, t_next) {
            return SupplyRtaOutcome::FallbackPath { response: t_next };
        }
        if t_next == t {
            // No progress though infeasible — cannot satisfy within D_i.
            return SupplyRtaOutcome::Unschedulable;
        }
        t = t_next;
    }
    SupplyRtaOutcome::Unschedulable
}

// ── AADL analysis pass ───────────────────────────────────────────────

/// ARINC-653 partition-supply-derived inner RTA pass
/// (REQ-RTA-ARINC-SUPPLY-001).
///
/// Fires only for threads discovered by **virtual-processor tree
/// containment** (a VirtualProcessor ancestor) that carry **no**
/// `Actual_Processor_Binding` — the two-level partition shape. Such
/// threads are `__unbound__` to the ordinary [`crate::rta::RtaAnalysis`]
/// (which requires a processor binding) and skipped there, so this pass
/// is their sole analyzer: no contradictory wall-clock number is emitted.
pub struct Arinc653SupplyRtaAnalysis;

/// Collected inner-task timing under one partition.
struct InnerTask {
    comp_idx: ComponentInstanceIdx,
    name: String,
    period: u64,
    exec: u64,
    deadline: u64,
    /// Longest single poll segment `C_j^seg`. For a run-to-completion task
    /// body this is `C_j` (the default); a task that yields across polls
    /// overrides it via `Poll_Segment_Time`. Feeds the non-preemptive
    /// blocking term `B_i = max_{lp} C_j^seg` of *higher*-priority tasks.
    segment: u64,
    extra_jitter: u64,
    priority: Option<u64>,
}

impl Analysis for Arinc653SupplyRtaAnalysis {
    fn name(&self) -> &str {
        "arinc653_supply_rta"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Group inner tasks by their enclosing virtual processor.
        // `ComponentInstanceIdx` is `Ord`, so it keys the map directly
        // (deterministic iteration order for stable diagnostics).
        let mut by_vp: std::collections::BTreeMap<ComponentInstanceIdx, Vec<InnerTask>> =
            std::collections::BTreeMap::new();

        for (idx, comp) in instance.all_components() {
            if comp.category != ComponentCategory::Thread {
                continue;
            }
            let props = instance.properties_for(idx);

            // Two-level partition shape: a VirtualProcessor ancestor AND no
            // explicit processor binding (else RtaAnalysis owns it).
            if get_processor_binding(props).is_some() {
                continue;
            }
            let Some(vp_idx) = owning_partition(instance, idx) else {
                continue;
            };

            let Some(period) = get_timing_property(props, "Period") else {
                continue;
            };
            let Some(exec) = get_execution_time(props) else {
                continue;
            };
            let deadline = get_timing_property(props, "Deadline").unwrap_or(period);
            // Run-to-completion default: the longest poll segment is the
            // whole task body (C_j); a yielding task overrides it.
            let segment = get_timing_property(props, "Poll_Segment_Time").unwrap_or(exec);
            let extra_jitter = get_timing_property(props, "Dispatch_Jitter").unwrap_or(0);
            let priority = crate::rta::get_priority(props);

            by_vp.entry(vp_idx).or_default().push(InnerTask {
                comp_idx: idx,
                name: comp.name.as_str().to_string(),
                period,
                exec,
                deadline,
                segment,
                extra_jitter,
                priority,
            });
        }

        for (_vp_raw, mut tasks) in by_vp {
            // The VP is the owning partition of any task in the group.
            let vp_idx =
                owning_partition(instance, tasks[0].comp_idx).expect("grouped by owning partition");
            let vp_props = instance.properties_for(vp_idx);
            let vp_name = instance.component(vp_idx).name.as_str().to_string();

            let Some(period_pi) = get_timing_property(vp_props, "Period") else {
                continue; // no partition period → not an analyzable Γ
            };
            let Some(budget_theta) = get_execution_time_or_exec(vp_props) else {
                continue; // no partition budget → not an analyzable Γ
            };

            // Priority order: explicit priority (lower = higher), then RM
            // (shorter period first) — same ordering as RtaAnalysis.
            tasks.sort_by(|a, b| match (a.priority, b.priority) {
                (Some(pa), Some(pb)) => pa.cmp(&pb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.period.cmp(&b.period),
            });

            for i in 0..tasks.len() {
                let task = &tasks[i];
                let hp: Vec<(u64, u64, u64)> = tasks[..i]
                    .iter()
                    .map(|t| (t.period, t.exec, t.extra_jitter))
                    .collect();
                // Cooperative non-preemptive blocking: one already-dispatched
                // lower-priority poll segment. B_i = max_{lp} C_j^seg.
                let blocking = tasks[i + 1..].iter().map(|t| t.segment).max().unwrap_or(0);

                let outcome = analyze_task(
                    period_pi,
                    budget_theta,
                    task.exec,
                    task.deadline,
                    blocking,
                    task.extra_jitter,
                    &hp,
                    &[],
                );
                diags.push(diagnostic_for(
                    self.name(),
                    instance,
                    task,
                    &vp_name,
                    period_pi,
                    budget_theta,
                    outcome,
                ));
            }
        }

        diags
    }
}

/// Nearest VirtualProcessor ancestor of `idx` (partition containment).
fn owning_partition(
    instance: &SystemInstance,
    idx: ComponentInstanceIdx,
) -> Option<ComponentInstanceIdx> {
    let mut current = instance.component(idx).parent;
    while let Some(ci) = current {
        if instance.component(ci).category == ComponentCategory::VirtualProcessor {
            return Some(ci);
        }
        current = instance.component(ci).parent;
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn diagnostic_for(
    analysis: &str,
    instance: &SystemInstance,
    task: &InnerTask,
    vp_name: &str,
    period_pi: u64,
    budget_theta: u64,
    outcome: SupplyRtaOutcome,
) -> AnalysisDiagnostic {
    let blackout = derived_jitter(period_pi, budget_theta);
    let path = component_path(instance, task.comp_idx);
    let (severity, message) = match outcome {
        SupplyRtaOutcome::JitterPath {
            response,
            busy_window,
        } => (
            Severity::Info,
            format!(
                "inner thread '{}' under partition '{}' (Π={}, Θ={}): supply-derived \
                 response time {} = 2(Π−Θ)={} + w={} (w≤Θ jitter path) <= deadline {} \
                 (ARINC-SUPPLY-RTA)",
                task.name,
                vp_name,
                period_pi,
                budget_theta,
                response,
                blackout,
                busy_window,
                task.deadline,
            ),
        ),
        SupplyRtaOutcome::FallbackPath { response } => (
            Severity::Info,
            format!(
                "inner thread '{}' under partition '{}' (Π={}, Θ={}): supply-derived \
                 response time {} via demand-vs-supply fallback (w>Θ; rbf≤lsbf) <= \
                 deadline {} (ARINC-SUPPLY-RTA)",
                task.name, vp_name, period_pi, budget_theta, response, task.deadline,
            ),
        ),
        SupplyRtaOutcome::DeadlineMiss { response } => (
            Severity::Error,
            format!(
                "inner thread '{}' under partition '{}' (Π={}, Θ={}) misses deadline: \
                 supply-derived response time {} > deadline {} (ARINC-SUPPLY-RTA)",
                task.name, vp_name, period_pi, budget_theta, response, task.deadline,
            ),
        ),
        SupplyRtaOutcome::Unschedulable => (
            Severity::Error,
            format!(
                "inner thread '{}' under partition '{}' (Π={}, Θ={}) is unschedulable: \
                 no t <= deadline {} satisfies rbf(t) <= lsbf(t) (ARINC-SUPPLY-RTA)",
                task.name, vp_name, period_pi, budget_theta, task.deadline,
            ),
        ),
        SupplyRtaOutcome::IllFormed => (
            Severity::Error,
            format!(
                "partition '{}' enclosing inner thread '{}' is ill-formed: \
                 require 0 < Θ ({}) <= Π ({}) (ARINC-SUPPLY-RTA)",
                vp_name, task.name, budget_theta, period_pi,
            ),
        ),
        SupplyRtaOutcome::DeclinedExtraJitter => (
            Severity::Error,
            format!(
                "inner thread '{}' under partition '{}' (Π={}, Θ={}): declined — busy window \
                 exceeds Θ (fallback path) while explicit Dispatch_Jitter is present; the v1 \
                 supply fallback is scoped to zero extra jitter (ARINC-SUPPLY-RTA)",
                task.name, vp_name, period_pi, budget_theta,
            ),
        ),
    };
    AnalysisDiagnostic {
        severity,
        message,
        path,
        analysis: analysis.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure core: the #331 golden worked example ────────────────────
    //
    // Γ = (Π = 1000 µs, Θ = 300 µs); blackout 2(Π−Θ) = 1400 µs.
    // Three inner tasks (D_i = T_i, priority 1 highest), B = 100/100/0.
    // NUMBERS ARE THE BUSY-WINDOW w-FORM (R = 2(Π−Θ) + w), pinned as such
    // per issue #331's form note — they coincide with the engine R-form on
    // THIS set but differ in general; do not "fix" a future mismatch by
    // switching forms.
    const PI: u64 = 1000;
    const THETA: u64 = 300;

    #[test]
    fn golden_jitter_path_tau1() {
        // τ1: C=20, T=D=5000, B=100, no hp. w = 20+100 = 120 ≤ 300.
        // R = 1400 + 120 = 1520.
        let out = analyze_task(PI, THETA, 20, 5000, 100, 0, &[], &[]);
        assert_eq!(
            out,
            SupplyRtaOutcome::JitterPath {
                response: 1520,
                busy_window: 120,
            }
        );
    }

    #[test]
    fn golden_jitter_path_tau2() {
        // τ2: C=50, T=D=10000, B=100, hp = τ1 (5000,20). Each hp J=1400.
        // w = 50+100+⌈(w+1400)/5000⌉·20 = 170 ≤ 300. R = 1400+170 = 1570.
        let out = analyze_task(PI, THETA, 50, 10000, 100, 0, &[(5000, 20, 0)], &[]);
        assert_eq!(
            out,
            SupplyRtaOutcome::JitterPath {
                response: 1570,
                busy_window: 170,
            }
        );
    }

    #[test]
    fn golden_jitter_path_tau3() {
        // τ3: C=100, T=D=20000, B=0, hp = τ1,τ2. w = 100+20+50 = 170 ≤ 300.
        // R = 1400 + 170 = 1570.
        let out = analyze_task(
            PI,
            THETA,
            100,
            20000,
            0,
            0,
            &[(5000, 20, 0), (10000, 50, 0)],
            &[],
        );
        assert_eq!(
            out,
            SupplyRtaOutcome::JitterPath {
                response: 1570,
                busy_window: 170,
            }
        );
    }

    // ── Fixture 2: w > Θ forces the rbf ≤ lsbf fallback, schedulable ──
    #[test]
    fn fixture2_fallback_schedulable() {
        // C=350 > Θ=300 → w-form diverges → fallback. No hp.
        // rbf(t)=350; min t with ⌊(t−1400)·300/1000⌋ ≥ 350:
        //   t = 1400 + ⌈350·1000/300⌉ = 1400 + 1167 = 2567.
        // lsbf(2567)=⌊0.3·1167⌋=350 ✓; 2567 ≤ D=5000.
        let out = analyze_task(PI, THETA, 350, 5000, 0, 0, &[], &[]);
        assert_eq!(out, SupplyRtaOutcome::FallbackPath { response: 2567 });
    }

    // ── Fixture 3: unschedulable → hard Error ────────────────────────
    #[test]
    fn fixture3_unschedulable_hard_error() {
        // C=400, D=1000 < blackout 1400 ⇒ lsbf(t)=0 ∀ t≤D ⇒ no feasible t.
        let out = analyze_task(PI, THETA, 400, 1000, 0, 0, &[], &[]);
        assert_eq!(out, SupplyRtaOutcome::Unschedulable);
        assert!(out.is_error());
    }

    // ── Fixture 4: extra jitter + fallback → decline (v1 scope) ───────
    #[test]
    fn fixture4_extra_jitter_fallback_declined() {
        // C=350 (w>Θ → fallback) with own extra jitter 200 > 0 → declined.
        let out = analyze_task(PI, THETA, 350, 5000, 0, 200, &[], &[]);
        assert_eq!(out, SupplyRtaOutcome::DeclinedExtraJitter);
        assert!(out.is_error());
    }

    // ── lsbf conservatism regression (issue's τ3 illustration) ───────
    #[test]
    fn lsbf_more_conservative_than_jitter_path() {
        // Demand 170 against lsbf: 170 ≤ ⌊0.3(t−1400)⌋ first at
        // t = 1400 + ⌈170·1000/300⌉ = 1400 + 567 = 1967 — vs the exact
        // jitter-path answer 1570. Documents the linear-sbf conservatism.
        let out = fallback_demand_vs_supply(PI, THETA, 170, 20000, 0, &[]);
        assert_eq!(out, SupplyRtaOutcome::FallbackPath { response: 1967 });
    }

    // ── Ill-formed resources ─────────────────────────────────────────
    #[test]
    fn ill_formed_budget_exceeds_period() {
        assert_eq!(
            analyze_task(300, 1000, 20, 5000, 0, 0, &[], &[]),
            SupplyRtaOutcome::IllFormed
        );
        assert_eq!(
            analyze_task(0, 0, 20, 5000, 0, 0, &[], &[]),
            SupplyRtaOutcome::IllFormed
        );
    }

    // ── Pure-function unit checks ────────────────────────────────────
    #[test]
    fn derived_jitter_is_twice_the_gap() {
        assert_eq!(derived_jitter(1000, 300), 1400);
        assert_eq!(derived_jitter(1000, 1000), 0); // full supply, no blackout
    }

    #[test]
    fn lsbf_zero_below_blackout_then_linear() {
        assert_eq!(lsbf(1000, 300, 1400), 0); // at blackout edge
        assert_eq!(lsbf(1000, 300, 1400 + 1000), 300); // ⌊0.3·1000⌋
        assert_eq!(lsbf(1000, 300, 100), 0); // well below
    }

    #[test]
    fn rbf_sums_ceiling_interference() {
        // exec 50, B 100, one hp (T=5000,C=20,J=0) at t=170: ⌈170/5000⌉·20=20.
        assert_eq!(rbf(50, 100, &[(5000, 20, 0)], 170), 170);
    }

    // ── AADL-level integration: two-level partition model ────────────
    //
    // Builds the #331 golden as an instance (VP-tree containment, no
    // Actual_Processor_Binding on the inner threads) and asserts BOTH
    // that the supply pass emits the 1520/1570/1570 µs bounds AND that the
    // ordinary RtaAnalysis is silent on these threads (no contradictory
    // wall-clock number) — the two-pass reconciliation the design hinges
    // on. All values in picoseconds (the AADL boundary unit).

    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::{
        ComponentInstance, ConnectionInstance, FeatureInstance, SystemInstance,
    };
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    const US: u64 = 1_000_000; // 1 µs in picoseconds

    struct TB {
        components: Arena<ComponentInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    }

    impl TB {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                property_maps: FxHashMap::default(),
            }
        }
        fn add(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(name),
                impl_name: Some(Name::new("impl")),
                package: Name::new("Pkg"),
                parent,
                children: Vec::new(),
                features: Vec::new(),
                connections: Vec::new(),
                flows: Vec::new(),
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                array_index: None,
                in_modes: Vec::new(),
            })
        }
        fn set_children(&mut self, p: ComponentInstanceIdx, c: Vec<ComponentInstanceIdx>) {
            self.components[p].children = c;
        }
        fn prop(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
            self.property_maps
                .entry(comp)
                .or_default()
                .add(PropertyValue {
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
        }
        fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
            SystemInstance {
                root,
                components: self.components,
                features: Arena::<FeatureInstance>::default(),
                connections: Arena::<ConnectionInstance>::default(),
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
                mode_instances: Arena::default(),
                mode_transition_instances: Arena::default(),
                diagnostics: Vec::new(),
                property_maps: self.property_maps,
                feature_property_maps: Default::default(),
                connection_property_maps: Default::default(),
                emv2_models: Default::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    /// The #331 golden two-level partition model as an instance.
    fn golden_instance() -> SystemInstance {
        let mut b = TB::new();
        let root = b.add("root", ComponentCategory::System, None);
        let cpu = b.add("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add("part_a", ComponentCategory::VirtualProcessor, Some(cpu));
        // Γ = (Π = 1000 µs, Θ = 300 µs).
        b.prop(vp, "Timing_Properties", "Period", "1000 us");
        b.prop(vp, "Timing_Properties", "Execution_Time", "300 us");

        let t1 = b.add("t1", ComponentCategory::Thread, Some(vp));
        let t2 = b.add("t2", ComponentCategory::Thread, Some(vp));
        let t3 = b.add("t3", ComponentCategory::Thread, Some(vp));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![t1, t2, t3]);

        // Inner tasks — NO Actual_Processor_Binding (so RtaAnalysis skips
        // them; the supply pass is the sole analyzer). D_i = T_i.
        for (t, prio, c, period) in [
            (t1, "1", "20 us", "5000 us"),
            (t2, "2", "50 us", "10000 us"),
            (t3, "3", "100 us", "20000 us"),
        ] {
            b.prop(t, "Deployment_Properties", "Priority", prio);
            b.prop(t, "Timing_Properties", "Compute_Execution_Time", c);
            b.prop(t, "Timing_Properties", "Period", period);
        }
        b.build(root)
    }

    #[test]
    fn integration_golden_emits_supply_bounds() {
        let inst = golden_instance();
        let diags = Arinc653SupplyRtaAnalysis.analyze(&inst);

        // Exactly three inner-task bounds, all Info (schedulable).
        assert_eq!(diags.len(), 3, "one bound per inner thread: {diags:#?}");
        assert!(diags.iter().all(|d| d.severity == Severity::Info));

        // Response times in ps: 1520 / 1570 / 1570 µs. The blackout 2(Π−Θ)
        // = 1400 µs is named in every message.
        let find = |name: &str| {
            diags
                .iter()
                .find(|d| d.message.contains(&format!("'{name}'")))
                .unwrap_or_else(|| panic!("no diagnostic for {name}: {diags:#?}"))
        };
        assert!(find("t1").message.contains(&format!("{}", 1520 * US)));
        assert!(find("t2").message.contains(&format!("{}", 1570 * US)));
        assert!(find("t3").message.contains(&format!("{}", 1570 * US)));
        // Every bound is the jitter path with blackout 1400 µs.
        assert!(
            diags
                .iter()
                .all(|d| d.message.contains(&format!("2(Π−Θ)={}", 1400 * US)))
        );
    }

    #[test]
    fn integration_no_double_analysis_from_rta() {
        // The critical reconciliation: because the inner threads carry no
        // Actual_Processor_Binding, the ordinary RtaAnalysis must NOT emit
        // a (contradictory, blackout-ignoring) response-time number.
        let inst = golden_instance();
        let rta_diags = crate::rta::RtaAnalysis.analyze(&inst);
        assert!(
            !rta_diags
                .iter()
                .any(|d| d.message.contains("response time")),
            "RtaAnalysis must be silent on partition-bound threads, got: {rta_diags:#?}"
        );
    }

    #[test]
    fn integration_ill_formed_partition_errors() {
        // Θ > Π on the enclosing partition ⇒ hard Error, no bound.
        let mut b = TB::new();
        let root = b.add("root", ComponentCategory::System, None);
        let cpu = b.add("cpu1", ComponentCategory::Processor, Some(root));
        let vp = b.add("bad", ComponentCategory::VirtualProcessor, Some(cpu));
        b.prop(vp, "Timing_Properties", "Period", "300 us");
        b.prop(vp, "Timing_Properties", "Execution_Time", "1000 us"); // Θ > Π
        let t1 = b.add("t1", ComponentCategory::Thread, Some(vp));
        b.set_children(root, vec![cpu]);
        b.set_children(cpu, vec![vp]);
        b.set_children(vp, vec![t1]);
        b.prop(t1, "Timing_Properties", "Compute_Execution_Time", "20 us");
        b.prop(t1, "Timing_Properties", "Period", "5000 us");
        let inst = b.build(root);

        let diags = Arinc653SupplyRtaAnalysis.analyze(&inst);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("ill-formed"));
    }
}
