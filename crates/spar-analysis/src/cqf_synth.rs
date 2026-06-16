//! AADL→CQF synthesis bridge — REQ-TSN-SYNTH-CQF-BRIDGE-001.
//!
//! Extracts CQF synthesis inputs directly from an AADL [`SystemInstance`]
//! and runs the literature-pinned synthesizer ([`synthesize_cqf`],
//! REQ-TSN-SYNTH-CQF-BASE-001) over them, so a real model — not a
//! hand-built [`CqfFlow`] set — drives standard 802.1Qch CQF
//! configuration. The synthesized schedule (or a synthesis error) is
//! surfaced as analysis diagnostics.
//!
//! # Scope — table-stakes, NOT the wedge
//!
//! This is AADL→TSN-config *glue*: the demonstrable end-to-end ship that
//! makes spar a configuration generator a user can run on their own
//! model. It composes **no** network calculus and is explicitly NOT the
//! PLP≪TFA strategic wedge. CQF's bounded delay depends only on
//! `(hops, cycle time)` (IETF draft-eckert-detnet-tcqf-05), so the bridge
//! needs the routing topology and a few per-flow scalars — nothing from
//! the NC engine, no per-class composition soundness obligation.
//!
//! # Extraction
//!
//! The routing walk is **reused** from the WCTT pass
//! ([`collect_streams`](crate::wctt::collect_streams)) rather than
//! reimplemented: a stream is a non-bus-access connection whose endpoints
//! are both end stations and which carries `Actual_Connection_Binding` to
//! switched buses. From each stream the bridge derives a [`CqfFlow`]:
//!
//! - **`reserved_bits_per_cycle`** — `Spar_TSN::Max_Frame_Size` on the
//!   source × 8 (one frame buffered per cycle), defaulting to one
//!   Ethernet MTU ([`DEFAULT_FRAME_BYTES`]).
//! - **`deadline_ps`** — `Timing_Properties::Deadline` on the source
//!   (no fallback to `Period`). A stream with no `Deadline` cannot
//!   constrain the cycle time and is skipped with an Info diagnostic
//!   rather than given an invented deadline.
//! - **`path`** — the bound switch sequence, each switch's arena index
//!   reused as the link id, so streams sharing a switch sum their
//!   per-cycle reservations there; `path.len()` is the hop count H.
//!
//! The homogeneous link rate handed to the synthesizer is the **minimum**
//! `Spar_Network::Output_Rate` across the switches — conservative for the
//! single-`csize` baseline (the slowest link has the smallest cycle
//! budget `csize = T·link_rate`, so it binds admission).
//!
//! # Soundness guard
//!
//! `reserved_bits_per_cycle` models one frame per cycle, sound only when
//! the synthesized cycle time T does not exceed a flow's period: if
//! T > period, more than one frame can arrive per cycle and a one-frame
//! reservation under-counts. The deadline-driven choice
//! T = min(deadline/(H+1)) keeps T ≤ deadline ≤ period in the normal
//! case, but `Deadline > Period` is AADL-legal (if unusual); the bridge
//! checks T ≤ period for every flow post-synthesis and emits a
//! `CqfSynthUnsound` warning rather than silently under-reserving.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

use spar_network::extract::{extract_network_graph, read_output_rate_bps};
use spar_network::tsn::get_max_frame_size_bytes;
use spar_network::{CqfFlow, synthesize_cqf};

use crate::property_accessors::get_timing_property;
use crate::wctt::collect_streams;
use crate::{Analysis, AnalysisDiagnostic, Severity};

/// Per-cycle frame size (bytes) assumed when the source declares no
/// `Spar_TSN::Max_Frame_Size` — one standard Ethernet MTU.
pub const DEFAULT_FRAME_BYTES: u64 = 1500;

const ANALYSIS_NAME: &str = "cqf_synth";

/// AADL→CQF synthesis bridge analysis pass (REQ-TSN-SYNTH-CQF-BRIDGE-001).
#[derive(Debug, Default)]
pub struct CqfSynthAnalysis;

impl Analysis for CqfSynthAnalysis {
    fn name(&self) -> &str {
        ANALYSIS_NAME
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        synth_cqf(instance)
    }
}

/// Structured CQF synthesis inputs extracted from a [`SystemInstance`].
///
/// Exposed `pub(crate)` so the oracle test can assert on the typed
/// [`CqfFlow`]s and feed [`synthesize_cqf`] directly — pinning the exact
/// [`CqfSchedule`](spar_network::CqfSchedule) rather than a formatted
/// diagnostic string.
#[derive(Debug, Default)]
pub(crate) struct CqfInputs {
    /// One flow per admissible stream (those declaring a deadline).
    pub(crate) flows: Vec<CqfFlow>,
    /// Parallel to `flows`: `(connection name, period_ps)` for the
    /// soundness guard and diagnostics.
    pub(crate) labels: Vec<(String, Option<u64>)>,
    /// Streams discovered but excluded for want of a deadline.
    pub(crate) skipped: Vec<String>,
    /// Conservative homogeneous link rate (min switch `Output_Rate`);
    /// `None` when no switch declares a usable rate.
    pub(crate) link_rate_bps: Option<u64>,
}

/// Walk the model and build the CQF synthesis inputs. Returns an empty
/// [`CqfInputs`] (no flows, `link_rate_bps = None`) for models with no
/// switched buses or no bound streams — the non-regression contract,
/// mirroring the WCTT pass.
pub(crate) fn collect_cqf_inputs(instance: &SystemInstance) -> CqfInputs {
    let mut out = CqfInputs::default();

    let graph = extract_network_graph(instance);
    if graph.switches().count() == 0 {
        return out;
    }

    // Switch name (lower-case) → idx, and the minimum switch link rate
    // (the conservative homogeneous rate for the single-csize baseline).
    let mut bus_by_name: FxHashMap<String, ComponentInstanceIdx> = FxHashMap::default();
    let mut min_link_rate_bps: Option<u64> = None;
    for node in graph.switches() {
        bus_by_name.insert(node.name.to_ascii_lowercase(), node.idx);
        let rate = read_output_rate_bps(instance.properties_for(node.idx)).unwrap_or(0);
        if rate > 0 {
            min_link_rate_bps = Some(min_link_rate_bps.map_or(rate, |m| m.min(rate)));
        }
    }
    out.link_rate_bps = min_link_rate_bps;

    // Reuse the WCTT routing resolution to discover end-to-end streams.
    let streams = collect_streams(instance, &graph, &bus_by_name);

    for s in &streams {
        let src_props = instance.properties_for(s.src_idx);

        // End-to-end deadline from Timing_Properties::Deadline (no Period
        // fallback). No Deadline ⇒ the flow can't constrain the cycle
        // time; record it as skipped rather than invent one.
        let Some(deadline_ps) = get_timing_property(src_props, "Deadline") else {
            out.skipped.push(s.name.clone());
            continue;
        };

        let frame_bytes = get_max_frame_size_bytes(src_props).unwrap_or(DEFAULT_FRAME_BYTES);
        let reserved_bits_per_cycle = frame_bytes.saturating_mul(8);

        let path: Vec<u32> = s.hops.iter().map(|h| h.into_raw().into_u32()).collect();

        let period_ps = get_timing_property(src_props, "Period");

        out.flows.push(CqfFlow {
            id: out.flows.len() as u32,
            reserved_bits_per_cycle,
            deadline_ps,
            path,
        });
        out.labels.push((s.name.clone(), period_ps));
    }

    out
}

/// Synthesize a CQF schedule from the model and render it (or any error /
/// soundness warning) as diagnostics.
fn synth_cqf(instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
    let mut diags = Vec::new();
    let inputs = collect_cqf_inputs(instance);

    for name in &inputs.skipped {
        diags.push(AnalysisDiagnostic {
            severity: Severity::Info,
            message: format!(
                "CqfSynthSkipped: stream '{name}' has no Timing_Properties::Deadline; \
                 excluded from CQF synthesis (no structural deadline to size the cycle)."
            ),
            path: vec![name.clone()],
            analysis: ANALYSIS_NAME.to_string(),
        });
    }

    // Need both a usable link rate and at least one constrained flow.
    let Some(link_rate_bps) = inputs.link_rate_bps else {
        return diags;
    };
    if inputs.flows.is_empty() {
        return diags;
    }

    match synthesize_cqf(&inputs.flows, link_rate_bps) {
        Ok(schedule) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "CqfSynthSchedule: synthesized 802.1Qch CQF over {} flow(s) at link rate \
                     {} bps — cycle time T = {} ns, cycle budget csize = {} bits, {} link(s) \
                     admitted.",
                    inputs.flows.len(),
                    link_rate_bps,
                    schedule.cycle_time_ps / 1000,
                    schedule.csize_bits,
                    schedule.per_link_bits.len(),
                ),
                path: vec![],
                analysis: ANALYSIS_NAME.to_string(),
            });

            // Soundness guard: T ≤ period for every flow (one frame per
            // cycle). T > period means a single per-cycle reservation can
            // under-count arrivals on the Deadline>Period edge case.
            for (name, period_ps) in &inputs.labels {
                if let Some(period_ps) = period_ps
                    && schedule.cycle_time_ps > *period_ps
                {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Warning,
                        message: format!(
                            "CqfSynthUnsound: flow '{}' has period {} ns < synthesized cycle \
                             time {} ns; the one-frame-per-cycle reservation may under-count \
                             arrivals (Deadline>Period). Tighten the deadline or model \
                             per-cycle bursts explicitly.",
                            name,
                            period_ps / 1000,
                            schedule.cycle_time_ps / 1000,
                        ),
                        path: vec![name.clone()],
                        analysis: ANALYSIS_NAME.to_string(),
                    });
                }
            }
        }
        Err(e) => {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Error,
                message: format!(
                    "CqfSynthError: CQF synthesis over {} flow(s) at link rate {} bps failed: {}",
                    inputs.flows.len(),
                    link_rate_bps,
                    e,
                ),
                path: vec![],
                analysis: ANALYSIS_NAME.to_string(),
            });
        }
    }

    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::Name;
    use spar_hir_def::{HirDefDatabase, file_item_tree, resolver::GlobalScope};
    use spar_network::{cqf_cycle_budget_bits, cqf_delay_max_ps};

    fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
        let db = HirDefDatabase::default();
        let file = spar_base_db::SourceFile::new(
            &db,
            "cqf_synth_test.aadl".to_string(),
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

    fn count_cqf(diags: &[AnalysisDiagnostic]) -> usize {
        diags
            .iter()
            .filter(|d| d.message.starts_with("CqfSynth"))
            .count()
    }

    // ── Oracle 1+2: end-to-end synthesis + extraction fidelity ───────
    //
    // Two source end stations, each bound (H=1) to one 1 Gbps FIFO
    // switch, declaring a 1000-byte Max_Frame_Size (NON-default, so a
    // silent parse failure that fell back to the 1500-byte default would
    // FAIL this test rather than pass it) and a 100 us deadline.
    //
    // Hand-computed schedule, pinned to draft-eckert-detnet-tcqf-05's
    // D_max = (H+1)·T:
    //   T      = floor(min deadline/(H+1)) = 100us / 2          = 50 us
    //   csize  = T·link_rate = 50e-6 s · 1e9 bps                = 50000 bits
    //   /link  = 2 flows · 1000 B · 8                           = 16000 bits  (<= csize)
    //   D_max  = (H+1)·T = 2·50us                               = 100 us = deadline
    #[test]
    fn bridge_single_switch_two_flows_matches_hand_computed_schedule() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
      c  : device src_d.impl;
      d  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      c_sw_c : bus access sw -> c.net;
      c_sw_d : bus access sw -> d.net;
      data1  : port a.out_p -> b.in_p;
      data2  : port c.out_p -> d.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);

        // Extraction fidelity: two flows, read (not defaulted) frame +
        // deadline, single-switch path, conservative link rate.
        assert_eq!(inputs.link_rate_bps, Some(1_000_000_000), "min switch rate");
        assert!(inputs.skipped.is_empty(), "no flow should be skipped");
        assert_eq!(inputs.flows.len(), 2, "two bound streams → two flows");
        for f in &inputs.flows {
            assert_eq!(
                f.reserved_bits_per_cycle, 8000,
                "1000 B Max_Frame_Size → 8000 bits (NOT the 1500 B default)"
            );
            assert_eq!(f.deadline_ps, 100_000_000, "100 us deadline in ps");
            assert_eq!(f.path.len(), 1, "single switch → H = 1");
        }
        // Both flows traverse the SAME switch ⇒ same link id.
        assert_eq!(
            inputs.flows[0].path, inputs.flows[1].path,
            "both flows share the one switch's link id"
        );

        // Now pin the exact synthesized schedule.
        let schedule = synthesize_cqf(&inputs.flows, inputs.link_rate_bps.unwrap())
            .expect("flows fit the cycle budget → admitted");
        assert_eq!(
            schedule.cycle_time_ps, 50_000_000,
            "T = deadline/(H+1) = 50 us"
        );
        assert_eq!(
            schedule.csize_bits,
            cqf_cycle_budget_bits(50_000_000, 1_000_000_000),
            "csize = T·link_rate consistency"
        );
        assert_eq!(schedule.csize_bits, 50_000, "50 us · 1 Gbps = 50000 bits");
        assert_eq!(schedule.per_link_bits.len(), 1, "one shared switch link");
        assert_eq!(
            schedule.per_link_bits.values().copied().sum::<u128>(),
            16_000,
            "2 flows · 1000 B · 8 = 16000 bits buffered per cycle"
        );
        // External ground-truth pin: D_max = (H+1)·T = deadline.
        assert_eq!(
            cqf_delay_max_ps(1, schedule.cycle_time_ps),
            100_000_000,
            "D_max = (H+1)·T meets the 100 us deadline exactly"
        );
    }

    // ── Oracle 2b: the Analysis pass surfaces the schedule ───────────
    #[test]
    fn bridge_pass_emits_schedule_diagnostic() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Info
                    && d.message.starts_with("CqfSynthSchedule:")),
            "expected a CqfSynthSchedule Info diagnostic, got {diags:#?}"
        );
    }

    // ── Oracle 3: min-rate — slowest switch on a 2-hop path binds ────
    #[test]
    fn bridge_uses_minimum_switch_rate_on_multi_hop_path() {
        let src = r#"
package Net
public

  bus eth_fast
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth_fast;
  bus implementation eth_fast.impl
  end eth_fast.impl;

  bus eth_slow
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 100000000 bitsps;
  end eth_slow;
  bus implementation eth_slow.impl
  end eth_slow.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw1 : bus eth_fast.impl;
      sw2 : bus eth_slow.impl;
      a   : device src_d.impl;
      b   : device snk_d.impl;
    connections
      c_sw1_a : bus access sw1 -> a.net;
      c_sw2_b : bus access sw2 -> b.net;
      data1   : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw1), reference (sw2));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);
        assert_eq!(
            inputs.link_rate_bps,
            Some(100_000_000),
            "min(1 Gbps, 100 Mbps) = 100 Mbps is the conservative rate"
        );
        assert_eq!(inputs.flows.len(), 1, "one bound stream");
        assert_eq!(inputs.flows[0].path.len(), 2, "two switches → H = 2");
        assert_ne!(
            inputs.flows[0].path[0], inputs.flows[0].path[1],
            "the two hops have distinct link ids"
        );
    }

    // ── Oracle 4: soundness guard — Deadline > Period ⇒ warning ──────
    //
    // Deadline 100 us drives T = 50 us, but the source period is 40 us,
    // so T > period: the one-frame-per-cycle reservation can under-count
    // and the bridge must warn rather than silently admit.
    #[test]
    fn bridge_warns_when_cycle_exceeds_period() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
      Timing_Properties::Period   => 40 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Warning
                    && d.message.starts_with("CqfSynthUnsound:")),
            "Deadline>Period (T=50us > period=40us) must emit CqfSynthUnsound, got {diags:#?}"
        );
    }

    // ── Oracle 5: non-regression — no switched buses, no diagnostics ─
    #[test]
    fn bridge_emits_nothing_without_switched_buses() {
        let src = r#"
package Plain
public

  bus plain_bus
  end plain_bus;
  bus implementation plain_bus.impl
  end plain_bus.impl;

  device d
    features
      net : requires bus access;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      pb : bus plain_bus.impl;
      x  : device d.impl;
      y  : device d.impl;
    connections
      c1 : bus access pb -> x.net;
      c2 : bus access pb -> y.net;
  end Sys.impl;
end Plain;
"#;
        let inst = instantiate(src, "Plain", "Sys", "impl");
        let diags = CqfSynthAnalysis.analyze(&inst);
        assert_eq!(
            count_cqf(&diags),
            0,
            "no Spar_Network::Switch_Type ⇒ zero CqfSynth* diagnostics, got {diags:#?}"
        );
    }

    // ── Oracle 6: skip branch — a bound flow with no timing budget ───
    //
    // The source declares a frame size but neither Deadline nor Period,
    // so there is no structural deadline to size the cycle. The stream is
    // still routed (so it reaches the skip branch rather than being lost),
    // recorded in `skipped`, and surfaced as a CqfSynthSkipped Info — never
    // silently dropped, never synthesized against an invented deadline.
    #[test]
    fn bridge_skips_bound_flow_without_timing_budget() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size => 1000;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);
        assert_eq!(
            inputs.skipped.len(),
            1,
            "the deadline-less bound stream is recorded as skipped, not synthesized"
        );
        assert!(
            inputs.flows.is_empty(),
            "no timing budget ⇒ no CqfFlow is constructed"
        );

        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Info && d.message.starts_with("CqfSynthSkipped:")),
            "expected a CqfSynthSkipped Info diagnostic, got {diags:#?}"
        );
        assert!(
            !diags
                .iter()
                .any(|d| d.message.starts_with("CqfSynthSchedule:")),
            "no flow remains ⇒ no schedule is synthesized, got {diags:#?}"
        );
    }

    // ── Oracle 7: error branch — deadline below the irreducible floor ─
    //
    // A 1 ns deadline over H=1 yields T_max = floor(1000ps / 2) = 0 ns:
    // even a one-nanosecond cycle violates the deadline, so synthesize_cqf
    // returns DeadlineTooTight. The bridge must surface that as a
    // CqfSynthError (Error), not swallow it or emit a bogus schedule.
    #[test]
    fn bridge_errors_when_deadline_below_cycle_floor() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 1 ns;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);
        assert_eq!(inputs.flows.len(), 1, "one bound stream with a deadline");
        assert!(
            synthesize_cqf(&inputs.flows, inputs.link_rate_bps.unwrap()).is_err(),
            "1 ns deadline over H=1 is below the irreducible cycle floor"
        );

        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.starts_with("CqfSynthError:")),
            "expected a CqfSynthError Error diagnostic, got {diags:#?}"
        );
    }

    // ── Oracle 8: no-rate branch — a switch without Output_Rate ──────
    //
    // The bus is a switch (Switch_Type set) but declares no Output_Rate,
    // so there is no link rate to size the cycle budget. The flow is still
    // constrained (it has a deadline), but synthesis can't proceed: the
    // bridge returns early with no schedule rather than defaulting a rate.
    #[test]
    fn bridge_returns_no_schedule_when_switch_lacks_output_rate() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);
        assert_eq!(
            inputs.link_rate_bps, None,
            "a switch with no Output_Rate yields no link rate"
        );
        assert_eq!(
            inputs.flows.len(),
            1,
            "the flow is still constrained by its deadline"
        );

        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            !diags
                .iter()
                .any(|d| d.message.starts_with("CqfSynthSchedule:")),
            "no link rate ⇒ early return, no schedule, got {diags:#?}"
        );
    }

    // ── Oracle 9: default branch — frame size defaulted when unspecified ─
    //
    // The source has a deadline but no Spar_TSN::Max_Frame_Size, so the
    // bridge reserves the DEFAULT_FRAME_BYTES (1500 B) fallback. Pins the
    // exact defaulted reservation so a mutated default or unwrap_or arm
    // (e.g. 1500→0) is caught.
    #[test]
    fn bridge_defaults_frame_size_when_unspecified() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Timing_Properties::Deadline => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let inputs = collect_cqf_inputs(&inst);
        assert_eq!(inputs.flows.len(), 1, "one bound stream");
        assert_eq!(
            inputs.flows[0].reserved_bits_per_cycle,
            DEFAULT_FRAME_BYTES * 8,
            "no Max_Frame_Size ⇒ DEFAULT_FRAME_BYTES (1500 B) · 8 = 12000 bits"
        );
    }

    // ── Oracle 10: the pass identifies itself ────────────────────────
    //
    // The registry keys passes by name(); a wrong/empty name silently
    // mis-registers or shadows another pass.
    #[test]
    fn bridge_pass_reports_its_name() {
        assert_eq!(CqfSynthAnalysis.name(), "cqf_synth");
        assert_eq!(CqfSynthAnalysis.name(), ANALYSIS_NAME);
    }

    // ── Oracle 11: soundness-guard boundary — T == period is SOUND ───
    //
    // The guard warns only when T strictly exceeds the period (more than
    // one frame can arrive per cycle). At T == period exactly, one frame
    // per cycle is admissible, so NO CqfSynthUnsound must fire — pinning
    // the boundary so `>` cannot weaken to `>=`. Deadline 100 us / (H=1+1)
    // drives T = 50 us; period is set to 50 us so T == period exactly.
    #[test]
    fn bridge_no_warn_when_cycle_equals_period() {
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type => FIFO;
      Spar_Network::Output_Rate => 1000000000 bitsps;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device src_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Max_Frame_Size    => 1000;
      Timing_Properties::Deadline => 100 us;
      Timing_Properties::Period   => 50 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  device snk_d
    features
      net  : requires bus access;
      in_p : in data port;
  end snk_d;
  device implementation snk_d.impl
  end snk_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device snk_d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        // Sanity-check the boundary actually holds: T == period == 50 us.
        let inputs = collect_cqf_inputs(&inst);
        let schedule = synthesize_cqf(&inputs.flows, inputs.link_rate_bps.unwrap())
            .expect("flows fit the cycle budget");
        assert_eq!(
            schedule.cycle_time_ps, 50_000_000,
            "T = deadline/(H+1) = 50 us, equal to the 50 us period"
        );

        let diags = CqfSynthAnalysis.analyze(&inst);
        assert!(
            !diags
                .iter()
                .any(|d| d.message.starts_with("CqfSynthUnsound:")),
            "T == period is sound (one frame per cycle fits) — no warning, got {diags:#?}"
        );
    }
}
