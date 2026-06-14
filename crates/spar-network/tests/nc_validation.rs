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

// ─── PLP cross-validation (REQ-NC-PLP-001) ──────────────────────────────
//
// The PLP engine lives behind the `milp-solver` feature (HiGHS/good_lp),
// like `pmoo`. These checks pin spar's `plp_bound` against panco's **pure**
// polynomial LP (`FifoLP(polynomial=True, tfa=False)`) on the same fixtures.
//
// Two checks per flow, but with a crucial difference from the TFA block
// above. spar's TFA is an *independent* reimplementation, so agreement with
// panco's TFA is method-validation. spar's PLP is *transcribed* from panco's
// LP and the exact floor is *also* panco's, so the agreement check here is
// **port-fidelity** (a solver-precision regression pin), not independent
// soundness — that is the deferred simulation floor REQ-NC-SIM-FLOOR-001.
// What stays independent is the EXACT soundness floor (`plp ≥ panco exact`),
// where panco's exact ELP is the true worst case. We ALSO assert `plp ≤ TFA`
// here, but note that is a *per-fixture* observation on these benign nets,
// NOT a universal law: pure polynomial PLP and TFA are incomparable
// over-approximations (neither dominates), and under extreme cross-burst PLP
// can exceed TFA — see the `network_wide_tfa_index_aligns_on_converging_tree`
// counterexample in spar-analysis, where panco's own PLP > panco's own TFA.
#[cfg(feature = "milp-solver")]
mod plp_xval {
    use super::*;
    use spar_network::{PlpFlow, plp_bound, tfa_bound};

    /// Port-fidelity tolerance: HiGHS must reproduce panco's lp_solve PLP
    /// optimum to solver precision. The pins are 2-decimal *truncations* of
    /// repeating decimals (72.22 ≈ 650/9, 61.11 ≈ 550/9 µs), so 0.05 % must
    /// absorb the pin's own ≤0.005 µs truncation error (~7e-5 rel at 70 µs)
    /// on top of f64/HiGHS tolerance — yet it is still ~7 ns at 72 µs, tight
    /// enough to fail on any real formulation drift.
    const PLP_TOL: f64 = 0.0005;

    fn plp_flow(burst: u64, rate: u64, path: &[usize]) -> PlpFlow {
        PlpFlow {
            alpha: ArrivalCurve::affine(burst, rate),
            path: path.to_vec(),
        }
    }

    /// For one fixture: spar PLP must (1) be `≥` panco EXACT (the independent
    /// soundness floor), (2) match the pinned pure-PLP value within `PLP_TOL`,
    /// and (3) be `≤` spar's own TFA bound. Check (3) is a *per-fixture* pin —
    /// PLP ≤ TFA holds on these benign nets but is NOT universal (the two are
    /// incomparable; see the converging-tree counterexample), so it is a
    /// regression pin on these inputs, not a soundness law.
    fn check_plp(
        name: &str,
        flows: &[PlpFlow],
        services: &[ServiceCurve],
        panco_plp_us: &[f64],
        panco_exact_us: &[f64],
    ) {
        let r =
            plp_bound(flows, services).unwrap_or_else(|e| panic!("{name}: plp_bound failed: {e}"));
        // spar's own TFA on the same fixture provides the sandwich ceiling.
        let tfa_flows: Vec<TfaFlow> = flows
            .iter()
            .map(|f| affine_flow(f.alpha.burst_bytes, f.alpha.sustained_rate_bps, &f.path))
            .collect();
        let tfa = tfa_bound(&tfa_flows, services).expect("tfa ceiling");

        assert_eq!(
            r.flow_delay_ps.len(),
            panco_plp_us.len(),
            "{name}: flow count"
        );
        for (i, &plp_ps) in r.flow_delay_ps.iter().enumerate() {
            let exact_ps = (panco_exact_us[i] * US_PS as f64).round() as u64;
            // (1) SOUNDNESS floor — independent of the port: EXACT ≤ plp.
            assert!(
                plp_ps >= exact_ps,
                "{name} flow {i}: UNSOUND — plp {plp_ps} ps < panco exact {exact_ps} ps",
            );
            // (2) PORT-FIDELITY — match panco's pure PLP to solver precision.
            let plp_us = plp_ps as f64 / US_PS as f64;
            let rel = (plp_us - panco_plp_us[i]).abs() / panco_plp_us[i];
            assert!(
                rel <= PLP_TOL,
                "{name} flow {i}: spar PLP {plp_us:.4} µs disagrees with panco PLP {:.4} µs \
                 by {:.3}% (> {:.3}% tol)",
                panco_plp_us[i],
                rel * 100.0,
                PLP_TOL * 100.0,
            );
            // (3) PER-FIXTURE pin — plp ≤ spar TFA on these benign nets. NOT a
            // universal law: PLP and TFA are incomparable (see the converging-
            // tree counterexample); this catches regressions on these inputs.
            assert!(
                plp_ps <= tfa.flow_delay_ps[i],
                "{name} flow {i}: plp {plp_ps} ps exceeds spar TFA {} ps \
                 (per-fixture PLP≤TFA pin broke)",
                tfa.flow_delay_ps[i],
            );
        }
    }

    #[test]
    fn panco_tandem_cross_plp() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 5 * US_PS),
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            plp_flow(FRAME, HUNDRED_MBPS, &[0]),
        ];
        // Pure PLP = EXACT on this tandem.
        check_plp(
            "tandem_cross",
            &flows,
            &services,
            &[39.0, 34.0],
            &[39.0, 34.0],
        );
    }

    #[test]
    fn panco_single_flow_tandem_plp() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 5 * US_PS),
        ];
        let flows = vec![plp_flow(FRAME, HUNDRED_MBPS, &[0, 1])];
        check_plp("single_flow_tandem", &flows, &services, &[27.0], &[27.0]);
    }

    #[test]
    fn panco_three_server_line_plp() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 1, 2]),
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            plp_flow(FRAME, HUNDRED_MBPS, &[1, 2]),
        ];
        // Pure PLP `[72.22, 61.11, 57.98]` — looser than EXACT `[68.4, 58.4,
        // 57.98]` but, on this benign net, strictly below TFA `[134.7, 86.8,
        // 100.7]` (PLP ≤ TFA is per-fixture here, not universal).
        check_plp(
            "three_server_line",
            &flows,
            &services,
            &[72.22, 61.11, 57.98],
            &[68.4, 58.4, 57.98],
        );
    }

    // ── Converging sink-trees (REQ-NC-PLP-CONVERGE-001) ───────────────────
    // The many-talkers→one-listener TSN shape. Each fixture is handed in
    // NON-topological order (sink at index 0, talkers above it) so the path
    // edges all *descend* — exercising `plp_bound`'s topological relabel. The
    // pinned panco values were regenerated fresh from the offline panco venv
    // (pure FifoLP(polynomial=True, tfa=False), lp_solve backend). Because the
    // bound is invariant under the relabel, these equal what panco reports for
    // the same physical net in topological order. The PLP ≪ TFA gap here is the
    // converging-tree dominance LUDB/PMOO cannot show.

    /// 2 talkers → 1 sink (1 hop each). panco: PLP 48.89 ≪ TFA 60.39 µs.
    #[test]
    fn panco_converge_2to1_plp() {
        // sink = 0; talkers = 1, 2. Edges 1→0 and 2→0 both descend.
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // sink = 0
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker A = 1
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker B = 2
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[1, 0]),
            plp_flow(FRAME, HUNDRED_MBPS, &[2, 0]),
        ];
        check_plp(
            "converge_2to1",
            &flows,
            &services,
            &[48.89, 48.89],
            &[46.2, 46.2],
        );
    }

    /// 2 talkers → shared tail switch → 1 sink (2 hops each). The 1.77× wedge:
    /// panco PLP 60.0 ≪ TFA 106.44 µs.
    #[test]
    fn panco_converge_2to1_tail2_plp() {
        // sink = 0; shared tail = 1; talkers = 2, 3. Paths 2→1→0 and 3→1→0.
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // sink = 0
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // shared tail = 1
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker A = 2
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker B = 3
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[2, 1, 0]),
            plp_flow(FRAME, HUNDRED_MBPS, &[3, 1, 0]),
        ];
        check_plp(
            "converge_2to1_tail2",
            &flows,
            &services,
            &[60.0, 60.0],
            &[56.2, 56.2],
        );
    }

    /// 3 talkers → 1 sink (1 hop each). panco: PLP 70.0 ≪ TFA 74.59 µs.
    #[test]
    fn panco_converge_3to1_plp() {
        // sink = 0; talkers = 1, 2, 3. Edges 1→0, 2→0, 3→0 all descend.
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // sink = 0
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker A = 1
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker B = 2
            ServiceCurve::rate_latency(GBPS, 10 * US_PS), // talker C = 3
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[1, 0]),
            plp_flow(FRAME, HUNDRED_MBPS, &[2, 0]),
            plp_flow(FRAME, HUNDRED_MBPS, &[3, 0]),
        ];
        check_plp(
            "converge_3to1",
            &flows,
            &services,
            &[70.0, 70.0, 70.0],
            &[60.4, 60.4, 60.4],
        );
    }
}

// ─── TFA-strengthened PLP (REQ-NC-PLP-003) ──────────────────────────────────
//
// `plp_bound_tfa_strengthened` adds panco's `tfa=True` per-server TFA-delay
// constraints to the polynomial LP (Bouillard `fifo/plpConstraints.py`),
// tightening the bound toward EXACT. The LP is still spar's port of panco's
// (as in the pure-PLP block), but here it is fed spar's OWN independent FP-TFA
// per-server delays (`tfa_bound`) rather than panco's, so reproducing panco's
// `tfa=True` column is a wiring-regression pin — agreement implied by
// (LP port-fidelity, the `plp_xval` block) ∧ (spar TFA ≈ panco TFA, the TFA
// block) — not a wholly independent recomputation. Soundness is preserved
// because spar's per-server delays are themselves sound upper bounds (≥ true
// delay), and the one-directional `t[h]−t[j] ≤ d[j]` constraints stay valid
// under any looser `d[j]`.
//
// The oracle pins the panco `tfa=True` column (regenerated offline from the
// venv, `FifoLP(polynomial=True, tfa=True)`), and asserts the three guarantees
// that matter:
//   (1) SOUNDNESS  — strengthened ≥ panco EXACT (independent ELP floor).
//   (2) AGREEMENT  — strengthened ≈ panco `tfa=True` within method tolerance.
//   (3) MONOTONE   — strengthened ≤ pure PLP (strengthening can only tighten).
// `converge_bridge` additionally asserts the headline: strengthened dataA ≤
// TFA, on the very net where *pure* PLP exceeds TFA (1148.33 > 1048.09). The
// strengthening drives it to EXACT (1035.2), restoring `plp ≤ TFA` — the whole
// point of REQ-NC-PLP-003.
#[cfg(feature = "milp-solver")]
mod plp_str_xval {
    use super::*;
    use spar_network::{PlpFlow, plp_bound, plp_bound_tfa_strengthened, tfa_bound};

    /// Method-agreement tolerance for the strengthened LP. spar reproduces
    /// panco's `tfa=True` column to the pinned 2 decimals; the slack absorbs
    /// per-server ps-rounding in both spar's TFA delays and the LP (the same
    /// ≤0.056 % regime as the TFA block), while still failing on any real
    /// formulation drift (~200 ps at 68 µs).
    const PLP_STR_TOL: f64 = 0.003;

    fn plp_flow(burst: u64, rate: u64, path: &[usize]) -> PlpFlow {
        PlpFlow {
            alpha: ArrivalCurve::affine(burst, rate),
            path: path.to_vec(),
        }
    }

    /// For one fixture: the strengthened bound must (1) be `≥` panco EXACT,
    /// (2) match the pinned panco `tfa=True` column within `PLP_STR_TOL`, and
    /// (3) be `≤` spar's own pure PLP on the same fixture (strengthening never
    /// loosens). Returns spar's strengthened per-flow delays (ps) so callers
    /// can pin extra net-specific invariants.
    fn check_plp_str(
        name: &str,
        flows: &[PlpFlow],
        services: &[ServiceCurve],
        panco_tfa_true_us: &[f64],
        panco_exact_us: &[f64],
    ) -> Vec<u64> {
        let r = plp_bound_tfa_strengthened(flows, services)
            .unwrap_or_else(|e| panic!("{name}: plp_bound_tfa_strengthened failed: {e}"));
        let pure = plp_bound(flows, services)
            .unwrap_or_else(|e| panic!("{name}: pure plp_bound failed: {e}"));

        assert_eq!(
            r.flow_delay_ps.len(),
            panco_tfa_true_us.len(),
            "{name}: flow count"
        );
        for (i, &str_ps) in r.flow_delay_ps.iter().enumerate() {
            let exact_ps = (panco_exact_us[i] * US_PS as f64).round() as u64;
            // (1) SOUNDNESS floor — strengthening must not undercut EXACT.
            assert!(
                str_ps >= exact_ps,
                "{name} flow {i}: UNSOUND — strengthened {str_ps} ps < panco exact {exact_ps} ps",
            );
            // (2) AGREEMENT — track panco's tfa=True column (method-level).
            let str_us = str_ps as f64 / US_PS as f64;
            let rel = (str_us - panco_tfa_true_us[i]).abs() / panco_tfa_true_us[i];
            assert!(
                rel <= PLP_STR_TOL,
                "{name} flow {i}: strengthened {str_us:.4} µs disagrees with panco tfa=True \
                 {:.4} µs by {:.3}% (> {:.3}% tol)",
                panco_tfa_true_us[i],
                rel * 100.0,
                PLP_STR_TOL * 100.0,
            );
            // (3) MONOTONE — strengthened ≤ pure PLP (only tightens).
            assert!(
                str_ps <= pure.flow_delay_ps[i],
                "{name} flow {i}: strengthened {str_ps} ps EXCEEDS pure PLP {} ps \
                 (strengthening must never loosen)",
                pure.flow_delay_ps[i],
            );
        }
        r.flow_delay_ps
    }

    /// `three_server_line` — strengthening drives the two crossing flows from
    /// pure PLP `[72.22, 61.11, 57.98]` down to EXACT `[68.4, 58.4, 57.98]`
    /// (= panco `tfa=True`).
    #[test]
    fn panco_three_server_line_plp_str() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
            ServiceCurve::rate_latency(GBPS, 10 * US_PS),
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 1, 2]),
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 1]),
            plp_flow(FRAME, HUNDRED_MBPS, &[1, 2]),
        ];
        check_plp_str(
            "three_server_line",
            &flows,
            &services,
            &[68.4, 58.4, 57.98],
            &[68.4, 58.4, 57.98],
        );
    }

    /// `converge_bridge` (wctt.rs::bridge_converge_aadl) — THE net that breaks
    /// pure PLP. Two talkers merge at a shared sink: dataA `[0,2]` (fast→fast),
    /// dataB `[1,2]` (slow→fast), sink = 2. Server latencies 5 µs / 10000 µs /
    /// 5 µs. Pure PLP gives dataA 1148.33 µs > TFA 1048.09 µs (both sound, but
    /// incomparable). Strengthening drives dataA to EXACT 1035.2 µs, *below*
    /// TFA — the REQ-NC-PLP-003 fix. Pinned panco `tfa=True` = `[1035.2,
    /// 10030.7]` = EXACT.
    #[test]
    fn panco_converge_bridge_plp_str() {
        let services = vec![
            ServiceCurve::rate_latency(GBPS, 5 * US_PS), // sw_a fast = 0
            ServiceCurve::rate_latency(GBPS, 10_000 * US_PS), // sw_b slow = 1
            ServiceCurve::rate_latency(GBPS, 5 * US_PS), // sw_c fast sink = 2
        ];
        let flows = vec![
            plp_flow(FRAME, HUNDRED_MBPS, &[0, 2]), // dataA
            plp_flow(FRAME, HUNDRED_MBPS, &[1, 2]), // dataB
        ];
        let str_ps = check_plp_str(
            "converge_bridge",
            &flows,
            &services,
            &[1035.2, 10030.7],
            &[1035.2, 10030.7],
        );

        // Headline: strengthened dataA ≤ TFA dataA, where *pure* PLP exceeds
        // it. This is the incomparability that REQ-NC-PLP-003 repairs.
        let tfa_flows: Vec<TfaFlow> = flows
            .iter()
            .map(|f| affine_flow(f.alpha.burst_bytes, f.alpha.sustained_rate_bps, &f.path))
            .collect();
        let tfa = tfa_bound(&tfa_flows, &services).expect("tfa ceiling");
        let pure = plp_bound(&flows, &services).expect("pure plp");
        assert!(
            pure.flow_delay_ps[0] > tfa.flow_delay_ps[0],
            "converge_bridge: precondition broke — pure PLP dataA {} ps should EXCEED \
             TFA {} ps (the pure-PLP incomparability this fix targets)",
            pure.flow_delay_ps[0],
            tfa.flow_delay_ps[0],
        );
        assert!(
            str_ps[0] <= tfa.flow_delay_ps[0],
            "converge_bridge: strengthened dataA {} ps must be ≤ TFA {} ps \
             (REQ-NC-PLP-003 restores plp ≤ TFA where pure PLP could not)",
            str_ps[0],
            tfa.flow_delay_ps[0],
        );
    }
}
