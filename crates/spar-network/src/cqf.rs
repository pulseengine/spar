//! 802.1Qch **Cyclic Queuing and Forwarding (CQF)** configuration synthesis —
//! the standard two-buffer, single-cycle-time baseline (REQ-TSN-SYNTH-CQF-BASE-001).
//!
//! # Why CQF is the clean next synthesis target
//!
//! Unlike 802.1Qbv gate scheduling (see [`crate::tsn`]), CQF's worst-case
//! end-to-end delay is **structurally decoupled from the per-link load**: a
//! frame received during cycle *c* is forwarded during cycle *c+1*, so for a
//! path of `H` hops at a global cycle time `T` the end-to-end latency is
//! bounded by
//!
//! ```text
//!     (H - 1) · T   ≤   D   ≤   (H + 1) · T
//! ```
//!
//! independent of topology and of how heavily each cycle is loaded — *given*
//! that no cycle is oversubscribed (IETF `draft-eckert-detnet-tcqf-05`,
//! IEEE 802.1Qch). The feasibility test therefore **factors into two small,
//! local checks** with no network-calculus curve composition:
//!
//! 1. **structural delay** — `(H+1)·T ≤ deadline` for every flow, and
//! 2. **per-cycle admission** — on every link the aggregate reservation fits
//!    the cycle budget `csize = T · link_rate` (the TCQF *csize* admission
//!    rule: "a maximum number of bits permitted to go into each cycle").
//!
//! That is precisely why CQF routes *around* the network-wide NC-composition
//! bridge that an MILP frame-scheduler needs to be both necessary and
//! checkable.
//!
//! # The duality oracle, anchored to external ground truth
//!
//! [`synthesize_cqf`] is the inverse of the CQF *checker* ([`cqf_delay_max_ps`]
//! / [`cqf_delay_min_ps`]): synthesis picks the cycle time `T`, the checker
//! re-derives the per-flow delay and per-link budget. To keep the duality from
//! degenerating into mere self-consistency, the checker's delay formula is
//! **pinned to the published worked example** in
//! `draft-eckert-detnet-tcqf-05`:
//!
//! > "if the number of hops is 24, when cycle interval is set to 10us, the
//! > end-to-end latency bound can be around (24+1)*10 = 250 us."
//!
//! See [`tests::delay_bound_matches_published_tcqf_example`]. The synthesizer
//! self-certifies against this same checker before returning.
//!
//! # Scope (baseline)
//!
//! Standard two-buffer CQF, one global cycle time, homogeneous link rate,
//! reservation-based (`csize`) admission. Multi-CQF / TCQF (3–7 tagged
//! buffers, heterogeneous cycle times, per-flow injection-time planning across
//! a hyperperiod) is REQ-TSN-SYNTH-CQF-001, deferred to v0.21.0. No new
//! dependency — pure integer arithmetic.

use core::fmt;
use std::collections::BTreeMap;

/// One reserved CQF flow to be admitted.
///
/// The flow reserves `reserved_bits_per_cycle` in *every* cycle on *every*
/// link of its `path` (the TCQF `csize` contribution); its end-to-end delay is
/// governed only by the hop count (`path.len()`) and the global cycle time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CqfFlow {
    /// Stable identifier, used only for deterministic error reporting.
    pub id: u32,
    /// Bits this flow reserves in each cycle on each link it traverses.
    pub reserved_bits_per_cycle: u64,
    /// End-to-end deadline in picoseconds.
    pub deadline_ps: u64,
    /// Links the flow traverses, by link id. `len()` is the hop count `H`.
    pub path: Vec<u32>,
    /// Per-hop one-way link latency, parallel to `path` (same length). EMPTY
    /// means "every hop is short" — the flow is handled by the hop-count
    /// [`synthesize_cqf`] exactly as before. Populated (length must equal
    /// `path.len()`) it drives the long-link cycle-quantized bound in
    /// [`synthesize_cqf_longlink`] (REQ-TSN-SYNTH-CQF-LONGLINK-001).
    pub link_delays: Vec<LinkDelay>,
}

/// A synthesized standard-CQF configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CqfSchedule {
    /// The chosen global cycle time `T`, in picoseconds (a whole number of ns).
    pub cycle_time_ps: u64,
    /// Cycle budget `csize = T · link_rate`, in bits.
    pub csize_bits: u128,
    /// Per-link aggregate reservation in bits/cycle (≤ `csize_bits` for every
    /// admitted link).
    pub per_link_bits: BTreeMap<u32, u128>,
}

/// Why CQF synthesis could not produce a feasible configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CqfSynthError {
    /// No flows were supplied.
    NoFlows,
    /// A flow had an empty path (zero hops) — no CQF delay is defined.
    EmptyPath { id: u32 },
    /// A flow reserved zero bits, or had a zero deadline.
    DegenerateFlow { id: u32 },
    /// Two flows shared the same `id` — error reporting would be ambiguous.
    DuplicateFlowId { id: u32 },
    /// `link_rate_bps` was zero.
    ZeroLinkRate,
    /// Even a one-nanosecond cycle violates this flow's deadline:
    /// `(hops + 1) · 1 ns > deadline`.
    DeadlineTooTight {
        id: u32,
        hops: u32,
        deadline_ps: u64,
    },
    /// A link's aggregate per-cycle reservation exceeds the cycle budget at the
    /// deadline-limited cycle time. CQF cannot widen the cycle without
    /// breaking the tightest deadline, so the port is oversubscribed.
    Oversubscribed {
        link: u32,
        required_bits: u128,
        csize_bits: u128,
    },
    /// The synthesized configuration failed its own re-check (a synthesis bug;
    /// should be unreachable).
    SelfCheck(&'static str),
    /// Long-link only: dead time `DT` is not strictly below the cycle time
    /// (`DT ≥ T_c`), so the per-hop cycle-advance formula is undefined.
    DeadTimeTooLarge { dead_time_ps: u64, cycle_ps: u64 },
    /// Long-link only: a flow's `link_delays` length does not match its `path`
    /// (hop count) length.
    LinkDelayLenMismatch {
        id: u32,
        path_len: usize,
        delays_len: usize,
    },
    /// Long-link only: the worst hop needs more cyclic buffers than the
    /// hardware cap (the draft's 7-cycle ceiling). Never silently clamped —
    /// undersized buffers would drop frames.
    BufferBudgetExceeded { needed: u32, cap: u32 },
    /// Long-link only: no cycle time in the "cycle dominates links"
    /// (`T_c > max link delay`) regime meets this flow's deadline — the
    /// deadline would require sub-link-delay cycles (the multi-cycle-per-hop
    /// regime), which this sound baseline does not yet synthesize.
    LongLinkDeadlineTooTight { id: u32 },
}

impl fmt::Display for CqfSynthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFlows => write!(f, "no CQF flows supplied"),
            Self::EmptyPath { id } => {
                write!(f, "flow {id} has an empty path (zero hops)")
            }
            Self::DegenerateFlow { id } => {
                write!(f, "flow {id} reserves zero bits or has a zero deadline")
            }
            Self::DuplicateFlowId { id } => write!(f, "duplicate flow id {id}"),
            Self::ZeroLinkRate => write!(f, "link rate must be non-zero"),
            Self::DeadlineTooTight {
                id,
                hops,
                deadline_ps,
            } => write!(
                f,
                "flow {id}: deadline {deadline_ps} ps is below the irreducible \
                 CQF latency ({hops}+1)·1ns for {hops} hops"
            ),
            Self::Oversubscribed {
                link,
                required_bits,
                csize_bits,
            } => write!(
                f,
                "link {link} oversubscribed: {required_bits} bits/cycle required \
                 but cycle budget is {csize_bits} bits"
            ),
            Self::SelfCheck(why) => write!(f, "CQF self-check failed: {why}"),
            Self::DeadTimeTooLarge {
                dead_time_ps,
                cycle_ps,
            } => write!(
                f,
                "dead time {dead_time_ps} ps must be strictly below cycle {cycle_ps} ps"
            ),
            Self::LinkDelayLenMismatch {
                id,
                path_len,
                delays_len,
            } => write!(
                f,
                "flow {id}: {delays_len} link delays for a {path_len}-hop path"
            ),
            Self::BufferBudgetExceeded { needed, cap } => write!(
                f,
                "long-link CQF needs {needed} cyclic buffers but the cap is {cap}"
            ),
            Self::LongLinkDeadlineTooTight { id } => write!(
                f,
                "flow {id}: deadline unmeetable with a cycle longer than every link \
                 (sub-link-delay cycles are not yet synthesized)"
            ),
        }
    }
}

impl core::error::Error for CqfSynthError {}

/// Worst-case end-to-end CQF latency, in picoseconds, for a flow of `hops`
/// hops at cycle time `cycle_ps`.
///
/// `D_max = (H + 1) · T` — IETF `draft-eckert-detnet-tcqf-05`, IEEE 802.1Qch.
#[must_use]
pub fn cqf_delay_max_ps(hops: u32, cycle_ps: u64) -> u128 {
    (u128::from(hops) + 1) * u128::from(cycle_ps)
}

/// Best-case end-to-end CQF latency, in picoseconds: `D_min = (H − 1) · T`.
///
/// Jitter is `D_max − D_min = 2·T`, consistent with the standard "2T jitter"
/// CQF figure.
#[must_use]
pub fn cqf_delay_min_ps(hops: u32, cycle_ps: u64) -> u128 {
    u128::from(hops.saturating_sub(1)) * u128::from(cycle_ps)
}

/// Cycle budget `csize = T · link_rate`, in bits, for a `cycle_ps`-picosecond
/// cycle on a `link_rate_bps` link.
#[must_use]
pub fn cqf_cycle_budget_bits(cycle_ps: u64, link_rate_bps: u64) -> u128 {
    // bits = rate[bit/s] · T[s] = rate · cycle_ps / 1e12. u128 avoids overflow
    // (100 Gbps · 1 ms ≈ 1e20 > u64::MAX).
    u128::from(link_rate_bps) * u128::from(cycle_ps) / 1_000_000_000_000u128
}

// ── Long-link CQF (REQ-TSN-SYNTH-CQF-LONGLINK-001) ────────────────────────
//
// The bounds above are hop-count-only: they assume every link's one-way
// latency is negligible relative to the cycle (a frame sent in cycle c is
// received in time to be forwarded in cycle c+1). On a LONG link that no
// longer holds — the frame can miss the next cycle boundary and be delayed by
// extra whole cycles. The sound generalization (draft-ietf-detnet-tcqf,
// draft-eckert-detnet-tcqf-05, advisor-confirmed) quantizes each hop's link
// latency into an integer CYCLE ADVANCE kᵢ and sums them.
//
// Dead time `DT` (0 ≤ DT < T_c) is the tcqf guard interval at the END of each
// cycle: no scheduled frame is sent in the last DT of a cycle so it is sure to
// arrive before the receiver's next boundary. Because DT sits at the cycle
// end, it makes same-cycle delivery EASIER — it enters kᵢ with a MINUS sign.

/// One link's one-way latency window (picoseconds): propagation + PHY /
/// processing + sync-error margin. Frame serialization is charged on the send
/// side (the `csize` budget), not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkDelay {
    /// Minimum one-way latency (best case), picoseconds.
    pub min_ps: u64,
    /// Maximum one-way latency (worst case), picoseconds.
    pub max_ps: u64,
}

/// Per-hop CQF cycle advance `kᵢ = 2 + ⌊(dᵢ − DT) / T_c⌋`, the number of whole
/// cycles a frame advances crossing a hop of one-way latency `link_delay_ps`,
/// with dead time `dead_time_ps` (`0 ≤ DT < T_c`) and cycle `cycle_ps`.
///
/// Uses floored (Euclidean) division so a `dᵢ < DT` short link yields exactly
/// `kᵢ = 1` (the classic single-cycle CQF hop) and a boundary `dᵢ = DT` is
/// charged pessimistically to `kᵢ = 2`. `kᵢ ≥ 1` is guaranteed by `DT < T_c`.
#[must_use]
pub fn cqf_hop_advance_cycles(link_delay_ps: u64, dead_time_ps: u64, cycle_ps: u64) -> u64 {
    debug_assert!(dead_time_ps < cycle_ps, "dead time must be < cycle");
    let numerator = i128::from(link_delay_ps) - i128::from(dead_time_ps);
    // div_euclid = floor for a positive divisor; numerator may be negative.
    (2 + numerator.div_euclid(i128::from(cycle_ps))) as u64
}

/// Worst-case long-link end-to-end delay `D_max = T_c + T_c·Σᵢ kᵢ(dᵢᵐᵃˣ)`,
/// picoseconds. `link_delays_max_ps` is the per-hop MAX latency in path order
/// (its length is the hop count `H`). The leading `T_c` is the worst-case
/// source-ingress alignment wait.
///
/// Degenerates to the shipped `(H+1)·T_c` when every hop is short
/// (`dᵢᵐᵃˣ < DT` ⇒ `kᵢ = 1`).
#[must_use]
pub fn cqf_delay_max_longlink_ps(
    link_delays_max_ps: &[u64],
    dead_time_ps: u64,
    cycle_ps: u64,
) -> u128 {
    let sum_k: u128 = link_delays_max_ps
        .iter()
        .map(|&d| u128::from(cqf_hop_advance_cycles(d, dead_time_ps, cycle_ps)))
        .sum();
    u128::from(cycle_ps) + u128::from(cycle_ps) * sum_k
}

/// Best-case long-link end-to-end delay
/// `D_min = ℓ_H,min + T_c·Σ_{i<H} kᵢ(dᵢᵐⁱⁿ)`, picoseconds, with the last-hop
/// physical floor `ℓ_H,min = 0` (the last hop is delivered to the end host, not
/// re-buffered, so it is not cycle-quantized).
///
/// Only the FIRST `H−1` (intermediate) hops are summed. This deliberate
/// asymmetry with [`cqf_delay_max_longlink_ps`] is what makes `(H+1)·T_c` and
/// `(H−1)·T_c` emerge together AND guarantees `D_min ≤ D_max` always (the prior
/// spec inverted precisely because it lacked the last-hop compensation).
/// Degenerates to `(H−1)·T_c` when every hop is short.
#[must_use]
pub fn cqf_delay_min_longlink_ps(
    link_delays_min_ps: &[u64],
    dead_time_ps: u64,
    cycle_ps: u64,
) -> u128 {
    let h = link_delays_min_ps.len();
    if h == 0 {
        return 0;
    }
    let sum_k: u128 = link_delays_min_ps[..h - 1]
        .iter()
        .map(|&d| u128::from(cqf_hop_advance_cycles(d, dead_time_ps, cycle_ps)))
        .sum();
    u128::from(cycle_ps) * sum_k // ℓ_H,min = 0
}

/// Cycle admission budget with dead time: `csize = (T_c − DT)·rate`, bits.
///
/// Only the `T_c − DT` transmittable window of each cycle may be filled — a
/// frame admitted into the dead-time tail could fail to drain before the cycle
/// boundary and spill into a later cycle, breaking `D_max`. Collapses to the
/// shipped [`cqf_cycle_budget_bits`] (`T_c·rate`) when `DT = 0`.
#[must_use]
pub fn cqf_cycle_budget_bits_with_dead_time(
    cycle_ps: u64,
    dead_time_ps: u64,
    link_rate_bps: u64,
) -> u128 {
    let window_ps = cycle_ps.saturating_sub(dead_time_ps);
    u128::from(link_rate_bps) * u128::from(window_ps) / 1_000_000_000_000u128
}

/// Per-path cyclic buffer count `B = max(3, maxᵢ((kᵢᵐᵃˣ − kᵢᵐⁱⁿ) + 2))` —
/// the tcqf 3-cycle floor plus one extra cycle per cycle of link-latency
/// jitter. `Ok(B)` when `B ≤ cap`; `Err(B)` when the needed count exceeds the
/// hardware cap (never silently clamped DOWN — undersized buffers drop frames).
///
/// The draft mandates support for 3 and 4 cycles and a 7-cycle ceiling, so
/// `cap` is normally 7.
pub fn cqf_buffer_count(
    link_delays: &[LinkDelay],
    dead_time_ps: u64,
    cycle_ps: u64,
    cap: u32,
) -> Result<u32, u32> {
    let mut needed = 3u32;
    for ld in link_delays {
        let k_max = cqf_hop_advance_cycles(ld.max_ps, dead_time_ps, cycle_ps);
        let k_min = cqf_hop_advance_cycles(ld.min_ps, dead_time_ps, cycle_ps);
        // k_max ≥ k_min (advance is monotone in latency), so the difference is
        // non-negative; +2 for double buffering.
        let b_i = (k_max - k_min) as u32 + 2;
        needed = needed.max(b_i);
    }
    if needed > cap {
        Err(needed)
    } else {
        Ok(needed)
    }
}

/// Synthesize a standard two-buffer CQF configuration for `flows` on links of
/// uniform `link_rate_bps`.
///
/// Picks the largest whole-nanosecond cycle time `T` that meets every flow's
/// structural deadline `(H+1)·T ≤ deadline`, then admits the flows iff every
/// link's aggregate per-cycle reservation fits the budget `csize = T·rate`.
/// The returned schedule is re-checked against the independent CQF checker
/// before return ([`CqfSynthError::SelfCheck`] on any discrepancy).
///
/// # Errors
///
/// Returns a [`CqfSynthError`] for empty/degenerate input, a deadline below the
/// irreducible CQF latency, or an oversubscribed link.
pub fn synthesize_cqf(flows: &[CqfFlow], link_rate_bps: u64) -> Result<CqfSchedule, CqfSynthError> {
    if flows.is_empty() {
        return Err(CqfSynthError::NoFlows);
    }
    if link_rate_bps == 0 {
        return Err(CqfSynthError::ZeroLinkRate);
    }

    let mut seen_ids = BTreeMap::new();
    for flow in flows {
        if seen_ids.insert(flow.id, ()).is_some() {
            return Err(CqfSynthError::DuplicateFlowId { id: flow.id });
        }
        if flow.path.is_empty() {
            return Err(CqfSynthError::EmptyPath { id: flow.id });
        }
        if flow.reserved_bits_per_cycle == 0 || flow.deadline_ps == 0 {
            return Err(CqfSynthError::DegenerateFlow { id: flow.id });
        }
    }

    // 1. Deadline-limited cycle time. Larger T means looser bandwidth but
    //    longer delay, so the largest T meeting *every* deadline maximizes
    //    admission headroom. T_max for a flow = floor(deadline / (H+1)).
    //    Quantize DOWN to a whole nanosecond (physical cycle times; smaller T
    //    only tightens bandwidth, never breaks a deadline). The largest such T
    //    is the global minimum of the per-flow limits, floored to ns.
    let mut cycle_ps = u64::MAX;
    for flow in flows {
        let hops = flow.path.len() as u32;
        let limit_ps = flow.deadline_ps / (u64::from(hops) + 1);
        cycle_ps = cycle_ps.min(limit_ps);
    }
    // Floor to whole nanoseconds.
    cycle_ps = (cycle_ps / 1_000) * 1_000;
    if cycle_ps == 0 {
        // The flow with the smallest deadline/(H+1) is the binding one.
        let tightest = flows
            .iter()
            .min_by_key(|fl| fl.deadline_ps / (u64::from(fl.path.len() as u32) + 1))
            .expect("flows is non-empty");
        return Err(CqfSynthError::DeadlineTooTight {
            id: tightest.id,
            hops: tightest.path.len() as u32,
            deadline_ps: tightest.deadline_ps,
        });
    }

    // 2. Per-cycle admission on every link.
    let csize_bits = cqf_cycle_budget_bits(cycle_ps, link_rate_bps);
    let mut per_link_bits: BTreeMap<u32, u128> = BTreeMap::new();
    for flow in flows {
        for &link in &flow.path {
            *per_link_bits.entry(link).or_insert(0) += u128::from(flow.reserved_bits_per_cycle);
        }
    }
    for (&link, &required) in &per_link_bits {
        if required > csize_bits {
            return Err(CqfSynthError::Oversubscribed {
                link,
                required_bits: required,
                csize_bits,
            });
        }
    }

    let schedule = CqfSchedule {
        cycle_time_ps: cycle_ps,
        csize_bits,
        per_link_bits,
    };

    // 3. Self-certify against the independent checker.
    self_check(&schedule, flows)?;
    Ok(schedule)
}

/// Re-derive every flow's worst-case delay and every link's budget against the
/// independent checker. By construction this always passes; it is a regression
/// guard if the cycle-selection or admission logic is ever changed.
fn self_check(schedule: &CqfSchedule, flows: &[CqfFlow]) -> Result<(), CqfSynthError> {
    for flow in flows {
        let hops = flow.path.len() as u32;
        let d_max = cqf_delay_max_ps(hops, schedule.cycle_time_ps);
        if d_max > u128::from(flow.deadline_ps) {
            return Err(CqfSynthError::SelfCheck("flow delay exceeds deadline"));
        }
    }
    for &required in schedule.per_link_bits.values() {
        if required > schedule.csize_bits {
            return Err(CqfSynthError::SelfCheck("link reservation exceeds csize"));
        }
    }
    Ok(())
}

/// The draft's cyclic-buffer ceiling: "7 or fewer cycles MUST be used".
pub const CQF_DEFAULT_BUFFER_CAP: u32 = 7;

/// A synthesized long-link CQF configuration (REQ-TSN-SYNTH-CQF-LONGLINK-001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CqfLongLinkSchedule {
    /// Chosen global cycle time `T_c`, picoseconds (whole ns, `> DT` and
    /// `>` every link delay).
    pub cycle_time_ps: u64,
    /// Dead time `DT` (`0 ≤ DT < T_c`), picoseconds.
    pub dead_time_ps: u64,
    /// Cycle budget `csize = (T_c − DT)·rate`, bits.
    pub csize_bits: u128,
    /// Per-link aggregate reservation, bits/cycle (`≤ csize_bits`).
    pub per_link_bits: BTreeMap<u32, u128>,
    /// Cyclic buffers required (`3..=cap`) — from the worst hop's latency
    /// jitter.
    pub buffers: u32,
}

/// Synthesize a long-link-sound CQF configuration, accounting for each hop's
/// one-way link latency via the cycle-quantized bound
/// ([`cqf_delay_max_longlink_ps`]) and the dead-time budget
/// ([`cqf_cycle_budget_bits_with_dead_time`]).
///
/// Baseline scope — the **"cycle dominates links"** regime: it selects the
/// largest whole-nanosecond `T_c` that is longer than every link delay (so each
/// hop advances one cycle if `dᵢ < DT`, else two) and meets every flow's
/// deadline `D_max ≤ deadline`, then admits flows against
/// `csize = (T_c − DT)·rate` and sizes the cyclic buffers. When a deadline can
/// only be met with a cycle *shorter* than some link (the multi-cycle-per-hop
/// regime), it returns [`CqfSynthError::LongLinkDeadlineTooTight`] rather than
/// an unsound configuration; optimal sub-link-cycle selection is a follow-up.
///
/// A flow with empty `link_delays` is treated as all-short (every `dᵢ = 0`);
/// with `DT > 0` that reproduces the hop-count [`synthesize_cqf`] result. The
/// returned schedule self-certifies against the exact long-link bound before
/// return.
pub fn synthesize_cqf_longlink(
    flows: &[CqfFlow],
    link_rate_bps: u64,
    dead_time_ps: u64,
    buffer_cap: u32,
) -> Result<CqfLongLinkSchedule, CqfSynthError> {
    if flows.is_empty() {
        return Err(CqfSynthError::NoFlows);
    }
    if link_rate_bps == 0 {
        return Err(CqfSynthError::ZeroLinkRate);
    }

    let mut seen_ids = BTreeMap::new();
    for flow in flows {
        if seen_ids.insert(flow.id, ()).is_some() {
            return Err(CqfSynthError::DuplicateFlowId { id: flow.id });
        }
        if flow.path.is_empty() {
            return Err(CqfSynthError::EmptyPath { id: flow.id });
        }
        if flow.reserved_bits_per_cycle == 0 || flow.deadline_ps == 0 {
            return Err(CqfSynthError::DegenerateFlow { id: flow.id });
        }
        if !flow.link_delays.is_empty() && flow.link_delays.len() != flow.path.len() {
            return Err(CqfSynthError::LinkDelayLenMismatch {
                id: flow.id,
                path_len: flow.path.len(),
                delays_len: flow.link_delays.len(),
            });
        }
    }

    // Max one-way latency of hop `i` (0 when a flow omits its delays).
    let hop_max =
        |flow: &CqfFlow, i: usize| -> u64 { flow.link_delays.get(i).map_or(0, |ld| ld.max_ps) };

    // Cycle-dominates regime: with T_c > every dᵢ, kᵢ = 1 if dᵢ < DT else 2.
    // The per-flow cycle limit is deadline / (1 + Σkᵢ); the global T_c is the
    // tightest, floored to a whole ns. Track the largest link delay so we can
    // enforce the regime assumption afterward.
    let mut cycle_ps = u64::MAX;
    let mut tightest_id = flows[0].id;
    let mut max_link_delay_ps = 0u64;
    for flow in flows {
        let mut sum_k = 0u64;
        for i in 0..flow.path.len() {
            let d = hop_max(flow, i);
            max_link_delay_ps = max_link_delay_ps.max(d);
            sum_k += if d < dead_time_ps { 1 } else { 2 };
        }
        let limit_ps = flow.deadline_ps / (1 + sum_k);
        if limit_ps < cycle_ps {
            cycle_ps = limit_ps;
            tightest_id = flow.id;
        }
    }
    cycle_ps = (cycle_ps / 1_000) * 1_000; // whole nanoseconds

    // Regime validity: the cycle must be longer than every link (so kᵢ ≤ 2).
    if cycle_ps == 0 || cycle_ps <= max_link_delay_ps {
        return Err(CqfSynthError::LongLinkDeadlineTooTight { id: tightest_id });
    }
    if dead_time_ps >= cycle_ps {
        return Err(CqfSynthError::DeadTimeTooLarge {
            dead_time_ps,
            cycle_ps,
        });
    }

    // Admission against the dead-time-reduced budget.
    let csize_bits = cqf_cycle_budget_bits_with_dead_time(cycle_ps, dead_time_ps, link_rate_bps);
    let mut per_link_bits: BTreeMap<u32, u128> = BTreeMap::new();
    for flow in flows {
        for &link in &flow.path {
            *per_link_bits.entry(link).or_insert(0) += u128::from(flow.reserved_bits_per_cycle);
        }
    }
    for (&link, &required) in &per_link_bits {
        if required > csize_bits {
            return Err(CqfSynthError::Oversubscribed {
                link,
                required_bits: required,
                csize_bits,
            });
        }
    }

    // Buffer sizing over every hop of every flow (empty delays ⇒ zero jitter).
    let mut hops: Vec<LinkDelay> = Vec::new();
    for flow in flows {
        for i in 0..flow.path.len() {
            hops.push(flow.link_delays.get(i).copied().unwrap_or(LinkDelay {
                min_ps: 0,
                max_ps: 0,
            }));
        }
    }
    let buffers =
        cqf_buffer_count(&hops, dead_time_ps, cycle_ps, buffer_cap).map_err(|needed| {
            CqfSynthError::BufferBudgetExceeded {
                needed,
                cap: buffer_cap,
            }
        })?;

    // Self-certify against the EXACT long-link bound (regression guard).
    for flow in flows {
        let d_max: Vec<u64> = (0..flow.path.len()).map(|i| hop_max(flow, i)).collect();
        if cqf_delay_max_longlink_ps(&d_max, dead_time_ps, cycle_ps) > u128::from(flow.deadline_ps)
        {
            return Err(CqfSynthError::SelfCheck(
                "long-link flow delay exceeds deadline",
            ));
        }
    }

    Ok(CqfLongLinkSchedule {
        cycle_time_ps: cycle_ps,
        dead_time_ps,
        csize_bits,
        per_link_bits,
        buffers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 microseconds expressed in picoseconds.
    const TEN_US_PS: u64 = 10_000_000;

    fn flow(id: u32, bits: u64, deadline_ps: u64, path: &[u32]) -> CqfFlow {
        CqfFlow {
            id,
            reserved_bits_per_cycle: bits,
            deadline_ps,
            path: path.to_vec(),
            link_delays: Vec::new(),
        }
    }

    /// The independence anchor: the checker must reproduce the published
    /// worked example from IETF draft-eckert-detnet-tcqf-05 exactly —
    /// 24 hops, 10 us cycle => (24+1)*10 = 250 us; and D_min = (24-1)*10 =
    /// 230 us, i.e. 2T = 20 us of jitter.
    #[test]
    fn delay_bound_matches_published_tcqf_example() {
        assert_eq!(cqf_delay_max_ps(24, TEN_US_PS), 250_000_000); // 250 us
        assert_eq!(cqf_delay_min_ps(24, TEN_US_PS), 230_000_000); // 230 us
        // Jitter is exactly 2T.
        assert_eq!(
            cqf_delay_max_ps(24, TEN_US_PS) - cqf_delay_min_ps(24, TEN_US_PS),
            2 * u128::from(TEN_US_PS)
        );
    }

    /// Assert a synthesized schedule is sound against the independent checker:
    /// every flow meets its deadline and no link exceeds the cycle budget.
    fn assert_cqf_sound(schedule: &CqfSchedule, flows: &[CqfFlow], link_rate_bps: u64) {
        // The budget recorded matches the formula at the chosen cycle time.
        assert_eq!(
            schedule.csize_bits,
            cqf_cycle_budget_bits(schedule.cycle_time_ps, link_rate_bps)
        );
        // Cycle time is a whole number of nanoseconds.
        assert_eq!(schedule.cycle_time_ps % 1_000, 0);
        for fl in flows {
            let hops = fl.path.len() as u32;
            let d_max = cqf_delay_max_ps(hops, schedule.cycle_time_ps);
            assert!(
                d_max <= u128::from(fl.deadline_ps),
                "flow {} delay {d_max} ps exceeds deadline {} ps",
                fl.id,
                fl.deadline_ps
            );
        }
        // Recompute per-link load independently and check the budget.
        let mut load: BTreeMap<u32, u128> = BTreeMap::new();
        for fl in flows {
            for &link in &fl.path {
                *load.entry(link).or_insert(0) += u128::from(fl.reserved_bits_per_cycle);
            }
        }
        assert_eq!(&load, &schedule.per_link_bits);
        for (&link, &required) in &load {
            assert!(
                required <= schedule.csize_bits,
                "link {link} load {required} exceeds csize {}",
                schedule.csize_bits
            );
        }
    }

    #[test]
    fn synth_multiflow_meets_all_deadlines_and_admits() {
        // 1 Gbps links. Three flows over a small line: links 0,1,2.
        let rate = 1_000_000_000;
        let flows = [
            // 3 hops, 500 us deadline -> T <= 125 us limit
            flow(1, 8_000, 500_000_000, &[0, 1, 2]),
            // 2 hops, 300 us deadline -> T <= 100 us limit (binding)
            flow(2, 4_000, 300_000_000, &[0, 1]),
            // 1 hop, 250 us deadline -> T <= 125 us limit
            flow(3, 2_000, 250_000_000, &[2]),
        ];
        let sched = synthesize_cqf(&flows, rate).expect("feasible");
        // Binding limit is flow 2: 300us/(2+1) = 100us, floored to ns = 100us.
        assert_eq!(sched.cycle_time_ps, 100_000_000);
        assert_cqf_sound(&sched, &flows, rate);
    }

    /// Tighter deadline on the binding flow shrinks the synthesized cycle.
    #[test]
    fn synth_tighter_deadline_shrinks_cycle() {
        let rate = 1_000_000_000;
        let loose = [flow(1, 1_000, 400_000_000, &[0, 1, 2])]; // 400us/4 = 100us
        let tight = [flow(1, 1_000, 200_000_000, &[0, 1, 2])]; // 200us/4 = 50us
        let cs_loose = synthesize_cqf(&loose, rate).unwrap().cycle_time_ps;
        let cs_tight = synthesize_cqf(&tight, rate).unwrap().cycle_time_ps;
        assert_eq!(cs_loose, 100_000_000);
        assert_eq!(cs_tight, 50_000_000);
        assert!(cs_tight < cs_loose);
    }

    /// A deadline below the irreducible (H+1)·1ns latency is rejected, naming
    /// the binding flow.
    #[test]
    fn synth_rejects_deadline_below_irreducible_latency() {
        let rate = 1_000_000_000;
        // 3 hops needs (3+1)=4 ns minimum; deadline of 3 ns floors the cycle
        // to 0 ns.
        let flows = [flow(7, 1_000, 3_000, &[0, 1, 2])];
        match synthesize_cqf(&flows, rate) {
            Err(CqfSynthError::DeadlineTooTight { id, hops, .. }) => {
                assert_eq!(id, 7);
                assert_eq!(hops, 3);
            }
            other => panic!("expected DeadlineTooTight, got {other:?}"),
        }
    }

    /// When the deadline-limited cycle is too small to fit a link's
    /// reservation, the port is oversubscribed.
    #[test]
    fn synth_rejects_oversubscribed_link() {
        // 1 Gbps, 1 hop, deadline 200us -> T = 100us -> csize = 1e9 * 100e-6
        // = 100_000 bits. Reserve 200_000 bits on the link -> oversubscribed.
        let rate = 1_000_000_000;
        let flows = [flow(1, 200_000, 200_000_000, &[5])];
        match synthesize_cqf(&flows, rate) {
            Err(CqfSynthError::Oversubscribed {
                link,
                required_bits,
                csize_bits,
            }) => {
                assert_eq!(link, 5);
                assert_eq!(required_bits, 200_000);
                assert_eq!(csize_bits, 100_000);
            }
            other => panic!("expected Oversubscribed, got {other:?}"),
        }
    }

    /// Two flows sharing a link sum their reservations against one budget.
    #[test]
    fn synth_shared_link_sums_reservations() {
        let rate = 1_000_000_000;
        // T = 100us -> csize = 100_000 bits. Two flows of 60_000 each on link 0
        // sum to 120_000 > 100_000 -> oversubscribed.
        let flows = [
            flow(1, 60_000, 200_000_000, &[0]),
            flow(2, 60_000, 200_000_000, &[0]),
        ];
        match synthesize_cqf(&flows, rate) {
            Err(CqfSynthError::Oversubscribed {
                link,
                required_bits,
                ..
            }) => {
                assert_eq!(link, 0);
                assert_eq!(required_bits, 120_000);
            }
            other => panic!("expected Oversubscribed, got {other:?}"),
        }
    }

    #[test]
    fn synth_input_validation() {
        let rate = 1_000_000_000;
        assert_eq!(synthesize_cqf(&[], rate), Err(CqfSynthError::NoFlows));
        assert_eq!(
            synthesize_cqf(&[flow(1, 1_000, 100_000_000, &[0])], 0),
            Err(CqfSynthError::ZeroLinkRate)
        );
        assert_eq!(
            synthesize_cqf(&[flow(1, 1_000, 100_000_000, &[])], rate),
            Err(CqfSynthError::EmptyPath { id: 1 })
        );
        assert_eq!(
            synthesize_cqf(&[flow(1, 0, 100_000_000, &[0])], rate),
            Err(CqfSynthError::DegenerateFlow { id: 1 })
        );
        assert_eq!(
            synthesize_cqf(&[flow(1, 1_000, 0, &[0])], rate),
            Err(CqfSynthError::DegenerateFlow { id: 1 })
        );
        assert_eq!(
            synthesize_cqf(
                &[
                    flow(1, 1_000, 100_000_000, &[0]),
                    flow(1, 1_000, 100_000_000, &[1]),
                ],
                rate
            ),
            Err(CqfSynthError::DuplicateFlowId { id: 1 })
        );
    }

    // ── Long-link CQF oracles (REQ-TSN-SYNTH-CQF-LONGLINK-001) ────────────
    //
    // Three NON-CIRCULAR oracles for the cycle-quantized bound. Constants:
    // T_c = 10 us, DT = 2 us. Hand-computed k_i = 2 + floor((d_i - DT)/T_c).

    const DT_2US_PS: u64 = 2_000_000;

    #[test]
    fn longlink_degeneracy_reproduces_shipped_hop_count_bound() {
        // (A) DEGENERACY: every hop short (d_i^max < DT) ⇒ k_i = 1 ⇒ the
        // long-link bound must EQUAL the independently-shipped hop-count bound
        // cqf_delay_max_ps / cqf_delay_min_ps — which is externally pinned to
        // the draft's 24-hop/250us example. Ties the new code to ground truth
        // WITHOUT re-deriving the new formula.
        let hops = 24usize;
        let d_max = vec![1_000_000u64; hops]; // 1 us < DT ⇒ k=1
        let d_min = vec![1_000_000u64; hops];
        let dmax = cqf_delay_max_longlink_ps(&d_max, DT_2US_PS, TEN_US_PS);
        let dmin = cqf_delay_min_longlink_ps(&d_min, DT_2US_PS, TEN_US_PS);
        assert_eq!(
            dmax,
            cqf_delay_max_ps(hops as u32, TEN_US_PS),
            "D_max ≠ (H+1)·T_c"
        );
        assert_eq!(
            dmin,
            cqf_delay_min_ps(hops as u32, TEN_US_PS),
            "D_min ≠ (H−1)·T_c"
        );
        // Absolute pin: 250 us / 230 us.
        assert_eq!(dmax, 250_000_000);
        assert_eq!(dmin, 230_000_000);
        // Every hop advances exactly one cycle.
        assert_eq!(cqf_hop_advance_cycles(1_000_000, DT_2US_PS, TEN_US_PS), 1);
    }

    #[test]
    fn longlink_discrimination_beats_naive_hop_count() {
        // (B) DISCRIMINATION: 3-hop path d = [1, 25, 8] us ⇒ k = [1, 4, 2].
        // D_max = T_c + T_c·(1+4+2) = 10 + 70 = 80 us. The naive hop-count
        // bound (H+1)·T_c = 40 us UNDER-bounds the true worst case by 2× —
        // proving the long-link bound is a strictly different, sounder
        // computation, not a rename of the old one.
        let d = [1_000_000u64, 25_000_000, 8_000_000];
        assert_eq!(cqf_hop_advance_cycles(d[0], DT_2US_PS, TEN_US_PS), 1);
        assert_eq!(cqf_hop_advance_cycles(d[1], DT_2US_PS, TEN_US_PS), 4);
        assert_eq!(cqf_hop_advance_cycles(d[2], DT_2US_PS, TEN_US_PS), 2);
        let dmax = cqf_delay_max_longlink_ps(&d, DT_2US_PS, TEN_US_PS);
        assert_eq!(dmax, 80_000_000, "D_max must be 80 us");
        let naive = cqf_delay_max_ps(d.len() as u32, TEN_US_PS); // 40 us
        assert!(
            naive < dmax,
            "naive (H+1)T_c {naive} must UNDER-bound long-link {dmax}"
        );
        // D_min sums the H−1 intermediate hops: (1+4)·10 = 50 us.
        let dmin = cqf_delay_min_longlink_ps(&d, DT_2US_PS, TEN_US_PS);
        assert_eq!(dmin, 50_000_000);
    }

    #[test]
    fn longlink_never_inverts_and_flags_naive_optimism() {
        // (C) OPTIMISM/INVERSION GUARD. Edge case H=2, both links d=15 us
        // (> T_c) ⇒ k = [3, 3] ⇒ D_max = 10 + 60 = 70 us, while the naive
        // (H+1)·T_c = 30 us. A 40 us-deadline flow PASSES the naive test but
        // its true worst case is 70 us — the exact unsoundness this feature
        // closes. Also: D_min ≤ D_max must hold for ALL inputs (the prior
        // spec inverted; this construction cannot).
        let d = [15_000_000u64, 15_000_000];
        assert_eq!(cqf_hop_advance_cycles(15_000_000, DT_2US_PS, TEN_US_PS), 3);
        let dmax = cqf_delay_max_longlink_ps(&d, DT_2US_PS, TEN_US_PS);
        assert_eq!(dmax, 70_000_000);
        assert!(
            cqf_delay_max_ps(2, TEN_US_PS) < dmax,
            "naive 30us must under-bound 70us"
        );
        // Never-inverts property across a grid of hop counts, delays, DT.
        for dt in [0u64, 1_000_000, 5_000_000, 9_000_000] {
            for &per_hop in &[0u64, 500_000, 2_000_000, 12_000_000, 33_000_000] {
                for h in 1..=6usize {
                    let dv = vec![per_hop; h];
                    let mx = cqf_delay_max_longlink_ps(&dv, dt, TEN_US_PS);
                    let mn = cqf_delay_min_longlink_ps(&dv, dt, TEN_US_PS);
                    assert!(
                        mn <= mx,
                        "inverted: D_min {mn} > D_max {mx} (h={h}, d={per_hop}, dt={dt})"
                    );
                }
            }
        }
    }

    #[test]
    fn longlink_csize_shrinks_by_dead_time_and_degenerates() {
        // csize = (T_c − DT)·rate. At DT=0 it equals the shipped T_c·rate;
        // with DT>0 it is strictly smaller (the dead-time tail is unusable).
        let rate = 1_000_000_000u64; // 1 Gbps
        let full = cqf_cycle_budget_bits(TEN_US_PS, rate);
        assert_eq!(
            cqf_cycle_budget_bits_with_dead_time(TEN_US_PS, 0, rate),
            full
        );
        let with_dt = cqf_cycle_budget_bits_with_dead_time(TEN_US_PS, DT_2US_PS, rate);
        // (10us−2us)·1Gbps = 8us·1e9 = 8000 bits.
        assert_eq!(with_dt, 8_000);
        assert!(with_dt < full);
    }

    #[test]
    fn longlink_buffer_count_from_jitter_and_cap() {
        // B = max(3, max_i((k_i^max − k_i^min)+2)); Err(needed) past the cap.
        // Hop with min=1us (k=1), max=25us (k=4): jitter 3 cycles ⇒ B_i=5.
        let jittery = [LinkDelay {
            min_ps: 1_000_000,
            max_ps: 25_000_000,
        }];
        assert_eq!(cqf_buffer_count(&jittery, DT_2US_PS, TEN_US_PS, 7), Ok(5));
        // All-short deterministic links ⇒ floor of 3.
        let short = [LinkDelay {
            min_ps: 0,
            max_ps: 500_000,
        }; 4];
        assert_eq!(cqf_buffer_count(&short, DT_2US_PS, TEN_US_PS, 7), Ok(3));
        // Exceeds the 7-cycle cap ⇒ Err(needed), never a silent down-clamp.
        // max=90us ⇒ k_max=2+⌊(90−2)/10⌋=10; min=0 ⇒ k_min=1; B=(10−1)+2=11.
        let huge = [LinkDelay {
            min_ps: 0,
            max_ps: 90_000_000,
        }];
        assert_eq!(cqf_buffer_count(&huge, DT_2US_PS, TEN_US_PS, 7), Err(11));
    }

    fn ll_flow(
        id: u32,
        bits: u64,
        deadline_ps: u64,
        path: &[u32],
        delays: &[(u64, u64)],
    ) -> CqfFlow {
        CqfFlow {
            id,
            reserved_bits_per_cycle: bits,
            deadline_ps,
            path: path.to_vec(),
            link_delays: delays
                .iter()
                .map(|&(min_ps, max_ps)| LinkDelay { min_ps, max_ps })
                .collect(),
        }
    }

    #[test]
    fn longlink_synth_feasible_in_cycle_dominates_regime() {
        // 2-hop path, both links 5 us (> DT=2us ⇒ k=2 each), deadline 150 us.
        // Σk=4 ⇒ T_c limit = 150/(1+4) = 30 us > max link 5 us ✓. D_max =
        // 30·(1+4) = 150 us ≤ deadline. csize = (30−2)us·1Gbps = 28_000 bits.
        let rate = 1_000_000_000u64;
        let flows = [ll_flow(
            1,
            1_000,
            150_000_000,
            &[0, 1],
            &[(5_000_000, 5_000_000); 2],
        )];
        let sched = synthesize_cqf_longlink(&flows, rate, DT_2US_PS, CQF_DEFAULT_BUFFER_CAP)
            .expect("feasible in cycle-dominates regime");
        assert_eq!(sched.cycle_time_ps, 30_000_000);
        assert_eq!(sched.dead_time_ps, DT_2US_PS);
        assert_eq!(sched.csize_bits, 28_000);
        assert_eq!(sched.buffers, 3, "deterministic links ⇒ 3-buffer floor");
        // The true long-link D_max at the chosen cycle meets the deadline.
        let dmax =
            cqf_delay_max_longlink_ps(&[5_000_000, 5_000_000], DT_2US_PS, sched.cycle_time_ps);
        assert!(dmax <= 150_000_000);
    }

    #[test]
    fn longlink_synth_rejects_deadline_needing_sublink_cycle() {
        // A 30 us link with a 60 us deadline: Σk=2 ⇒ T_c limit = 60/3 = 20 us,
        // which is SHORTER than the 30 us link — the cycle-dominates regime
        // cannot serve it, so we get a structured error, never an unsound
        // config that silently assumes a single-cycle hop.
        let rate = 1_000_000_000u64;
        let flows = [ll_flow(
            7,
            1_000,
            60_000_000,
            &[0],
            &[(30_000_000, 30_000_000)],
        )];
        assert_eq!(
            synthesize_cqf_longlink(&flows, rate, DT_2US_PS, CQF_DEFAULT_BUFFER_CAP),
            Err(CqfSynthError::LongLinkDeadlineTooTight { id: 7 })
        );
    }

    #[test]
    fn longlink_synth_all_short_matches_hop_count_synthesis() {
        // Every hop short (dᵢ=1us < DT) ⇒ the long-link synthesizer picks the
        // SAME cycle as the hop-count synthesize_cqf and its D_max is (H+1)·T_c.
        let rate = 1_000_000_000u64;
        let short = &[(1_000_000u64, 1_000_000u64); 3];
        let ll = [ll_flow(1, 1_000, 80_000_000, &[0, 1, 2], short)];
        let hop = [flow(1, 1_000, 80_000_000, &[0, 1, 2])];
        let a = synthesize_cqf_longlink(&ll, rate, DT_2US_PS, CQF_DEFAULT_BUFFER_CAP)
            .expect("feasible");
        let b = synthesize_cqf(&hop, rate).expect("feasible");
        assert_eq!(
            a.cycle_time_ps, b.cycle_time_ps,
            "same cycle as hop-count synth"
        );
        // D_max degenerates to (H+1)·T_c.
        assert_eq!(
            cqf_delay_max_longlink_ps(&[1_000_000; 3], DT_2US_PS, a.cycle_time_ps),
            cqf_delay_max_ps(3, a.cycle_time_ps)
        );
    }

    #[test]
    fn longlink_synth_flags_buffer_budget_and_len_mismatch() {
        let rate = 1_000_000_000u64;
        // In the cycle-dominates regime buffers are always the 3-cycle floor
        // (T_c > every link ⇒ jitter ≤ 1 cycle), so the only way to trip the
        // budget is a cap BELOW the draft's mandated 3 — a misconfiguration
        // the synthesizer must reject, not silently under-buffer.
        let f = [ll_flow(
            1,
            1_000,
            100_000_000,
            &[0],
            &[(1_000_000, 1_000_000)],
        )];
        assert_eq!(
            synthesize_cqf_longlink(&f, rate, DT_2US_PS, 2),
            Err(CqfSynthError::BufferBudgetExceeded { needed: 3, cap: 2 })
        );
        // link_delays length must match the path.
        let bad = [ll_flow(
            2,
            1_000,
            100_000_000,
            &[0, 1],
            &[(1_000_000, 1_000_000)],
        )];
        assert_eq!(
            synthesize_cqf_longlink(&bad, rate, DT_2US_PS, CQF_DEFAULT_BUFFER_CAP),
            Err(CqfSynthError::LinkDelayLenMismatch {
                id: 2,
                path_len: 2,
                delays_len: 1
            })
        );
    }
}
