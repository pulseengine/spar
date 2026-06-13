//! FP-TFA / TFA++ — Total Flow Analysis network-calculus bounds.
//!
//! Total Flow Analysis (Bouillard 2021, "Trade-off between accuracy and
//! tractability of Network Calculus in FIFO networks") computes per-server
//! delay bounds by aggregating *all* flows crossing a FIFO server into a
//! single arrival curve and bounding the whole aggregate against the
//! server's full service curve. Every flow crossing a server experiences
//! the same per-server delay; an end-to-end bound is the sum of the
//! per-server delays along the flow's path.
//!
//! Three properties distinguish TFA from the per-stream feed-forward
//! (separated-flow) walk in `spar-analysis`'s `wctt.rs`, and all three are
//! *soundness* requirements, not mere looseness:
//!
//! 1. **Self-inclusive aggregate against the full service curve.** The
//!    delay at a FIFO server is `delay_bound(Σ_f α_f, β)` where the sum
//!    runs over every flow crossing the server *including the tagged flow
//!    itself*, and `β` is the server's full rate-latency curve (no
//!    residual decomposition).
//!
//! 2. **Burst propagation by `ρ · D`, not `ρ · T`.** The output burst of
//!    a flow leaving a server is `σ + ρ · D_h` where `D_h` is the
//!    *server delay*, not the service latency `T_h`. Since `D_h ≥ T_h`,
//!    reusing the standard output-bound theorem (which inflates by
//!    `ρ · T`) would under-count the downstream burst and yield an
//!    invalid (optimistic) bound. See [`inflate_burst`].
//!
//! 3. **A fixed point over per-server input bursts** buys *valid bounds
//!    under cyclic dependencies*: where the feed-forward walk has no
//!    well-defined answer (the burst at a server depends transitively on
//!    itself), TFA iterates the burst vector to its least fixed point.
//!
//! The converged per-(flow, server) input bursts are also the
//! initialization phase for PLP (REQ-NC-PLP-001, v0.18.0): the polynomial
//! LP is warm-started from this fixed point.
//!
//! This module reuses the tested operators in [`crate::curves`]
//! ([`delay_bound`], [`output_bound`]) for all unit arithmetic — it does
//! not reimplement the picosecond / byte conversions.
//!
//! # Soundness contract
//!
//! The per-server rate check (`Σρ < R`) is *necessary* but **not
//! sufficient** for the burst fixed point to converge: the burst-coupling
//! matrix `M` has `M[h][k] = (Σ ρ of flows arriving at h from k) / R_h`,
//! and a flow reaching `h` at path-position `p` contributes its rate to
//! `p` rows — so a rate-stable network can still have spectral radius
//! `ρ(M) ≥ 1` once a path revisits a server (feed-forward networks are
//! always fine because `M` is nilpotent under topological order). TFA
//! therefore makes exactly one claim: it returns a **valid** bound when
//! the fixed point converges (`ρ(M) < 1`), and returns
//! [`TfaError::NonConvergent`] — never a finite-but-invalid number — when
//! a cycle has gain `≥ 1`. It does **not** claim that rate-stability
//! implies convergence.
//!
//! Reference: A. Bouillard, "Trade-off between accuracy and tractability
//! of Network Calculus in FIFO networks", Performance Evaluation, 2021.

use crate::curves::{ArrivalCurve, ServiceCurve, delay_bound, output_bound};

/// Hard cap on fixed-point iterations before declaring non-convergence.
/// A monotone-increasing integer burst vector that has not stabilized by
/// here is diverging (cycle gain ≥ 1); we return a structured error rather
/// than loop forever.
pub const TFA_MAX_ITER: u32 = 1_000;

/// Per-burst saturation ceiling (bytes). If any input burst exceeds this
/// during iteration the fixed point is diverging; bail with
/// [`TfaError::NonConvergent`] rather than churn to `TFA_MAX_ITER`.
pub const TFA_BURST_CEILING_BYTES: u64 = 1 << 50;

/// A flow through the network: its source arrival curve and the ordered
/// list of servers it crosses (indices into the `services` slice passed
/// to [`tfa_bound`]).
#[derive(Debug, Clone)]
pub struct TfaFlow {
    /// The arrival curve of the flow at its source (before any server).
    pub alpha: ArrivalCurve,
    /// Ordered server indices the flow traverses.
    pub path: Vec<usize>,
}

/// The result of a converged TFA fixed point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TfaResult {
    /// Per-server total-flow delay `D_h` in picoseconds, indexed like the
    /// `services` slice. Servers with no flow crossing them are `0`.
    pub server_delay_ps: Vec<u64>,
    /// Per-flow end-to-end delay in picoseconds, indexed like the input
    /// `flows` slice (`Σ_h D_h` over the flow's path).
    pub flow_delay_ps: Vec<u64>,
    /// Number of fixed-point iterations performed to reach convergence.
    pub iterations: u32,
}

/// Errors returned by [`tfa_bound`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TfaError {
    /// No flows, or no servers.
    EmptyNetwork,
    /// A flow path references a server index `>= services.len()`.
    OutOfRange,
    /// Aggregate sustained rate `≥` service rate at a server: the
    /// necessary stability condition fails and no finite bound exists.
    /// (Strict `<` is required for robustness under cyclic dependencies,
    /// where equality already permits unbounded burst growth.)
    Unstable {
        /// Index of the offending server.
        server: usize,
        /// Summed sustained rate of all flows crossing it (bits/s).
        agg_rate_bps: u64,
        /// The server's service rate (bits/s).
        service_rate_bps: u64,
    },
    /// The burst fixed point did not stabilize within [`TFA_MAX_ITER`]
    /// iterations or a burst exceeded [`TFA_BURST_CEILING_BYTES`]: the
    /// per-server stability holds but a cycle has gain `≥ 1`.
    NonConvergent {
        /// Iterations performed before bailing.
        iterations: u32,
    },
}

impl core::fmt::Display for TfaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyNetwork => write!(f, "TFA: no flows or no servers"),
            Self::OutOfRange => write!(f, "TFA: flow path references an unknown server index"),
            Self::Unstable {
                server,
                agg_rate_bps,
                service_rate_bps,
            } => write!(
                f,
                "TFA: server {server} unstable: aggregate rate {agg_rate_bps} bps \
                 >= service rate {service_rate_bps} bps; bound is unbounded"
            ),
            Self::NonConvergent { iterations } => write!(
                f,
                "TFA: burst fixed point did not converge within {iterations} iterations \
                 (cycle gain >= 1)"
            ),
        }
    }
}

impl core::error::Error for TfaError {}

/// Sum a set of arrival curves into one aggregate leaky bucket: bursts
/// summed, sustained rates summed, peak dropped (the aggregate of
/// peak-capped curves is not itself a single leaky bucket, so we fall
/// back to the conservative uncapped sum). Saturating throughout.
///
/// `curves.rs` has no arrival-curve summation primitive — the per-stream
/// walk inlines its own `u128` accumulation — so TFA provides this
/// reusable, self-inclusive version.
pub fn aggregate(flows: impl IntoIterator<Item = ArrivalCurve>) -> ArrivalCurve {
    let mut burst_bytes: u64 = 0;
    let mut sustained_rate_bps: u64 = 0;
    for a in flows {
        burst_bytes = burst_bytes.saturating_add(a.burst_bytes);
        sustained_rate_bps = sustained_rate_bps.saturating_add(a.sustained_rate_bps);
    }
    ArrivalCurve::affine(burst_bytes, sustained_rate_bps)
}

/// Inflate a flow's burst by `ρ · delay_ps` — the TFA output-burst
/// propagation. Implemented as a documented reuse of [`output_bound`]:
/// `output_bound(α, rate_latency(R ≥ ρ, D))` evaluates to
/// `(σ + ρ·D, ρ, peak)`. We pick a throwaway service rate `ρ + 1` so the
/// stability check inside `output_bound` can never trip.
fn inflate_burst(alpha: &ArrivalCurve, delay_ps: u64) -> ArrivalCurve {
    let probe = ServiceCurve::rate_latency(alpha.sustained_rate_bps.saturating_add(1), delay_ps);
    // ρ < ρ + 1, so output_bound never returns UnstableServer here.
    output_bound(alpha, &probe).expect("probe rate ρ+1 > ρ ⇒ stable")
}

/// Run the FP-TFA fixed point.
///
/// `services[h]` is the rate-latency service curve at server `h`;
/// `flows[i].path` lists the servers flow `i` crosses in order. Returns
/// per-server and per-flow delay bounds, or a [`TfaError`].
///
/// The state vector is the input burst of each flow at each server on its
/// path. It is initialized to the source burst at every position (zero
/// inflation) and iterated: `D_h = delay_bound(Σ α, β_h)` per server, then
/// each flow's burst at the next server is inflated by `ρ · D_h`. The
/// iteration is monotone non-decreasing in integers, so it stabilizes in
/// finitely many steps when the network is stable; divergence (a cycle of
/// gain ≥ 1) is caught by [`TFA_MAX_ITER`] and [`TFA_BURST_CEILING_BYTES`].
pub fn tfa_bound(flows: &[TfaFlow], services: &[ServiceCurve]) -> Result<TfaResult, TfaError> {
    if flows.is_empty() || services.is_empty() {
        return Err(TfaError::EmptyNetwork);
    }
    let n_servers = services.len();
    for flow in flows {
        if flow.path.iter().any(|&h| h >= n_servers) {
            return Err(TfaError::OutOfRange);
        }
    }

    // Stability precheck (once): aggregate sustained rate < service rate
    // at every server, summed over all flows crossing it. Rates are
    // invariant under TFA propagation (the output rate equals the input
    // rate), so this is checked a single time.
    let mut agg_rate = vec![0u64; n_servers];
    for flow in flows {
        for &h in &flow.path {
            agg_rate[h] = agg_rate[h].saturating_add(flow.alpha.sustained_rate_bps);
        }
    }
    for (h, &rate) in agg_rate.iter().enumerate() {
        // A server with no flow crossing it has agg_rate 0; skip it.
        if rate == 0 {
            continue;
        }
        if rate >= services[h].rate_bps {
            return Err(TfaError::Unstable {
                server: h,
                agg_rate_bps: rate,
                service_rate_bps: services[h].rate_bps,
            });
        }
    }

    // State: input burst of flow i at position j on its path. Initialized
    // to the source burst everywhere (zero inflation).
    let mut bursts: Vec<Vec<u64>> = flows
        .iter()
        .map(|f| vec![f.alpha.burst_bytes; f.path.len()])
        .collect();

    let mut server_delay_ps = vec![0u64; n_servers];
    let mut iterations = 0u32;

    loop {
        iterations += 1;

        // 1. Per-server delay from the current aggregate input bursts.
        //    Aggregate burst at server h = Σ over (flow i, position j with
        //    path[i][j] == h) of bursts[i][j]; aggregate rate is the
        //    pre-summed agg_rate[h].
        let mut agg_burst = vec![0u64; n_servers];
        for (i, flow) in flows.iter().enumerate() {
            for (j, &h) in flow.path.iter().enumerate() {
                agg_burst[h] = agg_burst[h].saturating_add(bursts[i][j]);
            }
        }
        for h in 0..n_servers {
            if agg_rate[h] == 0 {
                server_delay_ps[h] = 0;
                continue;
            }
            let agg = ArrivalCurve::affine(agg_burst[h], agg_rate[h]);
            // Stability was prechecked, so delay_bound cannot return
            // UnstableServer here; map any unexpected error defensively.
            match delay_bound(&agg, &services[h]) {
                Ok(d) => server_delay_ps[h] = d,
                Err(_) => {
                    return Err(TfaError::Unstable {
                        server: h,
                        agg_rate_bps: agg_rate[h],
                        service_rate_bps: services[h].rate_bps,
                    });
                }
            }
        }

        // 2. Propagate: burst of flow i entering position j+1 is its burst
        //    at position j inflated by ρ_i · D_{path[j]}. Position 0 is
        //    pinned to the source burst.
        let mut next: Vec<Vec<u64>> = bursts.clone();
        let mut diverged = false;
        for (i, flow) in flows.iter().enumerate() {
            for j in 1..flow.path.len() {
                let prev_server = flow.path[j - 1];
                let alpha_in =
                    ArrivalCurve::affine(bursts[i][j - 1], flow.alpha.sustained_rate_bps);
                let out = inflate_burst(&alpha_in, server_delay_ps[prev_server]);
                next[i][j] = out.burst_bytes;
                if out.burst_bytes > TFA_BURST_CEILING_BYTES {
                    diverged = true;
                }
            }
        }

        if diverged {
            return Err(TfaError::NonConvergent { iterations });
        }
        if next == bursts {
            break; // fixed point reached
        }
        if iterations >= TFA_MAX_ITER {
            return Err(TfaError::NonConvergent { iterations });
        }
        bursts = next;
    }

    // End-to-end delay per flow = Σ of per-server delays along its path.
    let flow_delay_ps = flows
        .iter()
        .map(|f| {
            f.path
                .iter()
                .fold(0u64, |acc, &h| acc.saturating_add(server_delay_ps[h]))
        })
        .collect();

    Ok(TfaResult {
        server_delay_ps,
        flow_delay_ps,
        iterations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GBPS: u64 = 1_000_000_000;
    const HUNDRED_MBPS: u64 = 100_000_000;
    const TEN_US_PS: u64 = 10_000_000;
    const FIVE_US_PS: u64 = 5_000_000;
    const FRAME: u64 = 1500;

    /// (a) Golden fixture — 2-server tandem with a cross-flow joining at
    /// server 0. Every number is hand-computed against the exact
    /// `curves.rs` operator semantics (delay rounds σ/R up; burst
    /// inflation ρ·D rounds down).
    ///
    /// Server 0: agg σ = 3000 B, ρ = 200 Mbps; D_0 = 10us + 24us = 34us.
    /// Tagged out-burst @0 = 1500 + floor(1e8·34e6/8e12) = 1925 B
    ///   (ρ·D = 425 B — NOT ρ·T = 125 B; this discriminates the bug).
    /// Server 1: agg σ = 1925 B (tagged only), ρ = 100 Mbps;
    ///   D_1 = 5us + ceil(1925·8e12/1e9) = 5us + 15.4us = 20.4us.
    /// Tagged end-to-end = 34us + 20.4us = 54.4us; cross = D_0 = 34us.
    #[test]
    fn tandem_cross_golden() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
            ServiceCurve::rate_latency(GBPS, FIVE_US_PS),
        ];
        let tagged = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0, 1],
        };
        let cross = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0],
        };
        let r = tfa_bound(&[tagged, cross], &services).unwrap();

        assert_eq!(r.server_delay_ps[0], 34_000_000, "D_0");
        assert_eq!(r.server_delay_ps[1], 20_400_000, "D_1");
        assert_eq!(r.flow_delay_ps[0], 54_400_000, "tagged end-to-end");
        assert_eq!(r.flow_delay_ps[1], 34_000_000, "cross == D_0");
    }

    /// The burst really is inflated by ρ·D (425 B), not ρ·T (125 B). If
    /// `inflate_burst` regressed to the standard output-bound theorem,
    /// D_1 would shrink to 5us + ceil(1625·8e12/1e9) = 18us and this test
    /// (and the golden above) would fail.
    #[test]
    fn burst_propagation_uses_rho_d_not_rho_t() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
            ServiceCurve::rate_latency(GBPS, FIVE_US_PS),
        ];
        let tagged = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0, 1],
        };
        let r = tfa_bound(&[tagged], &services).unwrap();
        // Single flow: D_0 = 10us + ceil(1500·8e12/1e9) = 10us + 12us = 22us.
        assert_eq!(r.server_delay_ps[0], 22_000_000, "single-flow D_0");
        // Out-burst @0 = 1500 + floor(1e8·22e6/8e12) = 1500 + 275 = 1775 B.
        // D_1 = 5us + ceil(1775·8e12/1e9) = 5us + 14.2us = 19_200_000 ps.
        assert_eq!(
            r.server_delay_ps[1], 19_200_000,
            "single-flow D_1 (ρ·D burst)"
        );
    }

    /// (c) Cyclic 2-server topology: flow A: 0→1, flow B: 1→0. Each
    /// server aggregates both flows. Low utilization (agg 200 Mbps ≪
    /// 1 Gbps) ⇒ the fixed point converges to a finite bound where the
    /// feed-forward walk has no defined answer.
    #[test]
    fn cyclic_converges() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
        ];
        let a = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0, 1],
        };
        let b = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![1, 0],
        };
        let r = tfa_bound(&[a, b], &services).unwrap();
        assert!(
            r.flow_delay_ps.iter().all(|&d| d > 0 && d < u64::MAX),
            "finite positive bounds: {:?}",
            r.flow_delay_ps
        );
        assert!(r.iterations >= 1 && r.iterations < TFA_MAX_ITER);
    }

    /// (c-stable) A *high-utilization but stable* symmetric 2-cycle still
    /// converges. Any 2-cycle has burst-coupling spectral radius
    /// `√(ρ_a·ρ_b / R_0·R_1)`, which the rate-stability precheck keeps
    /// `< 1` — so 450 Mbps each (900 Mbps aggregate < 1 Gbps) converges to
    /// a finite bound, it does not diverge. (An earlier draft of this test
    /// wrongly expected divergence; the real divergence boundary needs a
    /// path that *revisits* a server — see `revisiting_cycle_diverges`.)
    #[test]
    fn cyclic_high_util_still_converges() {
        let hot = 450_000_000u64; // 450 Mbps each ⇒ 900 Mbps aggregate < 1 Gbps
        let services = vec![
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
        ];
        let a = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, hot),
            path: vec![0, 1],
        };
        let b = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, hot),
            path: vec![1, 0],
        };
        let r = tfa_bound(&[a, b], &services).unwrap();
        assert!(
            r.flow_delay_ps.iter().all(|&d| d > 0 && d < u64::MAX),
            "finite bounds at high stable utilization: {:?}",
            r.flow_delay_ps
        );
    }

    /// (c-guard) The divergence guard *is* reachable through the public
    /// API on precheck-passing input — but it needs a path that revisits a
    /// server, so a flow's burst feeds back onto itself with gain `> 1`.
    ///
    /// Flow path `[0, 1, 2, 0]`, ρ = 900 Mbps. Per-server rate check
    /// passes: server 0 carries the flow twice (1.8 Gbps < 1.9 Gbps),
    /// servers 1 and 2 once (0.9 Gbps < 1.0 Gbps). But the burst-coupling
    /// spectral radius ≈ 1.33, so bursts grow geometrically until they hit
    /// `TFA_BURST_CEILING_BYTES`, and TFA returns `NonConvergent` rather
    /// than a wrong (finite-but-invalid) bound. This is the soundness
    /// contract: never emit a bound for a non-convergent fixed point.
    #[test]
    fn revisiting_cycle_diverges() {
        let rate = 900_000_000u64;
        let services = vec![
            ServiceCurve::rate_latency(1_900_000_000, TEN_US_PS), // server 0: holds 2× the flow
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
            ServiceCurve::rate_latency(GBPS, TEN_US_PS),
        ];
        let flow = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, rate),
            path: vec![0, 1, 2, 0], // revisits server 0
        };
        let e = tfa_bound(&[flow], &services).unwrap_err();
        assert!(matches!(e, TfaError::NonConvergent { .. }), "got {e:?}");
    }

    /// Stability precheck: aggregate rate ≥ service rate ⇒ Unstable.
    #[test]
    fn unstable_when_aggregate_exceeds_service() {
        let services = vec![ServiceCurve::rate_latency(HUNDRED_MBPS, TEN_US_PS)];
        // Two 100 Mbps flows on a 100 Mbps server ⇒ agg 200 Mbps ≥ 100.
        let a = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0],
        };
        let b = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0],
        };
        let e = tfa_bound(&[a, b], &services).unwrap_err();
        assert!(
            matches!(e, TfaError::Unstable { server: 0, .. }),
            "got {e:?}"
        );
    }

    #[test]
    fn empty_and_out_of_range() {
        let services = vec![ServiceCurve::rate_latency(GBPS, TEN_US_PS)];
        assert_eq!(
            tfa_bound(&[], &services).unwrap_err(),
            TfaError::EmptyNetwork
        );
        let f = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0],
        };
        assert_eq!(
            tfa_bound(std::slice::from_ref(&f), &[]).unwrap_err(),
            TfaError::EmptyNetwork
        );
        let bad = TfaFlow {
            alpha: ArrivalCurve::affine(FRAME, HUNDRED_MBPS),
            path: vec![0, 5],
        };
        assert_eq!(
            tfa_bound(&[bad], &services).unwrap_err(),
            TfaError::OutOfRange
        );
    }

    /// `aggregate` sums bursts and rates, drops peak.
    #[test]
    fn aggregate_sums() {
        let a = ArrivalCurve::with_peak(1000, HUNDRED_MBPS, GBPS);
        let b = ArrivalCurve::affine(500, HUNDRED_MBPS);
        let agg = aggregate([a, b]);
        assert_eq!(agg.burst_bytes, 1500);
        assert_eq!(agg.sustained_rate_bps, 200_000_000);
        assert_eq!(agg.peak_rate_bps, None);
    }
}
