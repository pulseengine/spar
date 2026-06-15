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
}
