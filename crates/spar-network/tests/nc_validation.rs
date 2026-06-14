//! NC cross-validation against panco (REQ-NC-VALIDATION-001).
//!
//! Independent validation of spar-network's FP-TFA engine (`tfa_bound`,
//! REQ-NC-TFA-001) against [panco][panco] — Anne Bouillard's own reference
//! implementation of the LP-based network-calculus results spar's `tfa.rs`
//! is derived from. Where `tfa.rs::tests::tandem_cross_golden` is a *hand*-
//! computed golden fixture (it can only confirm spar agrees with itself),
//! this test pins bounds an **external, independent** tool produced and
//! checks the two things a self-consistent golden cannot:
//!
//! 1. **Soundness** — `spar_bound ≥ panco_exact`. panco's exact ELP bound
//!    (Bouillard–Stea, ValueTools 2012) is the *true* worst-case FIFO delay;
//!    no sound NC method may fall below it. An unsound (optimistic) bound is
//!    *also* `≤ TFA`, so this floor is the only check that can catch it.
//! 2. **Arithmetic agreement** — `spar_bound ≈ panco_TFA` within rounding.
//!    Confirms spar implements the TFA *method* (not merely *some* sound
//!    bound): a sound-but-grossly-pessimistic bug fails this while passing
//!    soundness.
//!
//! The pinned numbers, their provenance (panco commit, `lp_solve` version,
//! unit mapping) and the regeneration recipe live in
//! `tests/oracles/panco/README.md`; the generator is `panco_bench.py`.
//! panco is **not** vendored and Python is **not** in CI — only the numbers
//! are pinned (the `dataset-itc30nc` pattern).
//!
//! [panco]: https://github.com/Huawei-Paris-Research-Center/panco

use spar_network::{ArrivalCurve, ServiceCurve, TfaFlow, tfa_bound};

const GBPS: u64 = 1_000_000_000;
const HUNDRED_MBPS: u64 = 100_000_000;
const FRAME: u64 = 1500;
const US_PS: u64 = 1_000_000; // 1 microsecond in picoseconds

/// A single panco-derived expectation for one flow.
struct FlowRef {
    /// panco `TfaLP` bound for this flow, in microseconds (the method spar
    /// implements — spar must agree within rounding).
    panco_tfa_us: f64,
    /// panco exact ELP bound, in microseconds (true worst case — spar must
    /// be `≥` this or it is unsound).
    panco_exact_us: f64,
}

/// Agreement tolerance: spar's bound must track panco's TFA to within this
/// fraction. spar accumulates integer ceil/floor ps-rounding per server
/// (`σ/R` up, `ρ·D` down); across all three fixtures (incl. the 3-hop
/// `three_server_line`) the worst observed gap is 0.056 %, so 0.3 % keeps
/// ~5× headroom over ps-rounding while still failing on a real method
/// divergence — a sound-but-pessimistic bug would diverge by percent, not
/// per-mille. (A looser bound such as 2 % would silently accept a 2 %-wrong
/// safety bound, defeating the oracle's purpose.)
const AGREEMENT_TOL: f64 = 0.003;

fn affine_flow(burst: u64, rate: u64, path: &[usize]) -> TfaFlow {
    TfaFlow {
        alpha: ArrivalCurve::affine(burst, rate),
        path: path.to_vec(),
    }
}

/// Assert spar's per-flow TFA bounds are sound against panco's exact bound
/// and agree with panco's TFA, for one fixture.
fn check_fixture(name: &str, flows: &[TfaFlow], services: &[ServiceCurve], refs: &[FlowRef]) {
    let r =
        tfa_bound(flows, services).unwrap_or_else(|e| panic!("{name}: spar tfa_bound failed: {e}"));
    assert_eq!(
        r.flow_delay_ps.len(),
        refs.len(),
        "{name}: flow count mismatch"
    );

    for (i, (&spar_ps, fref)) in r.flow_delay_ps.iter().zip(refs).enumerate() {
        let exact_ps = (fref.panco_exact_us * US_PS as f64).round() as u64;
        let tfa_ps = (fref.panco_tfa_us * US_PS as f64).round() as u64;

        // (1) SOUNDNESS — spar must not undercut the true worst case.
        assert!(
            spar_ps >= exact_ps,
            "{name} flow {i}: UNSOUND — spar {spar_ps} ps < panco exact {exact_ps} ps \
             ({:.4} µs < {:.4} µs)",
            spar_ps as f64 / US_PS as f64,
            fref.panco_exact_us,
        );

        // (2) AGREEMENT — spar must track the TFA reference within rounding.
        let rel = (spar_ps as f64 - tfa_ps as f64).abs() / tfa_ps as f64;
        assert!(
            rel <= AGREEMENT_TOL,
            "{name} flow {i}: spar TFA {spar_ps} ps disagrees with panco TFA {tfa_ps} ps \
             by {:.3}% (> {:.1}% tol) — {:.4} µs vs {:.4} µs",
            rel * 100.0,
            AGREEMENT_TOL * 100.0,
            spar_ps as f64 / US_PS as f64,
            fref.panco_tfa_us,
        );
    }
}

/// fixture 1 — `tandem_cross`: 2-server tandem, tagged `[0→1]` + cross `[0]`.
/// Mirrors `tfa.rs::tests::tandem_cross_golden`; panco independently
/// confirms the hand-computed golden (tagged 54.39 µs, cross 34.0 µs exact).
#[test]
fn panco_tandem_cross_sound_and_agrees() {
    let services = vec![
        ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ServiceCurve::rate_latency(GBPS, 5 * US_PS),
    ];
    let flows = vec![
        affine_flow(FRAME, HUNDRED_MBPS, &[0, 1]),
        affine_flow(FRAME, HUNDRED_MBPS, &[0]),
    ];
    let refs = [
        FlowRef {
            panco_tfa_us: 54.3861,
            panco_exact_us: 39.0,
        }, // tagged
        FlowRef {
            panco_tfa_us: 34.0,
            panco_exact_us: 34.0,
        }, // cross
    ];
    check_fixture("tandem_cross", &flows, &services, &refs);
}

/// fixture 2 — `single_flow_tandem`: one flow `[0→1]`, same servers.
#[test]
fn panco_single_flow_tandem_sound_and_agrees() {
    let services = vec![
        ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ServiceCurve::rate_latency(GBPS, 5 * US_PS),
    ];
    let flows = vec![affine_flow(FRAME, HUNDRED_MBPS, &[0, 1])];
    let refs = [FlowRef {
        panco_tfa_us: 41.1872,
        panco_exact_us: 27.0,
    }];
    check_fixture("single_flow_tandem", &flows, &services, &refs);
}

/// fixture 3 — `three_server_line`: 3 servers `(1 Gbps, 10 µs)`; tagged
/// `[0→1→2]`, cross1 `[0→1]`, cross2 `[1→2]`. A non-trivial topology where
/// the bound is not hand-checkable — the oracle earns its keep here.
#[test]
fn panco_three_server_line_sound_and_agrees() {
    let services = vec![
        ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ServiceCurve::rate_latency(GBPS, 10 * US_PS),
    ];
    let flows = vec![
        affine_flow(FRAME, HUNDRED_MBPS, &[0, 1, 2]),
        affine_flow(FRAME, HUNDRED_MBPS, &[0, 1]),
        affine_flow(FRAME, HUNDRED_MBPS, &[1, 2]),
    ];
    let refs = [
        FlowRef {
            panco_tfa_us: 134.7037,
            panco_exact_us: 68.4,
        }, // tagged
        FlowRef {
            panco_tfa_us: 86.7784,
            panco_exact_us: 58.4,
        }, // cross1
        FlowRef {
            panco_tfa_us: 100.7037,
            panco_exact_us: 57.98,
        }, // cross2
    ];
    check_fixture("three_server_line", &flows, &services, &refs);
}
