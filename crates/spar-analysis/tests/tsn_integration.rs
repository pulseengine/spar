//! Phase 2 TSN end-to-end integration test.
//!
//! Exercises the four-way TSN dispatch in
//! [`spar_analysis::wctt::WcttAnalysis`] introduced across v0.8.1
//! commits 2-4 (PR #180 TAS, #182 CBS, #181 Frame Preemption). A single
//! `SystemInstance` is constructed with three TSN-typed buses — one per
//! shaping path — wrapped as three sub-systems of a root system. Each
//! sub-system carries its own `Actual_Connection_Binding` so the
//! dispatch consistently picks one shaping arm per stream:
//!
//! - **TAS (802.1Qbv)**: bus carries `Spar_TSN::Gate_Control_List`; the
//!   stream declares `Spar_TSN::Class_of_Service => 7`. Dispatch's
//!   first arm matches `(GCL_on_bus, stream.cos)` and routes here.
//!   Expect one `WcttTasGated` Info diagnostic carrying the open
//!   fraction (50% for the GCL `"0:5000:0x80;5000:5000:0x7F"`) and the
//!   worst-case gate latency (5 000 000 ps = 5 us — the GCL offsets
//!   are in nanoseconds per the c2 parser, hence ps when reported).
//! - **CBS (802.1Qav)**: bus is plain TSN (no GCL); the stream
//!   declares `Spar_TSN::Bandwidth_Reservation` plus a CoS so the
//!   dispatch falls past TAS (no GCL on this bus) into the CBS arm.
//!   Expect one `WcttCbsShaped` Info diagnostic carrying
//!   `idle_slope=300000000 bps`.
//! - **Frame preemption (802.1Qbu)**: bus declares only
//!   `Spar_TSN::Frame_Preemption => true` (no GCL); the stream
//!   declares `Spar_TSN::Class_of_Service => 7`, which makes it
//!   express via [`is_express_stream`]'s default heuristic. Both prior
//!   arms fail (no GCL on this bus, no Bandwidth_Reservation on the
//!   stream); the preemption arm matches because the bus has
//!   Frame_Preemption=true and the stream is express. Expect one
//!   `WcttPreemptionApplied` Info diagnostic carrying both the legacy
//!   max-frame blocking (1518 B · 8 / 100 Mbps = 121 440 ns) and the
//!   802.1Qbu fragment blocking (68 B · 8 / 100 Mbps = 5 440 ns).
//!
//! The test asserts that all three diagnostics fire in the same
//! `analyze` run, that no `WcttDeferred` slips through (which would
//! indicate the dispatch arms aren't reached), and that each diagnostic
//! carries plausible numeric values.
//!
//! Per the v0.8.1 c5 close-out scope: same `SystemInstance`, same
//! `WcttAnalysis::analyze` call, all three Phase-2 diagnostics
//! co-existing in one run. Multi-stream sharing of a CBS class and
//! advanced TAS guards stay deferred to v0.8.x follow-ups.
//!
//! Traceability (Rivet): REQ-TSN-001 (TAS), REQ-TSN-002 (preemption),
//! REQ-TSN-003 (CBS); TEST-TSN-DISPATCH (this file).

use spar_analysis::{Analysis, AnalysisDiagnostic, Severity, wctt::WcttAnalysis};
use spar_hir_def::{
    HirDefDatabase, Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope,
};

/// AADL fixture exercising all three Phase-2 TSN dispatch paths in one
/// system instance. The root `Sys.impl` has three child sub-systems
/// (`tas_seg`, `cbs_seg`, `pmt_seg`), each containing its own bus +
/// source + sink + data stream. The sub-system isolation means each
/// stream's `Actual_Connection_Binding` resolves to exactly one bus —
/// the one inside its parent sub-system — so the dispatch arm picked
/// in `wctt.rs` is deterministic per stream.
const TSN_TRIPLE_DISPATCH_AADL: &str = r#"
package TsnTriple
public

  -- ── TAS bus: carries a GCL, opens 50% of the cycle for CoS 7. ─────
  bus eth_tas
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Gate_Control_List      => "0:5000:0x80;5000:5000:0x7F";
  end eth_tas;
  bus implementation eth_tas.impl
  end eth_tas.impl;

  -- ── CBS bus: plain TSN (no GCL, no preemption). ───────────────────
  bus eth_cbs
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth_cbs;
  bus implementation eth_cbs.impl
  end eth_cbs.impl;

  -- ── Preemption bus: 100 Mbps + Frame_Preemption => true. No GCL,
  --    so high-CoS streams skip the TAS arm and land on preemption. ──
  bus eth_pmt
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 100000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Frame_Preemption       => true;
  end eth_pmt;
  bus implementation eth_pmt.impl
  end eth_pmt.impl;

  -- ── Plain sink device: no TSN annotations, just receives. ─────────
  device d_sink
    features
      net   : requires bus access;
      in_p  : in data port;
  end d_sink;
  device implementation d_sink.impl
  end d_sink.impl;

  -- ── TAS source: declares CoS=7 only — bus's GCL drives the gate. ─
  device d_tas_src
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end d_tas_src;
  device implementation d_tas_src.impl
  end d_tas_src.impl;

  -- ── CBS source: declares Bandwidth_Reservation + CoS=5. The CBS
  --    bus has no GCL so the TAS arm cannot fire even with CoS set;
  --    the CBS arm matches on the idle slope. CoS=5 keeps the source
  --    out of the express set (so preemption dispatch is not a risk
  --    even if a future commit reorders the arms). ───────────────────
  device d_cbs_src
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service        => 5;
      Spar_TSN::Bandwidth_Reservation   => 300000000 bitsps;
      Spar_Network::Queue_Depth         => 1;
  end d_cbs_src;
  device implementation d_cbs_src.impl
  end d_cbs_src.impl;

  -- ── Preemption-express source: high CoS so it counts as express
  --    by the default heuristic. No CBS reservation, no GCL on its
  --    bus → falls to the preemption arm. ─────────────────────────────
  device d_pmt_src
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end d_pmt_src;
  device implementation d_pmt_src.impl
  end d_pmt_src.impl;

  -- ── Per-arm sub-system. Each sub-system contains its own bus, its
  --    own pair of endpoints, its own stream, and its own
  --    Actual_Connection_Binding. Because the binding is declared at
  --    the sub-system scope (i.e. the `system implementation` whose
  --    `connections` list owns the stream), `collect_streams` picks
  --    up exactly that bus for that stream. ────────────────────────
  system TasSeg
  end TasSeg;
  system implementation TasSeg.impl
    subcomponents
      sw_tas : bus eth_tas.impl;
      src    : device d_tas_src.impl;
      dst    : device d_sink.impl;
    connections
      c_sw_src : bus access sw_tas -> src.net;
      c_sw_dst : bus access sw_tas -> dst.net;
      data_tas : port src.out_p -> dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw_tas));
  end TasSeg.impl;

  system CbsSeg
  end CbsSeg;
  system implementation CbsSeg.impl
    subcomponents
      sw_cbs : bus eth_cbs.impl;
      src    : device d_cbs_src.impl;
      dst    : device d_sink.impl;
    connections
      c_sw_src : bus access sw_cbs -> src.net;
      c_sw_dst : bus access sw_cbs -> dst.net;
      data_cbs : port src.out_p -> dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw_cbs));
  end CbsSeg.impl;

  system PmtSeg
  end PmtSeg;
  system implementation PmtSeg.impl
    subcomponents
      sw_pmt : bus eth_pmt.impl;
      src    : device d_pmt_src.impl;
      dst    : device d_sink.impl;
    connections
      c_sw_src : bus access sw_pmt -> src.net;
      c_sw_dst : bus access sw_pmt -> dst.net;
      data_pmt : port src.out_p -> dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw_pmt));
  end PmtSeg.impl;

  -- Root system aggregates the three arm sub-systems.
  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      tas_seg : system TasSeg.impl;
      cbs_seg : system CbsSeg.impl;
      pmt_seg : system PmtSeg.impl;
  end Sys.impl;
end TsnTriple;
"#;

fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(
        &db,
        "tsn_integration.aadl".to_string(),
        aadl_src.to_string(),
    );
    let tree = file_item_tree(&db, file);
    let scope = GlobalScope::from_trees(vec![tree]);
    let inst = SystemInstance::instantiate(
        &scope,
        &Name::new(pkg),
        &Name::new(sys),
        &Name::new(sys_impl),
    );
    assert!(
        inst.component_count() > 0,
        "instantiation failed; diagnostics: {:?}",
        inst.diagnostics
    );
    inst
}

#[test]
fn phase2_dispatch_routes_each_stream_to_its_shaping_path() {
    let inst = instantiate(TSN_TRIPLE_DISPATCH_AADL, "TsnTriple", "Sys", "impl");
    let diags: Vec<AnalysisDiagnostic> = WcttAnalysis::default().analyze(&inst);

    let by_kind = |kind: &'static str| -> Vec<&AnalysisDiagnostic> {
        diags
            .iter()
            .filter(|d| d.message.starts_with(kind))
            .collect()
    };

    let tas = by_kind("WcttTasGated");
    let cbs = by_kind("WcttCbsShaped");
    let pmt = by_kind("WcttPreemptionApplied");
    let deferred = by_kind("WcttDeferred");

    // ── Co-existence: all three shaping diagnostics in the same run. ──
    assert_eq!(
        tas.len(),
        1,
        "expected exactly one WcttTasGated for the TAS-routed stream; \
         got {}: full diagnostics: {:#?}",
        tas.len(),
        diags
    );
    assert_eq!(
        cbs.len(),
        1,
        "expected exactly one WcttCbsShaped for the CBS-routed stream; \
         got {}: full diagnostics: {:#?}",
        cbs.len(),
        diags
    );
    assert_eq!(
        pmt.len(),
        1,
        "expected exactly one WcttPreemptionApplied for the express \
         stream on the preemption-capable port; got {}: full \
         diagnostics: {:#?}",
        pmt.len(),
        diags
    );

    // ── No WcttDeferred slipped through. If the dispatch routed a
    //    stream to the legacy Phase-1 deferral path, the dispatch is
    //    broken — surface it loudly. ───────────────────────────────────
    assert!(
        deferred.is_empty(),
        "no WcttDeferred should fire when every stream has a Phase-2 \
         shaping path: {:#?}",
        deferred
    );

    // ── Severity is Info for all three (they describe successful
    //    bound computation, not a problem). ───────────────────────────
    assert!(
        tas[0].severity == Severity::Info,
        "WcttTasGated must be Info: {:?}",
        tas[0]
    );
    assert!(
        cbs[0].severity == Severity::Info,
        "WcttCbsShaped must be Info: {:?}",
        cbs[0]
    );
    assert!(
        pmt[0].severity == Severity::Info,
        "WcttPreemptionApplied must be Info: {:?}",
        pmt[0]
    );

    // ── Plausible numeric content per arm. ───────────────────────────

    // TAS: GCL is "0:5000:0x80;5000:5000:0x7F" — 5 us (5 000 ns)
    //      window for CoS 7 followed by 5 us window for CoS 0..6.
    //      Open fraction for CoS 7 is 5 000 / 10 000 = 50%. The c2
    //      parser stores GCL offsets/durations in picoseconds (5 us
    //      = 5 000 000 ps), so the worst-case gate latency reported
    //      in the diagnostic is `5000000 ps`.
    let tas_msg = &tas[0].message;
    assert!(
        tas_msg.contains("data_tas"),
        "WcttTasGated must reference the TAS-routed connection: {}",
        tas_msg
    );
    assert!(
        tas_msg.contains("CoS 7"),
        "WcttTasGated must report stream's CoS: {}",
        tas_msg
    );
    assert!(
        tas_msg.contains("50%"),
        "WcttTasGated must report 50% open fraction (5 us / 10 us): {}",
        tas_msg
    );
    assert!(
        tas_msg.contains("gate latency 5000000 ps"),
        "WcttTasGated must report gate latency 5_000_000 ps (5 us): {}",
        tas_msg
    );

    // CBS: idle slope is 300 Mbps (declared on d_cbs_src). The
    //      WcttCbsShaped message reports `idle_slope=<bps> bps`.
    let cbs_msg = &cbs[0].message;
    assert!(
        cbs_msg.contains("data_cbs"),
        "WcttCbsShaped must reference the CBS-routed connection: {}",
        cbs_msg
    );
    assert!(
        cbs_msg.contains("idle_slope=300000000 bps"),
        "WcttCbsShaped must report idle slope 300 Mbps: {}",
        cbs_msg
    );
    assert!(
        cbs_msg.contains("cos=5"),
        "WcttCbsShaped must report stream's CoS: {}",
        cbs_msg
    );
    // The CBS service-curve latency is reported in nanoseconds; it
    // captures other-class blocking by the 1518 B max-competing
    // frame. Must be a positive number with the `ns` suffix.
    assert!(
        cbs_msg.contains("service_latency=") && cbs_msg.contains(" ns"),
        "WcttCbsShaped must report service latency in ns: {}",
        cbs_msg
    );

    // Preemption: the eth_pmt bus is 100 Mbps. Legacy max-frame
    //             blocking is 1518 B · 8 bits / 100_000_000 bps =
    //             121 440 ns. With 802.1Qbu the preemption-fragment
    //             term is 68 B · 8 / 100 Mbps = 5 440 ns (the 64 B
    //             minimum frame plus the 4 B mPacket header).
    let pmt_msg = &pmt[0].message;
    assert!(
        pmt_msg.contains("data_pmt"),
        "WcttPreemptionApplied must reference the preemption-routed \
         connection: {}",
        pmt_msg
    );
    assert!(
        pmt_msg.contains("121440 ns"),
        "WcttPreemptionApplied must report legacy blocking 121440 ns: {}",
        pmt_msg
    );
    assert!(
        pmt_msg.contains("5440 ns"),
        "WcttPreemptionApplied must report fragment blocking 5440 ns: {}",
        pmt_msg
    );

    // ── Each stream produces a finite WcttBound. ────────────────────
    let bounds = by_kind("WcttBound");
    assert_eq!(
        bounds.len(),
        3,
        "expected one WcttBound per stream (3 streams): {:#?}",
        bounds
    );
    for b in &bounds {
        assert!(b.severity == Severity::Info);
        assert!(
            b.message.contains(" ps "),
            "WcttBound must carry a picosecond bound: {}",
            b.message
        );
    }
}
