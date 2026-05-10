//! WCTT (Worst-Case Traversal Time) Analysis.
//!
//! Composes spar-network's NC primitives over the [`NetworkGraph`]
//! extracted from a [`SystemInstance`] to produce per-stream end-to-end
//! traversal-time bounds.
//!
//! # Phase 1 (this commit)
//!
//! Classical FIFO/Priority networks. Streams are inferred from AADL
//! data-port (or feature) connections that bind to one or more
//! switched buses via `Deployment_Properties::Actual_Connection_Binding`.
//! For each stream we walk the bound switches in order, compute the
//! per-hop residual service curve (splitting bandwidth across competing
//! streams on the same switch), apply [`delay_bound`] for the per-hop
//! delay, and propagate the [`output_bound`] forward to the next hop.
//! The end-to-end WCTT is the sum of per-hop delays.
//!
//! Priority switches are accepted in Phase 1 but treated identically to
//! FIFO — the per-class residual-service decomposition lands in Phase 2
//! alongside the `Spar_TSN` property set. TSN switches are even more
//! opaque and emit a Phase-2 deferral note.
//!
//! # Diagnostics
//!
//! - [`Severity::Info`] `WcttBound`: per-stream end-to-end traversal-time
//!   bound, one diagnostic per analysed stream.
//! - [`Severity::Error`] `WcttExceedsBudget`: a stream's predicted bound
//!   exceeds an explicit `Spar_Network::WCTT_Budget` set on the source
//!   bus.
//! - [`Severity::Error`] `WcttUnservable`: at some hop the residual
//!   service for the tagged stream is exhausted by competing flows
//!   (`ρ_competing ≥ R`); the analysis stops walking that stream.
//! - [`Severity::Error`] `WcttSwitchOverloaded`: aggregate competing
//!   arrival rate exceeds the switch port's service rate (>1 utilisation).
//! - [`Severity::Info`] `WcttDeferred`: a TSN switch was encountered;
//!   Phase 2 is required for TAS/CBS-shaped service curves.
//!
//! # Non-regression
//!
//! Models with **zero** `Spar_Network::Switch_Type` annotations produce
//! **zero** `Wctt*` diagnostics. See the
//! `no_switched_buses_emits_no_diagnostics` test.

use rustc_hash::FxHashMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, PropertyExpr};
use spar_hir_def::name::Name;
use spar_hir_def::properties::PropertyMap;
use spar_hir_def::property_value::parse_time_value;

use spar_network::curves::{
    ArrivalCurve, NcError, ServiceCurve, delay_bound, output_bound, residual_service,
};
use spar_network::extract::{
    extract_network_graph, read_forwarding_latency_ps, read_output_rate_bps, read_queue_depth,
};
use spar_network::tsn::{
    CbsReservation, ClassOfService, GateSchedule, cbs_residual_service, frame_quantization_ps,
    get_bandwidth_reservation_bps, get_class_of_service, get_frame_preemption, get_gate_schedule,
    get_hi_credit_bytes, get_lo_credit_bytes, get_max_frame_size_bytes, get_sync_error_ps,
    is_express_stream, preemption_blocking_term_ps, tas_residual_service_with_sync_error,
};
use spar_network::types::{NetworkGraph, NodeKind, SwitchType};

use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

const SPAR_NETWORK: &str = "Spar_Network";
const DEPLOYMENT: &str = "Deployment_Properties";

/// Default per-stream burst (bytes) used when neither the source nor
/// the bus specifies a [`read_queue_depth`]. Conservatively chosen to be
/// the standard Ethernet MTU so the analysis is never silently
/// optimistic.
const DEFAULT_BURST_BYTES: u64 = 1500;

/// Default frame size in bytes assumed by [`read_queue_depth`] when
/// expressed in frames; we approximate one queue slot as one MTU.
const FRAME_BYTES: u64 = 1500;

/// Maximum preemptable Ethernet frame size in bytes used for the
/// legacy blocking term (MTU + 14-byte Ethernet header + 4-byte FCS =
/// 1518 bytes). This is the worst-case in-flight preemptable frame an
/// express stream can be blocked behind on a TSN-capable port that
/// has *not* enabled 802.1Qbu preemption — the value the legacy
/// blocking term [`preemption_blocking_term_ps`] computes when called
/// with `preemption_enabled = false`. See IEEE 802.1Qbu §6.7.2 and
/// `docs/designs/track-d-tsn-wctt-research.md` §5.2-5.3.
const FRAME_BYTES_PREEMPTION_LEGACY: u64 = 1518;

/// Default maximum competing-class frame size assumed by the CBS
/// service-curve closed form when no `Spar_TSN::Max_Frame_Size` is
/// declared on competing flows. Standard Ethernet MTU including the
/// preamble — pessimistic but safe.
const CBS_DEFAULT_COMPETING_FRAME_BYTES: u64 = 1518;

/// WCTT analysis pass.
///
/// See the module-level docs for diagnostic kinds and the Phase 1
/// algorithm.
pub struct WcttAnalysis;

impl Analysis for WcttAnalysis {
    fn name(&self) -> &str {
        "wctt"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        self.compute(instance)
    }
}

impl WcttAnalysis {
    fn compute(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        let mut diags = Vec::new();

        // Step 1: extract the typed network graph. Without any switched
        // buses we have nothing to analyse and we deliberately emit no
        // diagnostics (non-regression contract).
        let graph = extract_network_graph(instance);
        if graph.switches().count() == 0 {
            return diags;
        }

        // Step 2: build the lookup tables we'll need:
        //   - bus name (lower-case) → bus idx
        //   - bus idx → SwitchType
        //   - bus idx → service curve at egress (rate-latency)
        //   - bus idx → WCTT budget in picoseconds (Some only when set)
        let mut bus_by_name: FxHashMap<String, ComponentInstanceIdx> = FxHashMap::default();
        let mut switch_type: FxHashMap<ComponentInstanceIdx, SwitchType> = FxHashMap::default();
        let mut service_for_bus: FxHashMap<ComponentInstanceIdx, ServiceCurve> =
            FxHashMap::default();
        let mut budget_ps_for_bus: FxHashMap<ComponentInstanceIdx, u64> = FxHashMap::default();
        // Per-bus TAS gate schedule, when present on a TSN-typed bus.
        // Used to dispatch [`tas_residual_service`] in the per-hop walk
        // below.
        let mut gate_schedule_for_bus: FxHashMap<ComponentInstanceIdx, GateSchedule> =
            FxHashMap::default();
        // Per-bus link rate (bits per second) — kept separately because
        // `tas_residual_service` rebuilds the service curve from the raw
        // R_link rather than the rate-latency form already in
        // `service_for_bus`.
        let mut link_rate_for_bus: FxHashMap<ComponentInstanceIdx, u64> = FxHashMap::default();
        for node in graph.switches() {
            if let NodeKind::Switch { switch_type: st } = node.kind {
                switch_type.insert(node.idx, st);
            }
            bus_by_name.insert(node.name.to_ascii_lowercase(), node.idx);

            let props = instance.properties_for(node.idx);
            let rate_bps = read_output_rate_bps(props).unwrap_or(0);
            let (_bcet_ps, wcet_ps) = read_forwarding_latency_ps(props).unwrap_or((0, 0));
            service_for_bus.insert(node.idx, ServiceCurve::rate_latency(rate_bps, wcet_ps));
            link_rate_for_bus.insert(node.idx, rate_bps);
            if let Some(budget) = read_wctt_budget_ps(props) {
                budget_ps_for_bus.insert(node.idx, budget);
            }
            // Spar_TSN::Gate_Control_List on the bus enables the TAS
            // service-curve dispatch below (v0.8.1 commit 2). Malformed
            // GCL strings are silently treated as "no schedule" — the
            // existing WcttDeferred path handles that fall-through.
            if let Some(schedule) = get_gate_schedule(props) {
                gate_schedule_for_bus.insert(node.idx, schedule);
            }
        }

        // Step 3: discover streams. A stream is a non-bus-access AADL
        // connection (port / feature-group / feature / parameter) whose
        // source and destination both resolve to end-station components
        // and which carries `Actual_Connection_Binding => reference (sw)`
        // to one or more switched buses.
        let streams = collect_streams(instance, &graph, &bus_by_name);

        if streams.is_empty() {
            return diags;
        }

        // Step 4: per-switch overload check. Sum sustained arrival rates
        // of all streams traversing a given switch and compare against
        // its service rate. >100% means the switch is structurally
        // overloaded — we emit a single `WcttSwitchOverloaded` and keep
        // walking; the per-stream `WcttUnservable` will follow on the
        // affected streams.
        let mut sw_aggregate_rate: FxHashMap<ComponentInstanceIdx, u128> = FxHashMap::default();
        for s in &streams {
            for hop in &s.hops {
                let entry = sw_aggregate_rate.entry(*hop).or_insert(0);
                *entry = entry.saturating_add(s.alpha.sustained_rate_bps as u128);
            }
        }
        let mut sw_keys: Vec<ComponentInstanceIdx> = sw_aggregate_rate.keys().copied().collect();
        sw_keys.sort_by_key(|idx| graph.node(*idx).map(|n| n.name.clone()).unwrap_or_default());
        for sw_idx in &sw_keys {
            let agg = sw_aggregate_rate[sw_idx];
            let svc = service_for_bus
                .get(sw_idx)
                .copied()
                .unwrap_or(ServiceCurve::rate_latency(0, 0));
            if svc.rate_bps == 0 {
                continue;
            }
            // utilisation in percent, integer-rounded toward zero.
            let utilization_pct = (agg.saturating_mul(100) / svc.rate_bps as u128) as u64;
            if utilization_pct > 100 {
                let path = graph
                    .node(*sw_idx)
                    .map(|n| component_path(instance, n.idx))
                    .unwrap_or_default();
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "WcttSwitchOverloaded: switch '{}' utilization {}% (>100%): aggregate \
                         arrival rate {} bps exceeds service rate {} bps",
                        graph
                            .node(*sw_idx)
                            .map(|n| n.name.as_str())
                            .unwrap_or("<unknown>"),
                        utilization_pct,
                        agg,
                        svc.rate_bps,
                    ),
                    path,
                    analysis: self.name().to_string(),
                });
            }
        }

        // Step 5: walk each stream's hops, accumulating per-hop delay
        // and propagating the output (departure) curve forward.
        for stream in &streams {
            let stream_name = stream.display_name(instance);
            let stream_path = component_path(instance, stream.src_idx);
            let mut alpha = stream.alpha;
            let mut total_delay_ps: u64 = 0;
            let mut unservable_emitted = false;
            let mut deferred_emitted = false;
            // v0.9.2 sensitivity tracking: capture the *minimum* residual
            // service rate across hops (worst-case sensitivity) and the
            // number of hops contributing to total_delay_ps. Both feed
            // the post-stream WcttSensitivity diagnostic.
            let mut min_residual_bps: u64 = u64::MAX;
            let mut max_comp_rate_bps: u64 = 0;
            let mut hops_counted: u64 = 0;

            // v0.9.2 RTA→WCTT release-jitter coupling diagnostic. The
            // burst inflation already happened in `collect_streams`;
            // this is the user-facing Info that the coupling fired.
            if stream.jitter_burst_bytes > 0 {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "WcttRtaCoupled: stream '{}' release-jitter {} ns inflates ingress \
                         burst by {} B (ρ·J coupling — RTA → WCTT per Buttazzo / Le \
                         Boudec & Thiran)",
                        stream_name,
                        stream.release_jitter_ps / 1_000,
                        stream.jitter_burst_bytes,
                    ),
                    path: stream_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            for (hop_idx, sw_idx) in stream.hops.iter().enumerate() {
                let st = switch_type.get(sw_idx).copied().unwrap_or(SwitchType::Fifo);

                // ── TSN switch dispatch ──────────────────────────
                //
                // Four-way dispatch order (see PR #180 / #181 / #182
                // and docs/designs/track-d-tsn-wctt-research.md §5.2):
                //   1. **TAS (802.1Qbv) gate-window service curve** —
                //      when the bus has a parsed
                //      `Spar_TSN::Gate_Control_List` *and* the stream
                //      carries `Spar_TSN::Class_of_Service`. Use
                //      `tas_residual_service` and emit `WcttTasGated`.
                //   2. **CBS (802.1Qav) credit-pool service curve** —
                //      when the stream carries
                //      `Spar_TSN::Bandwidth_Reservation` (idle-slope).
                //      The CBS curve absorbs other-class blocking
                //      (including preemption blocking) into its
                //      latency term, so we prefer it over the
                //      preemption arm when both could fire. Emit
                //      `WcttCbsShaped` and set `cbs_service` so the
                //      downstream competing-flow residual subtraction
                //      is suppressed.
                //   3. **Frame preemption (802.1Qbu) blocking term** —
                //      when the bus has `Frame_Preemption => true` and
                //      the stream is express (per `is_express_stream`).
                //      Use the bus service curve with its forwarding
                //      latency replaced by the fragment-blocking term;
                //      emit `WcttPreemptionApplied` reporting the gain
                //      relative to the legacy max-frame blocking.
                //   4. **Deferred** — none of the above. Emit
                //      `WcttDeferred` once per stream and skip the hop.
                //
                // CBS is class-isolated: its service-curve latency
                // already absorbs blocking by other classes, so the
                // per-hop residual subtraction below is suppressed when
                // `cbs_service.is_some()` (`cbs_active`). TAS leaves
                // the residual decomposition in place — the gate is
                // about *when* class-K can transmit, not about
                // ownership of bandwidth across competing streams.
                let mut cbs_service: Option<ServiceCurve> = None;
                // v0.9.1 NC soundness: a hop is "quantizable" iff its
                // service curve does not already account for the
                // atomic-frame max-MTU blocking term. The TAS arm and
                // the FIFO/Priority fallback arm do not — they undercount
                // the per-hop bound by up to one MTU because the
                // bytes-level NC kernel treats packets as continuous.
                // The CBS arm's `cbs_residual_service` already absorbs
                // max-frame blocking via its closed-form latency; the
                // preemption arm's `preemption_blocking_term_ps`
                // *replaces* the same term with the much smaller
                // fragment-time. Both of those leave `quantization_ps = 0`.
                let mut quantization_ps: u64 = 0;
                let svc = if matches!(st, SwitchType::Tsn) {
                    let bus_props = instance.properties_for(*sw_idx);
                    let bus_preemption = get_frame_preemption(bus_props).unwrap_or(false);
                    if let (Some(schedule), Some(cos)) =
                        (gate_schedule_for_bus.get(sw_idx), stream.cos)
                    {
                        // Path 1: TAS service curve, with the v0.9.1
                        // gPTP synchronization-error budget ε applied
                        // (subtracted from the effective open time,
                        // added to the worst-case gate latency). When
                        // `Spar_TSN::Sync_Error` is unset on the bus we
                        // pass ε = 0, which reproduces the v0.8.1
                        // service curve byte-identically.
                        let link_rate = link_rate_for_bus.get(sw_idx).copied().unwrap_or(0);
                        let sync_error_ps = get_sync_error_ps(bus_props).unwrap_or(0);
                        let tas_svc = tas_residual_service_with_sync_error(
                            schedule,
                            cos,
                            link_rate,
                            sync_error_ps,
                        );
                        let (open_ps, cycle_ps) = schedule.open_fraction(cos);
                        // open_fraction is reported as a percentage
                        // (integer-rounded toward zero) for human
                        // readability in the diagnostic message.
                        let open_pct = if cycle_ps == 0 {
                            0
                        } else {
                            ((open_ps as u128) * 100 / (cycle_ps as u128)) as u64
                        };
                        let gate_latency_ps = schedule.worst_case_latency(cos);
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "WcttTasGated: stream '{}' (CoS {}) on TSN switch '{}' at hop \
                                 {}: open fraction {}% gate latency {} ps sync_error {} ps",
                                stream_name,
                                cos.0,
                                graph
                                    .node(*sw_idx)
                                    .map(|n| n.name.as_str())
                                    .unwrap_or("<unknown>"),
                                hop_idx,
                                open_pct,
                                gate_latency_ps,
                                sync_error_ps,
                            ),
                            path: stream_path.clone(),
                            analysis: self.name().to_string(),
                        });
                        // TAS service curve: rate = R_link · ρ_K, but the
                        // link itself still serializes one max-frame per
                        // hop. Apply atomic-frame quantization at link rate.
                        let bus_max_frame = get_max_frame_size_bytes(bus_props).unwrap_or(1518);
                        quantization_ps = frame_quantization_ps(bus_max_frame, link_rate);
                        tas_svc
                    } else if let Some(idle_slope_bps) = stream.cbs_idle_slope_bps {
                        // Path 2: CBS service curve. Stream declares
                        // Spar_TSN::Bandwidth_Reservation. Build a
                        // reservation against the bus link rate and
                        // emit `WcttCbsShaped`. If the reservation
                        // fails to validate (oversubscription / zero
                        // link rate) we fall through to the preemption
                        // / deferred path so the user is asked to fix
                        // the model.
                        let link_rate_bps = read_output_rate_bps(bus_props).unwrap_or(0);
                        let max_competing_frame_bytes = get_max_frame_size_bytes(bus_props)
                            .unwrap_or(CBS_DEFAULT_COMPETING_FRAME_BYTES);
                        // v0.9.2: explicit hi/loCredit override the v0.8.1
                        // default (`max_competing_frame_bytes` for both),
                        // letting users plug in real Qcc/YANG credit
                        // numbers. Default unset = byte-identical to v0.8.1
                        // / v0.9.1.
                        let hi_credit_bytes =
                            get_hi_credit_bytes(bus_props).unwrap_or(max_competing_frame_bytes);
                        let lo_credit_bytes =
                            get_lo_credit_bytes(bus_props).unwrap_or(max_competing_frame_bytes);
                        let credits_explicit = get_hi_credit_bytes(bus_props).is_some()
                            || get_lo_credit_bytes(bus_props).is_some();
                        let reservation = CbsReservation::new(
                            idle_slope_bps,
                            link_rate_bps,
                            hi_credit_bytes,
                            lo_credit_bytes,
                        );
                        if let Some(reservation) = reservation {
                            let beta = cbs_residual_service(
                                &reservation,
                                link_rate_bps,
                                max_competing_frame_bytes,
                            );
                            cbs_service = Some(beta);
                            let cos_str = stream
                                .cos
                                .map(|c| c.0.to_string())
                                .unwrap_or_else(|| "?".to_string());
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "WcttCbsShaped: stream '{}' (cos={}) at hop {} on switch \
                                     '{}': CBS service curve idle_slope={} bps, \
                                     service_latency={} ns",
                                    stream_name,
                                    cos_str,
                                    hop_idx,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    idle_slope_bps,
                                    beta.latency_ps / 1_000,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            if credits_explicit {
                                diags.push(AnalysisDiagnostic {
                                    severity: Severity::Info,
                                    message: format!(
                                        "WcttCbsCredit: stream '{}' at hop {}: explicit \
                                         hi_credit={} B, lo_credit={} B (override default \
                                         max_competing_frame={} B)",
                                        stream_name,
                                        hop_idx,
                                        hi_credit_bytes,
                                        lo_credit_bytes,
                                        max_competing_frame_bytes,
                                    ),
                                    path: stream_path.clone(),
                                    analysis: self.name().to_string(),
                                });
                            }
                            beta
                        } else if bus_preemption && stream.is_express {
                            // CBS reservation invalid → fall back to
                            // preemption when applicable.
                            let svc_base = match service_for_bus.get(sw_idx) {
                                Some(s) => *s,
                                None => continue,
                            };
                            if svc_base.rate_bps == 0 {
                                continue;
                            }
                            let blocking_legacy_ps = preemption_blocking_term_ps(
                                svc_base.rate_bps,
                                FRAME_BYTES_PREEMPTION_LEGACY,
                                false,
                            );
                            let blocking_pmt_ps = preemption_blocking_term_ps(
                                svc_base.rate_bps,
                                FRAME_BYTES_PREEMPTION_LEGACY,
                                true,
                            );
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "WcttPreemptionApplied: stream '{}' at hop {} on TSN \
                                     switch '{}': blocking shrinks from {} ns (legacy \
                                     max-frame) to {} ns (802.1Qbu fragment + header)",
                                    stream_name,
                                    hop_idx,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    blocking_legacy_ps / 1_000,
                                    blocking_pmt_ps / 1_000,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            ServiceCurve::rate_latency(
                                svc_base.rate_bps,
                                svc_base.latency_ps.saturating_add(blocking_pmt_ps),
                            )
                        } else {
                            // CBS reservation invalid and no preemption
                            // fallback — defer.
                            if !deferred_emitted {
                                diags.push(AnalysisDiagnostic {
                                    severity: Severity::Info,
                                    message: format!(
                                        "WcttDeferred: stream '{}' traverses TSN switch '{}' \
                                         at hop {}; TAS/CBS-shaped service curves are \
                                         deferred to Phase 2 (tracked in \
                                         docs/designs/track-d-tsn-wctt-research.md §5.5)",
                                        stream_name,
                                        graph
                                            .node(*sw_idx)
                                            .map(|n| n.name.as_str())
                                            .unwrap_or("<unknown>"),
                                        hop_idx,
                                    ),
                                    path: stream_path.clone(),
                                    analysis: self.name().to_string(),
                                });
                                deferred_emitted = true;
                            }
                            continue;
                        }
                    } else if bus_preemption && stream.is_express {
                        // Path 3: frame preemption when bus enables
                        // it and the stream is express.
                        let svc_base = match service_for_bus.get(sw_idx) {
                            Some(s) => *s,
                            None => continue,
                        };
                        if svc_base.rate_bps == 0 {
                            continue;
                        }
                        let blocking_legacy_ps = preemption_blocking_term_ps(
                            svc_base.rate_bps,
                            FRAME_BYTES_PREEMPTION_LEGACY,
                            false,
                        );
                        let blocking_pmt_ps = preemption_blocking_term_ps(
                            svc_base.rate_bps,
                            FRAME_BYTES_PREEMPTION_LEGACY,
                            true,
                        );
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "WcttPreemptionApplied: stream '{}' at hop {} on TSN \
                                 switch '{}': blocking shrinks from {} ns (legacy \
                                 max-frame) to {} ns (802.1Qbu fragment + header)",
                                stream_name,
                                hop_idx,
                                graph
                                    .node(*sw_idx)
                                    .map(|n| n.name.as_str())
                                    .unwrap_or("<unknown>"),
                                blocking_legacy_ps / 1_000,
                                blocking_pmt_ps / 1_000,
                            ),
                            path: stream_path.clone(),
                            analysis: self.name().to_string(),
                        });
                        ServiceCurve::rate_latency(
                            svc_base.rate_bps,
                            svc_base.latency_ps.saturating_add(blocking_pmt_ps),
                        )
                    } else {
                        // Path 4: deferred placeholder. Emit at most
                        // one WcttDeferred per stream.
                        if !deferred_emitted {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "WcttDeferred: stream '{}' traverses TSN switch '{}' at hop \
                                     {}; TAS/CBS-shaped service curves are deferred to Phase 2 \
                                     (tracked in docs/designs/track-d-tsn-wctt-research.md §5.5)",
                                    stream_name,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    hop_idx,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            deferred_emitted = true;
                        }
                        continue;
                    }
                } else {
                    let s = match service_for_bus.get(sw_idx) {
                        Some(s) => *s,
                        None => continue,
                    };
                    // FIFO / Priority hop: bytes-level NC undercounts by
                    // up to one MTU; apply atomic-frame quantization.
                    let bus_props = instance.properties_for(*sw_idx);
                    let bus_max_frame = get_max_frame_size_bytes(bus_props).unwrap_or(1518);
                    quantization_ps = frame_quantization_ps(bus_max_frame, s.rate_bps);
                    s
                };

                if svc.rate_bps == 0 {
                    // No bandwidth declared — without a finite service
                    // rate we cannot bound delay; skip the hop with
                    // zero contribution. The Spar_Network::Output_Rate
                    // that fed the service curve is missing on the
                    // bus, which other passes (bus_bandwidth) already
                    // diagnose.
                    continue;
                }

                // Aggregate the competing flows (every other stream
                // that also crosses this switch) into a single
                // ArrivalCurve. We sum bursts and rates — a standard
                // (loose but safe) NC aggregation for FIFO servers.
                //
                // For the CBS path (`cbs_service.is_some()`) the
                // service curve is *already* class-isolated: its
                // latency captures worst-case blocking by other
                // classes via the closed-form `T = max_competing_frame
                // / link_rate + lo_credit / |sendSlope|`. Streams in
                // *other* classes therefore should not be subtracted
                // again. For v0.8.1 commit 3 we make the conservative
                // simplification of treating the CBS service curve as
                // exclusive to the tagged stream — same-class sharing
                // (residual decomposition between streams of one CBS
                // class) is a follow-up. The competing-flow set below
                // is suppressed to a no-op when CBS is active.
                let cbs_active = cbs_service.is_some();
                let mut comp_burst: u128 = 0;
                let mut comp_rate: u128 = 0;
                if !cbs_active {
                    for other in &streams {
                        if std::ptr::eq(other, stream) {
                            continue;
                        }
                        if !other.hops.contains(sw_idx) {
                            continue;
                        }
                        comp_burst = comp_burst.saturating_add(other.alpha.burst_bytes as u128);
                        comp_rate =
                            comp_rate.saturating_add(other.alpha.sustained_rate_bps as u128);
                    }
                }
                let comp_alpha = ArrivalCurve::affine(
                    saturate_u128_to_u64(comp_burst),
                    saturate_u128_to_u64(comp_rate),
                );

                // Compute residual service. If competing flows already
                // saturate the server (or are over the bus rate),
                // residual_service returns UnservableFlow; we emit
                // WcttUnservable and stop walking this stream.
                let residual = if comp_alpha.sustained_rate_bps == 0 && comp_alpha.burst_bytes == 0
                {
                    svc
                } else {
                    match residual_service(&svc, &comp_alpha) {
                        Ok(r) => r,
                        Err(NcError::UnservableFlow) | Err(NcError::UnstableServer) => {
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "WcttUnservable: stream '{}' at hop {} on switch '{}': \
                                     competing flows ({} bps) saturate the {} bps service rate",
                                    stream_name,
                                    hop_idx,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    comp_alpha.sustained_rate_bps,
                                    svc.rate_bps,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            unservable_emitted = true;
                            break;
                        }
                    }
                };

                // v0.9.2 sensitivity: capture the residual service rate
                // and the competing rate at this hop *before* computing
                // delay. They feed the WcttSensitivity diagnostic.
                if residual.rate_bps > 0 && residual.rate_bps < min_residual_bps {
                    min_residual_bps = residual.rate_bps;
                }
                if comp_alpha.sustained_rate_bps > max_comp_rate_bps {
                    max_comp_rate_bps = comp_alpha.sustained_rate_bps;
                }

                // Per-hop delay using the tagged stream's α and the
                // residual service. Then add `quantization_ps` for
                // atomic-frame correctness (zero on CBS / preemption arms,
                // computed at link rate on TAS / FIFO arms).
                match delay_bound(&alpha, &residual) {
                    Ok(d) => {
                        total_delay_ps = total_delay_ps.saturating_add(d);
                        hops_counted = hops_counted.saturating_add(1);
                        if quantization_ps > 0 {
                            total_delay_ps = total_delay_ps.saturating_add(quantization_ps);
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Info,
                                message: format!(
                                    "WcttFrameQuantization: stream '{}' at hop {} on switch \
                                     '{}': atomic-frame correction +{} ns (max-frame serialization \
                                     at link rate)",
                                    stream_name,
                                    hop_idx,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    quantization_ps / 1_000,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                        }
                    }
                    Err(NcError::UnservableFlow) | Err(NcError::UnstableServer) => {
                        // delay_bound also returns UnstableServer when
                        // the tagged α exceeds the residual β rate;
                        // this is the same observable pathology so we
                        // surface it under the same `WcttUnservable`
                        // diagnostic.
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Error,
                            message: format!(
                                "WcttUnservable: stream '{}' at hop {} on switch '{}': tagged \
                                 arrival rate {} bps exceeds residual service rate {} bps",
                                stream_name,
                                hop_idx,
                                graph
                                    .node(*sw_idx)
                                    .map(|n| n.name.as_str())
                                    .unwrap_or("<unknown>"),
                                alpha.sustained_rate_bps,
                                residual.rate_bps,
                            ),
                            path: stream_path.clone(),
                            analysis: self.name().to_string(),
                        });
                        unservable_emitted = true;
                        break;
                    }
                }

                // Propagate the departure curve to the next hop.
                if let Ok(out) = output_bound(&alpha, &residual) {
                    alpha = out;
                }
            }

            if unservable_emitted {
                continue;
            }

            // Step 6: budget check. If the source bus carried a
            // WCTT_Budget, compare. We use the *first* bound switch's
            // budget as the per-stream budget (matches the doc's
            // "explicit budget on the source bus" wording).
            if let Some(first_bus) = stream.hops.first()
                && let Some(&budget_ps) = budget_ps_for_bus.get(first_bus)
                && total_delay_ps > budget_ps
            {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "WcttExceedsBudget: stream '{}' predicted end-to-end WCTT {} ps > \
                         budget {} ps",
                        stream_name, total_delay_ps, budget_ps,
                    ),
                    path: stream_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "WcttBound: stream '{}' end-to-end WCTT {} ps ({} hop{})",
                    stream_name,
                    total_delay_ps,
                    stream.hops.len(),
                    if stream.hops.len() == 1 { "" } else { "s" },
                ),
                path: stream_path.clone(),
                analysis: self.name().to_string(),
            });

            // v0.9.2 sensitivity output (NC reviewer top-5 #13 — pure
            // post-processing on closed-form derivatives). For each
            // bound, report worst-case partial derivatives at the
            // operating point. Not bounds themselves; informational.
            //
            // d_e2e ≈ Σ_h ( T_h + σ / R_residual_h )  [bytes-fluid kernel]
            //   ∂d/∂σ_self      = Σ 8e12 / R_residual_h ps/B; bound below by
            //                     8e12 / min(R_residual) (worst hop dominates)
            //   ∂d/∂ρ_competing ≈ σ_total / (R - ρ_c)^2 at the worst hop
            //   ∂d/∂T_link      = hops_counted (chain rule across passthrough)
            //
            // When `min_residual_bps == u64::MAX` no hop contributed
            // (all deferred / unservable); skip emission.
            if hops_counted > 0 && min_residual_bps != u64::MAX && min_residual_bps > 0 {
                // ps per byte = 8 bits/B · 1e12 ps/s / R bps. Saturate
                // on the unlikely overflow path.
                let dsigma_ps_per_byte = (8u128 * 1_000_000_000_000u128)
                    .checked_div(min_residual_bps as u128)
                    .unwrap_or(u128::MAX);
                let dsigma_ns_per_byte = dsigma_ps_per_byte / 1_000;
                // Aggregate σ_total across the chain (rough proxy is the
                // self-burst plus max competing burst at any hop). Use
                // initial alpha + max_comp_rate × stream-period as a
                // safe upper estimate; lacking that, fall back to the
                // self-burst alone.
                let sigma_total_bytes = stream.alpha.burst_bytes as u128;
                let dt_link_unitless = hops_counted;
                // For ρ_c sensitivity: closed-form is σ/(R-ρ)^2; we
                // approximate using residual rate squared.
                let r_residual_sq = (min_residual_bps as u128).pow(2).max(1);
                let drho_ps_per_bps = sigma_total_bytes
                    .saturating_mul(8u128 * 1_000_000_000_000u128)
                    .checked_div(r_residual_sq)
                    .unwrap_or(0);
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Info,
                    message: format!(
                        "WcttSensitivity: stream '{}' end-to-end ∂WCTT (worst hop, residual rate \
                         {} bps): ∂σ_self={} ns/B, ∂ρ_competing≈{} ps per bps (using σ={} B), \
                         ∂T_link={} ns/ns",
                        stream_name,
                        min_residual_bps,
                        dsigma_ns_per_byte,
                        drho_ps_per_bps,
                        sigma_total_bytes,
                        dt_link_unitless,
                    ),
                    path: stream_path,
                    analysis: self.name().to_string(),
                });
            }
        }

        diags
    }
}

/// End-to-end traversal-time bound computed by
/// [`compute_network_hop_latency`].
///
/// All times are picoseconds. `min_ps` is the optimistic
/// (forwarding-latency-floor-only) estimate and `max_ps` is the NC-derived
/// worst-case bound across the bound switches.
///
/// `unservable` is set when one of the bound switches' residual service
/// is exhausted by competing traffic (mirrors the
/// [`Severity::Error`]-emitting `WcttUnservable` diagnostic in
/// [`WcttAnalysis::analyze`]). Callers — chiefly
/// `latency.rs` — surface this state with their own diagnostic instead of
/// blindly accumulating the bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkHopLatency {
    /// Best-case bound, picoseconds. Sum of the per-hop forwarding-latency
    /// floors (BCET side of `Spar_Network::Forwarding_Latency`) — no
    /// queuing contribution.
    pub min_ps: u64,
    /// Worst-case bound, picoseconds. Sum of the per-hop NC delay bounds
    /// (forwarding latency + queuing).
    pub max_ps: u64,
    /// `true` when at least one hop's residual service is saturated by
    /// competing flows; `max_ps` is then a placeholder (caller should
    /// emit an explicit error and stop aggregating that chain).
    pub unservable: bool,
}

/// Compute the per-hop end-to-end network traversal-time bound for a
/// single AADL connection that crosses one or more switched buses.
///
/// Returns `None` when the connection is *not* a network hop, namely:
/// - the connection has no `Actual_Connection_Binding` to any bus
///   declared with `Spar_Network::Switch_Type`; or
/// - the connection cannot be resolved in the owner.
///
/// When a binding to a switched bus is found, the helper walks every
/// bound switch in order, summing the per-hop NC delay bound (using the
/// same residual-service / aggregate-competing-flows decomposition as
/// [`WcttAnalysis::analyze`]). Competing flows are inferred from every
/// other connection in the owner that binds to the same switch.
///
/// This is the public entry point for the v0.8.0 latency-analysis
/// integration (Track D commit 6) — `latency.rs` invokes it for each
/// connection segment in an end-to-end flow and substitutes the result
/// for the legacy scalar `Bus_Properties::Latency` placeholder when the
/// model carries `Spar_Network::*` annotations.
pub fn compute_network_hop_latency(
    instance: &SystemInstance,
    owner_idx: ComponentInstanceIdx,
    connection_name: &str,
) -> Option<NetworkHopLatency> {
    // Build the network graph and the bus-name lookup. We extract the
    // graph fresh on each call: callers in `latency.rs` already iterate
    // O(segments) per chain, so the cost is bounded and avoids leaking
    // a cache across the public API. (A salsa-cached variant is the
    // natural follow-up when this becomes a hotspot.)
    let graph = extract_network_graph(instance);
    if graph.switches().count() == 0 {
        return None;
    }

    let mut bus_by_name: FxHashMap<String, ComponentInstanceIdx> = FxHashMap::default();
    for node in graph.switches() {
        bus_by_name.insert(node.name.to_ascii_lowercase(), node.idx);
    }

    let owner = instance.component(owner_idx);
    let conn_idx = owner.connections.iter().copied().find(|&idx| {
        instance.connections[idx]
            .name
            .as_str()
            .eq_ignore_ascii_case(connection_name)
    })?;
    let conn = &instance.connections[conn_idx];
    if matches!(conn.kind, spar_hir_def::item_tree::ConnectionKind::Access) {
        return None;
    }

    // Connection-level binding takes precedence over owner-level.
    // `latency.rs` calls don't currently distinguish; we only check the
    // owner's properties (matches `collect_streams`'s behaviour).
    let owner_props = instance.properties_for(owner_idx);
    let bound_buses = resolve_connection_binding(owner_props, conn.name.as_str(), &bus_by_name);

    let hops: Vec<ComponentInstanceIdx> = bound_buses
        .into_iter()
        .filter(|idx| {
            graph
                .node(*idx)
                .map(|n| matches!(n.kind, NodeKind::Switch { .. }))
                .unwrap_or(false)
        })
        .collect();

    if hops.is_empty() {
        return None;
    }

    // Resolve source endpoint to drive the source-side arrival curve.
    // For non-end-station endpoints (typically threads on a CPU), we
    // fall back to the conservative Ethernet-MTU burst.
    let src_idx = conn
        .src
        .as_ref()
        .and_then(|end| resolve_subcomponent(instance, owner_idx, &end.subcomponent));

    let (rate_bps, burst_bytes, src_is_express) = if let Some(idx) = src_idx {
        let src_props = instance.properties_for(idx);
        let rate = read_output_rate_bps(src_props).unwrap_or(0);
        let burst = read_queue_depth(src_props)
            .map(|q| q.saturating_mul(FRAME_BYTES))
            .unwrap_or(DEFAULT_BURST_BYTES);
        let express = is_express_stream(src_props);
        (rate, burst, express)
    } else {
        (0, DEFAULT_BURST_BYTES, false)
    };

    let mut alpha = ArrivalCurve::affine(burst_bytes, rate_bps);

    // Aggregate competing flows per-switch in advance — every other
    // connection in the same owner whose binding includes the same
    // switch is a competitor.
    let mut comp_alpha_by_sw: FxHashMap<ComponentInstanceIdx, (u128, u128)> = FxHashMap::default();
    for &other_conn_idx in &owner.connections {
        if other_conn_idx == conn_idx {
            continue;
        }
        let other_conn = &instance.connections[other_conn_idx];
        if matches!(
            other_conn.kind,
            spar_hir_def::item_tree::ConnectionKind::Access
        ) {
            continue;
        }
        let other_buses =
            resolve_connection_binding(owner_props, other_conn.name.as_str(), &bus_by_name);
        let other_src_idx = other_conn
            .src
            .as_ref()
            .and_then(|end| resolve_subcomponent(instance, owner_idx, &end.subcomponent));
        let (o_rate, o_burst) = if let Some(idx) = other_src_idx {
            let p = instance.properties_for(idx);
            let r = read_output_rate_bps(p).unwrap_or(0);
            let b = read_queue_depth(p)
                .map(|q| q.saturating_mul(FRAME_BYTES))
                .unwrap_or(DEFAULT_BURST_BYTES);
            (r, b)
        } else {
            (0, DEFAULT_BURST_BYTES)
        };
        for sw_idx in other_buses {
            let entry = comp_alpha_by_sw.entry(sw_idx).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(o_burst as u128);
            entry.1 = entry.1.saturating_add(o_rate as u128);
        }
    }

    let mut total_max_ps: u64 = 0;
    let mut total_min_ps: u64 = 0;
    let mut unservable = false;

    for sw_idx in &hops {
        let bus_props = instance.properties_for(*sw_idx);
        let rate = read_output_rate_bps(bus_props).unwrap_or(0);
        let (bcet_ps, wcet_ps) = read_forwarding_latency_ps(bus_props).unwrap_or((0, 0));
        let bus_preemption = get_frame_preemption(bus_props).unwrap_or(false);
        let svc_base = ServiceCurve::rate_latency(rate, wcet_ps);

        // The min (best-case) bound is the BCET forwarding latency for
        // this hop — purely the propagation/forwarding floor, no
        // queuing. We deliberately ignore competing traffic here since
        // BCET is a lower bound.
        total_min_ps = total_min_ps.saturating_add(bcet_ps);

        if svc_base.rate_bps == 0 {
            // Without a finite service rate we cannot compute a worst-
            // case bound for this hop. Skip the queuing contribution
            // and trust BCET == WCET on the propagation floor.
            total_max_ps = total_max_ps.saturating_add(wcet_ps);
            continue;
        }

        // TSN switches: preemption-aware dispatch (v0.8.1 c4). When
        // both the bus and the source stream declare
        // `Frame_Preemption => true`, we replace the bus
        // forwarding-latency floor with the small fragment-blocking
        // term and continue with residual_service / delay_bound.
        // Without preemption active we keep the v0.8.0 Phase 1 fallback
        // (propagate the WCET floor only) — c2/c3 fill in TAS/CBS later.
        let mut svc = svc_base;
        if let Some(node) = graph.node(*sw_idx)
            && let NodeKind::Switch { switch_type } = node.kind
            && matches!(switch_type, SwitchType::Tsn)
        {
            if bus_preemption && src_is_express {
                let blocking_pmt_ps = preemption_blocking_term_ps(
                    svc_base.rate_bps,
                    FRAME_BYTES_PREEMPTION_LEGACY,
                    true,
                );
                svc = ServiceCurve::rate_latency(
                    svc_base.rate_bps,
                    svc_base.latency_ps.saturating_add(blocking_pmt_ps),
                );
            } else {
                total_max_ps = total_max_ps.saturating_add(wcet_ps);
                continue;
            }
        }

        // Build the competing arrival curve for this hop.
        let (comp_burst_u128, comp_rate_u128) =
            comp_alpha_by_sw.get(sw_idx).copied().unwrap_or((0, 0));
        let comp_alpha = ArrivalCurve::affine(
            saturate_u128_to_u64(comp_burst_u128),
            saturate_u128_to_u64(comp_rate_u128),
        );

        let residual = if comp_alpha.sustained_rate_bps == 0 && comp_alpha.burst_bytes == 0 {
            svc
        } else {
            match residual_service(&svc, &comp_alpha) {
                Ok(r) => r,
                Err(_) => {
                    unservable = true;
                    break;
                }
            }
        };

        match delay_bound(&alpha, &residual) {
            Ok(d) => total_max_ps = total_max_ps.saturating_add(d),
            Err(_) => {
                unservable = true;
                break;
            }
        }

        if let Ok(out) = output_bound(&alpha, &residual) {
            alpha = out;
        }
    }

    Some(NetworkHopLatency {
        min_ps: total_min_ps,
        max_ps: total_max_ps,
        unservable,
    })
}

/// Read `Spar_Network::WCTT_Budget` (Time) in picoseconds. Mirrors the
/// typed-first / string-fallback idiom used by the network extractor's
/// other accessors.
fn read_wctt_budget_ps(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = props
        .get_typed(SPAR_NETWORK, "WCTT_Budget")
        .or_else(|| props.get_typed("", "WCTT_Budget"))
        && let Some(ps) = extract_time_ps(expr)
    {
        return Some(ps);
    }
    let raw = props
        .get(SPAR_NETWORK, "WCTT_Budget")
        .or_else(|| props.get("", "WCTT_Budget"))?;
    parse_time_value(raw)
}

/// Convert a typed [`PropertyExpr`] for time into picoseconds. Local
/// reimplementation that avoids pulling `spar-network::extract`'s
/// private helper across crates.
fn extract_time_ps(expr: &PropertyExpr) -> Option<u64> {
    fn unit_factor(name: &str) -> Option<u64> {
        match name.to_ascii_lowercase().as_str() {
            "ps" => Some(1),
            "ns" => Some(1_000),
            "us" => Some(1_000_000),
            "ms" => Some(1_000_000_000),
            "sec" => Some(1_000_000_000_000),
            "min" => Some(60_000_000_000_000),
            "hr" => Some(3_600_000_000_000_000),
            _ => None,
        }
    }
    match expr {
        PropertyExpr::Integer(v, Some(unit)) if *v >= 0 => {
            unit_factor(unit.as_str()).and_then(|f| (*v as u64).checked_mul(f))
        }
        PropertyExpr::Integer(v, None) if *v >= 0 => Some(*v as u64),
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            let f = unit_factor(unit.as_str())?;
            Some((v * f as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let f = unit_factor(unit.as_str())?;
            match inner.as_ref() {
                PropertyExpr::Integer(v, _) if *v >= 0 => (*v as u64).checked_mul(f),
                PropertyExpr::Real(s, _) => {
                    let v: f64 = s.parse().ok()?;
                    Some((v * f as f64) as u64)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// A single stream's logical description for the WCTT walk.
#[derive(Debug, Clone)]
struct Stream {
    /// Stable connection name from AADL, used in diagnostics.
    name: String,
    /// Source end-station component (device/processor) idx.
    src_idx: ComponentInstanceIdx,
    /// Sink end-station component idx (kept for future PMOO/SFA use).
    #[allow(dead_code)]
    sink_idx: ComponentInstanceIdx,
    /// Ordered list of switched buses this stream traverses.
    hops: Vec<ComponentInstanceIdx>,
    /// Source-side arrival curve.
    alpha: ArrivalCurve,
    /// Stream's `Spar_TSN::Class_of_Service` (0..=7) when annotated on
    /// the source end station. Required for the TAS service-curve
    /// dispatch on TSN switches; also surfaced (when present) on
    /// the CBS dispatch path for diagnostic readability. Streams
    /// without a CoS that traverse a TSN switch with neither a TAS
    /// schedule nor a CBS reservation fall back to the
    /// [`Severity::Info`]-emitting `WcttDeferred` path.
    cos: Option<ClassOfService>,
    /// Whether this stream qualifies as "express" for IEEE 802.1Qbu
    /// preemption purposes (see [`is_express_stream`]). Express
    /// streams pay only the preemption-fragment blocking at TSN ports
    /// where the bus also enables `Frame_Preemption`; non-express
    /// streams keep the legacy `max_frame / R` blocking term.
    is_express: bool,
    /// CBS reserved bandwidth (idleSlope) in bits per second when the
    /// source declares `Spar_TSN::Bandwidth_Reservation`. The full
    /// `CbsReservation` (with hi/lo credit and send slope) is built at
    /// each TSN hop because it depends on the bus's link rate.
    cbs_idle_slope_bps: Option<u64>,
    /// v0.9.2 RTA→WCTT release-jitter coupling: when the source end
    /// station declares `Timing_Properties::Dispatch_Jitter`, that
    /// value (picoseconds) is treated as ingress release-jitter J and
    /// inflates the arrival burst by ρ·J bytes. Stored here so the
    /// `WcttRtaCoupled` Info diagnostic at run-time can echo the
    /// pair (jitter_ps, jitter_burst_bytes) back to the user. `0`
    /// when the property is unset (= byte-identical v0.8.x behaviour).
    release_jitter_ps: u64,
    jitter_burst_bytes: u64,
}

impl Stream {
    fn display_name(&self, instance: &SystemInstance) -> String {
        let src = instance.component(self.src_idx).name.as_str().to_string();
        let sink = instance.component(self.sink_idx).name.as_str().to_string();
        format!("{} ({} → {})", self.name, src, sink)
    }
}

/// Walk every component's connections looking for non-bus-access
/// connections that
/// 1. resolve to two end-station endpoints (device/processor on both
///    sides), and
/// 2. carry `Actual_Connection_Binding` to one or more buses that
///    appear as switches in `graph`.
fn collect_streams(
    instance: &SystemInstance,
    graph: &NetworkGraph,
    bus_by_name: &FxHashMap<String, ComponentInstanceIdx>,
) -> Vec<Stream> {
    let mut streams = Vec::new();

    for (owner_idx, owner) in instance.all_components() {
        for &conn_idx in &owner.connections {
            let conn = &instance.connections[conn_idx];
            // Bus-access connections describe topology, not data flow;
            // they are already consumed by the network extractor.
            if matches!(conn.kind, spar_hir_def::item_tree::ConnectionKind::Access) {
                continue;
            }

            let src_idx = conn
                .src
                .as_ref()
                .and_then(|end| resolve_subcomponent(instance, owner_idx, &end.subcomponent));
            let dst_idx = conn
                .dst
                .as_ref()
                .and_then(|end| resolve_subcomponent(instance, owner_idx, &end.subcomponent));

            let (Some(src_idx), Some(dst_idx)) = (src_idx, dst_idx) else {
                continue;
            };

            if src_idx == dst_idx {
                continue;
            }
            if !is_end_station_category(instance.component(src_idx).category) {
                continue;
            }
            if !is_end_station_category(instance.component(dst_idx).category) {
                continue;
            }

            // Lookup binding on the connection itself first, then on
            // the owner. Connection-level binding takes precedence.
            let binding_owner_props = instance.properties_for(owner_idx);
            let bound_buses =
                resolve_connection_binding(binding_owner_props, conn.name.as_str(), bus_by_name);

            // Filter to buses that the extractor classified as
            // switches; non-switched binding targets are ignored
            // (bus_bandwidth handles those).
            let hops: Vec<ComponentInstanceIdx> = bound_buses
                .into_iter()
                .filter(|idx| {
                    graph
                        .node(*idx)
                        .map(|n| matches!(n.kind, NodeKind::Switch { .. }))
                        .unwrap_or(false)
                })
                .collect();

            if hops.is_empty() {
                continue;
            }

            // Construct the source-side arrival curve.
            //
            // Sustained rate: prefer `Spar_Network::Output_Rate` on the
            // source end station (it characterises the *flow*, not the
            // bus). When the source has no annotation the stream is
            // treated as a pure single-burst flow (ρ = 0); that is the
            // tightest safe assumption when no rate metadata is
            // declared and matches the doc's "leave it to the user to
            // annotate" policy.
            //
            // Burst: source's `Spar_Network::Queue_Depth` (in frames,
            // scaled by FRAME_BYTES) when set; otherwise default to one
            // Ethernet MTU (DEFAULT_BURST_BYTES).
            let src_props = instance.properties_for(src_idx);
            let rate_bps = read_output_rate_bps(src_props).unwrap_or(0);
            let burst_base_bytes = read_queue_depth(src_props)
                .map(|q| q.saturating_mul(FRAME_BYTES))
                .unwrap_or(DEFAULT_BURST_BYTES);

            // v0.9.2 RTA→WCTT release-jitter coupling (NC reviewer top-5
            // #4 — single biggest credibility lift, no new math). When
            // the source end station declares `Timing_Properties::
            // Dispatch_Jitter`, treat it as release-jitter J: a thread
            // whose dispatcher fires up to J ps late at any cycle still
            // produces the same number of bytes per period, but the
            // *burst seen at the NIC* inflates by ρ·J. This couples
            // RTA's response-time semantics into the WCTT input.
            //
            // Default unset = J=0 = byte-identical to v0.8.1/v0.9.1.
            //
            // Future v0.9.x: also consume RTA's *computed*
            // response_time directly (today the user must propagate it
            // via Dispatch_Jitter explicitly, which is the existing
            // AS5506 property semantics).
            let release_jitter_ps = src_props
                .get("Timing_Properties", "Dispatch_Jitter")
                .or_else(|| src_props.get("", "Dispatch_Jitter"))
                .and_then(parse_time_value)
                .unwrap_or(0);
            // ρ·J in bytes = (rate_bps · jitter_ps) / 8 / 1e12, with
            // ceiling rounding so the burst is never under-estimated.
            let jitter_burst_bytes = if release_jitter_ps > 0 && rate_bps > 0 {
                let bits = (rate_bps as u128).saturating_mul(release_jitter_ps as u128);
                let bytes = bits.div_ceil(8u128 * 1_000_000_000_000u128);
                u64::try_from(bytes).unwrap_or(u64::MAX)
            } else {
                0
            };
            let burst_bytes = burst_base_bytes.saturating_add(jitter_burst_bytes);

            let alpha = ArrivalCurve::affine(burst_bytes, rate_bps);
            if jitter_burst_bytes > 0 {
                // Diagnostic emitted lazily inside `streams_diagnostics`
                // below since `stream_name` is built later. We thread
                // the jitter values through the Stream struct.
            }
            // TSN dispatch metadata read off the source end station.
            // Spar_TSN::Class_of_Service drives the TAS gate-window
            // service curve and is also surfaced on CBS-shaped
            // diagnostics; Spar_TSN::Bandwidth_Reservation drives the
            // CBS credit-pool service curve. The wctt walk only
            // consults these when a hop's switch is classified as
            // `SwitchType::Tsn`. The lookup follows the same precedence
            // as the typed/string accessor in spar-network::tsn — we
            // read from the source end station because these
            // properties apply to ports / connections; the closest
            // reachable carrier in the current property model is the
            // source device's PropertyMap, which the existing rate /
            // queue-depth reads already use.
            let cos = get_class_of_service(src_props);
            let cbs_idle_slope_bps = get_bandwidth_reservation_bps(src_props);

            // Express-stream classification (IEEE 802.1Qbu). Read from
            // the source-component's PropertyMap so that AADL fixtures
            // can declare either `Spar_TSN::Frame_Preemption => true`
            // explicitly or rely on the `Class_of_Service >= 6`
            // default. A stream that is not express still walks TSN
            // hops below; it just pays the legacy `max_frame / R`
            // blocking term rather than the preemption-fragment term.
            let is_express = is_express_stream(src_props);

            streams.push(Stream {
                name: conn.name.as_str().to_string(),
                src_idx,
                sink_idx: dst_idx,
                hops,
                alpha,
                cos,
                is_express,
                cbs_idle_slope_bps,
                release_jitter_ps,
                jitter_burst_bytes,
            });
        }
    }

    streams
}

/// Resolve `Actual_Connection_Binding` to a list of bus
/// [`ComponentInstanceIdx`]es. Accepts both single `reference (bus)` and
/// list `(reference (bus1), reference (bus2))` forms; preserves order
/// because that is the hop order the WCTT walk consumes.
fn resolve_connection_binding(
    props: &PropertyMap,
    _conn_name: &str,
    bus_by_name: &FxHashMap<String, ComponentInstanceIdx>,
) -> Vec<ComponentInstanceIdx> {
    // We rely on the raw string form because the existing typed
    // `PropertyExpr` shape for Actual_Connection_Binding is a
    // ListOfReference whose lowering across the workspace is currently
    // string-based. The existing bus_bandwidth pass uses the same
    // approach.
    let raw = match props
        .get(DEPLOYMENT, "Actual_Connection_Binding")
        .or_else(|| props.get("", "Actual_Connection_Binding"))
    {
        Some(s) => s,
        None => return Vec::new(),
    };

    let mut result = Vec::new();
    let mut s = raw;
    while let Some(start) = s.find("reference") {
        s = &s[start + "reference".len()..];
        if let Some(open) = s.find('(') {
            s = &s[open + 1..];
            if let Some(close) = s.find(')') {
                let target = s[..close].trim();
                if !target.is_empty()
                    && let Some(idx) = bus_by_name.get(&target.to_ascii_lowercase())
                {
                    result.push(*idx);
                }
                s = &s[close + 1..];
            } else {
                break;
            }
        } else {
            break;
        }
    }
    result
}

fn resolve_subcomponent(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    subcomponent: &Option<Name>,
) -> Option<ComponentInstanceIdx> {
    match subcomponent {
        Some(sub_name) => {
            let owner_comp = instance.component(owner);
            owner_comp
                .children
                .iter()
                .find(|&&child_idx| {
                    instance.component(child_idx).name.as_str() == sub_name.as_str()
                })
                .copied()
        }
        None => Some(owner),
    }
}

fn is_end_station_category(cat: ComponentCategory) -> bool {
    matches!(
        cat,
        ComponentCategory::Device | ComponentCategory::Processor
    )
}

fn saturate_u128_to_u64(v: u128) -> u64 {
    if v > u64::MAX as u128 {
        u64::MAX
    } else {
        v as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::{HirDefDatabase, file_item_tree, resolver::GlobalScope};

    fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
        let db = HirDefDatabase::default();
        let file =
            spar_base_db::SourceFile::new(&db, "wctt_test.aadl".to_string(), aadl_src.to_string());
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

    fn count_wctt(diags: &[AnalysisDiagnostic]) -> usize {
        diags
            .iter()
            .filter(|d| d.message.starts_with("Wctt"))
            .count()
    }

    // ── Test 1: non-regression — no switches, no diagnostics ─────────
    #[test]
    fn no_switched_buses_emits_no_diagnostics() {
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
      b : bus plain_bus.impl;
      x : device d.impl;
      y : device d.impl;
    connections
      c1 : bus access b -> x.net;
      c2 : bus access b -> y.net;
  end Sys.impl;
end Plain;
"#;
        let inst = instantiate(src, "Plain", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);
        assert_eq!(
            count_wctt(&diags),
            0,
            "models without Spar_Network::Switch_Type must produce zero Wctt* diagnostics, got {:#?}",
            diags
        );
    }

    // ── Test 2: single hop classical Ethernet bound ─────────────────
    #[test]
    fn single_hop_classical_ethernet_bound_correct() {
        // 1 Gbps switch, no forwarding latency, single stream; no
        // competing flows. Expected delay = latency + σ/R.
        // With burst = 1500 bytes, R = 1 Gbps, T = 0:
        //   D = 0 + 1500·8·10^12 / 1·10^9 = 12·10^6 ps = 12 us.
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

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let info: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttBound"))
            .collect();
        assert_eq!(info.len(), 1, "exactly one stream expected: {:#?}", diags);
        // v0.9.1 soundness: 12 µs (NC bytes-fluid) + 12.144 µs (atomic-frame
        // quantization at 1 Gbps for 1518-byte MTU) = 24.144 µs.
        assert!(
            info[0].message.contains("24144000 ps"),
            "expected 24.144 us bound (12 us NC + 12.144 us frame quantization), got: {}",
            info[0].message
        );
    }

    // ── v0.9.2 — RTA→WCTT release-jitter coupling ─────────────────
    #[test]
    fn rta_wctt_dispatch_jitter_inflates_burst_and_emits_diagnostic() {
        // Source device with Dispatch_Jitter = 100 us. At 1 Gbps,
        // ρ·J = 1e9 × 100e-6 / 8 = 12500 bytes of inflation.
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

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device src_d
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_Network::Output_Rate    => 1000000000 bitsps;
      Timing_Properties::Dispatch_Jitter => 100 us;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let coupled = diags
            .iter()
            .find(|d| d.message.starts_with("WcttRtaCoupled"))
            .unwrap_or_else(|| panic!("expected WcttRtaCoupled diagnostic, got: {:#?}", diags));
        assert!(
            coupled.message.contains("100000 ns"),
            "expected jitter 100000 ns in message: {}",
            coupled.message
        );
        // ρ·J = 1Gbps × 100us / 8 = 12500 bytes
        assert!(
            coupled.message.contains("12500 B"),
            "expected 12500 B inflation in message: {}",
            coupled.message
        );
    }

    #[test]
    fn no_dispatch_jitter_no_coupling_diagnostic() {
        // Without Dispatch_Jitter the coupling diagnostic must not
        // fire, preserving v0.8.x / v0.9.1 byte-identical output.
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

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);
        assert!(
            !diags
                .iter()
                .any(|d| d.message.starts_with("WcttRtaCoupled")),
            "no Dispatch_Jitter must not emit WcttRtaCoupled: {:#?}",
            diags
        );
    }

    // ── Test 3: two streams sharing one FIFO switch ─────────────────
    #[test]
    fn two_streams_share_switch_residual_split() {
        // Two streams share a 1 Gbps switch; each source-device
        // declares Output_Rate = 100 Mbps so the residual server seen
        // by each is 1Gbps − 100Mbps = 900 Mbps. Both streams must
        // converge to a finite per-stream bound.
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

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device src_d
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_Network::Output_Rate => 100000000 bitsps;
      Spar_Network::Queue_Depth => 1;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      a2 : device src_d.impl;
      b  : device d.impl;
      c  : device d.impl;
    connections
      c_sw_a  : bus access sw -> a.net;
      c_sw_a2 : bus access sw -> a2.net;
      c_sw_b  : bus access sw -> b.net;
      c_sw_c  : bus access sw -> c.net;
      data1   : port a.out_p  -> b.in_p;
      data2   : port a2.out_p -> c.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        let infos: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttBound"))
            .collect();
        assert_eq!(infos.len(), 2, "expected two streams: {:#?}", diags);
        // Each stream's bound is finite and equal (symmetric model).
        // Note: with rate=0 on competing streams, residual = base svc.
        for info in &infos {
            assert!(
                info.message.contains("ps"),
                "missing ps unit in: {}",
                info.message
            );
        }
    }

    // ── Test 4: two flows whose rates exceed server: unservable ─────
    #[test]
    fn competing_flow_exceeds_rate_emits_unservable() {
        // Two streams whose sustained rates each equal the server
        // rate (1 kbps source on a 1 kbps server). Aggregate
        // competing rate per stream is the other stream's 1 kbps =
        // saturates server. residual_service returns
        // UnservableFlow → WcttUnservable on each stream.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Output_Rate        => 1000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device hot_d
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_Network::Output_Rate => 1000 bitsps;
      Spar_Network::Queue_Depth => 1;
  end hot_d;
  device implementation hot_d.impl
  end hot_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device hot_d.impl;
      a2 : device hot_d.impl;
      b  : device d.impl;
      c  : device d.impl;
    connections
      c_sw_a  : bus access sw -> a.net;
      c_sw_a2 : bus access sw -> a2.net;
      c_sw_b  : bus access sw -> b.net;
      c_sw_c  : bus access sw -> c.net;
      data1   : port a.out_p  -> b.in_p;
      data2   : port a2.out_p -> c.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        let unservable: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttUnservable"))
            .collect();
        assert!(
            !unservable.is_empty(),
            "expected at least one WcttUnservable diagnostic: {:#?}",
            diags
        );
    }

    // ── Test 5: multi-hop chain bound = sum of per-hop delays ───────
    #[test]
    fn multi_hop_chain_bound_aggregates() {
        // Three switches in series, each with the same params. A
        // single stream binds to all three. The bound should be the
        // sum of three identical per-hop delays. Each hop has
        // forwarding latency = 5 us and effectively no competing
        // flow. Expected per-hop = 5 us + σ/R (12 us) = 17 us; total
        // = 51 us = 51_000_000 ps.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 5 us .. 5 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw1 : bus eth.impl;
      sw2 : bus eth.impl;
      sw3 : bus eth.impl;
      a   : device d.impl;
      b   : device d.impl;
    connections
      c_sw1_a : bus access sw1 -> a.net;
      c_sw3_b : bus access sw3 -> b.net;
      data1   : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding =>
        (reference (sw1), reference (sw2), reference (sw3));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        let info = diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap_or_else(|| panic!("expected WcttBound, got {:#?}", diags));
        assert!(
            info.message.contains("3 hops"),
            "expected 3 hops in: {}",
            info.message
        );
        // v0.9.1 soundness: 51 µs (NC) + 3 × 12.144 µs (atomic-frame
        // quantization, one per hop) = 87.432 µs.
        assert!(
            info.message.contains("87432000 ps"),
            "expected 87.432 us bound (51 us NC + 36.432 us quantization), got: {}",
            info.message
        );
    }

    // ── Test 6: bound exceeds budget → Error ────────────────────────
    #[test]
    fn bound_exceeds_budget_emits_error() {
        // Same single-hop topology as test 2 (12 us bound), with a
        // tight 1 us WCTT_Budget on the bus. Bound (12 us) must
        // exceed budget (1 us), so we expect `WcttExceedsBudget`.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_Network::WCTT_Budget        => 1 us;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let err = diags
            .iter()
            .find(|d| d.severity == Severity::Error && d.message.starts_with("WcttExceedsBudget"))
            .unwrap_or_else(|| panic!("expected WcttExceedsBudget Error: {:#?}", diags));
        assert!(err.message.contains("budget"));
    }

    // ── Test 7: bound within budget → Info only ─────────────────────
    #[test]
    fn bound_within_budget_emits_info_only() {
        // 200 us budget, bound is 12 us → no Error, just `WcttBound`.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_Network::WCTT_Budget        => 200 us;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let errors: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.starts_with("Wctt"))
            .collect();
        assert!(
            errors.is_empty(),
            "no Wctt Error expected when bound < budget: {:#?}",
            errors
        );
        let info = diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap();
        assert!(info.severity == Severity::Info);
    }

    // ── Test 8: priority switches treated like FIFO in Phase 1 ──────
    #[test]
    fn priority_switch_recognized_classified_correctly() {
        // Phase 1 simply walks Priority switches with the same
        // residual-service formula as FIFO (priority semantics ship in
        // Phase 2). Verify no panic and a `WcttBound` is emitted.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => Priority;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 1 us .. 1 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let info = diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap();
        assert!(info.severity == Severity::Info);
    }

    // ── Test 9: TSN switch defers to Phase 2 ────────────────────────
    #[test]
    fn tsn_switch_remains_opaque() {
        // TSN switch should emit a `WcttDeferred` Info diagnostic
        // noting Phase 2 is required. The hop is skipped (no per-hop
        // delay contribution).
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        let deferred = diags
            .iter()
            .find(|d| d.message.starts_with("WcttDeferred"))
            .unwrap_or_else(|| panic!("expected WcttDeferred: {:#?}", diags));
        assert!(deferred.severity == Severity::Info);
        assert!(deferred.message.contains("Phase 2"));
    }

    // ── Test 11: CBS — single-switch, two CBS classes each 30% ──────
    #[test]
    fn cbs_dispatch_two_classes_each_get_idle_slope() {
        // 1 Gbps TSN switch with two streams; each source declares a
        // 30% Bandwidth_Reservation (300 Mbps) and a CoS. Per the CBS
        // closed form, each class sees idleSlope = 300 Mbps service
        // rate + a latency capturing other-class blocking. The
        // analysis must emit `WcttCbsShaped` Info per stream and a
        // `WcttBound` per stream, and *no* `WcttDeferred` (CBS
        // dispatch supersedes the Phase-1 deferral on TSN switches).
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device cbs_a
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service        => 6;
      Spar_TSN::Bandwidth_Reservation   => 300000000 bitsps;
      Spar_Network::Queue_Depth         => 1;
  end cbs_a;
  device implementation cbs_a.impl
  end cbs_a.impl;

  device cbs_b
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service        => 5;
      Spar_TSN::Bandwidth_Reservation   => 300000000 bitsps;
      Spar_Network::Queue_Depth         => 1;
  end cbs_b;
  device implementation cbs_b.impl
  end cbs_b.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device cbs_a.impl;
      b  : device cbs_b.impl;
      x  : device d.impl;
      y  : device d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      c_sw_x : bus access sw -> x.net;
      c_sw_y : bus access sw -> y.net;
      data1  : port a.out_p -> x.in_p;
      data2  : port b.out_p -> y.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        let cbs_shaped: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttCbsShaped"))
            .collect();
        assert_eq!(
            cbs_shaped.len(),
            2,
            "expected one WcttCbsShaped per CBS stream: {:#?}",
            diags
        );
        for d in &cbs_shaped {
            assert!(d.severity == Severity::Info);
            assert!(
                d.message.contains("idle_slope=300000000 bps"),
                "CBS shaped diagnostic must announce idle slope 300 Mbps: {}",
                d.message
            );
        }

        // No `WcttDeferred` — CBS dispatch supersedes deferral.
        let deferred: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttDeferred"))
            .collect();
        assert!(
            deferred.is_empty(),
            "expected no WcttDeferred when CBS reservation is set: {:#?}",
            deferred
        );

        // One WcttBound per stream.
        let bounds: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttBound"))
            .collect();
        assert_eq!(
            bounds.len(),
            2,
            "expected one WcttBound per stream: {:#?}",
            diags
        );
    }

    // ── Test 12: bus without Switch_Type is invisible to wctt ───────
    #[test]
    fn unannotated_bus_skipped() {
        // A regular AADL bus carrying no Spar_Network properties must
        // not appear as a stream hop. A connection bound to such a
        // bus produces zero `Wctt*` diagnostics — bus_bandwidth still
        // analyses it, wctt does not.
        let src = r#"
package Net
public

  bus plain
  end plain;
  bus implementation plain.impl
  end plain.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      pb : bus plain.impl;
      a  : device d.impl;
      b  : device d.impl;
    connections
      c_pb_a : bus access pb -> a.net;
      c_pb_b : bus access pb -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (pb));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);
        assert_eq!(
            count_wctt(&diags),
            0,
            "unswitched bus must not produce Wctt* diagnostics: {:#?}",
            diags
        );
    }

    // ── Test 11: TAS service curve dispatch (v0.8.1 commit 2) ───────
    #[test]
    fn tsn_with_gcl_and_cos_dispatches_tas_service_curve() {
        // 1 Gbps TSN switch carrying a Gate_Control_List that opens
        // CoS 7 for 50% of the cycle; the source device declares
        // Class_of_Service=7. Expect a `WcttTasGated` Info diagnostic
        // (not `WcttDeferred`) and a finite `WcttBound` whose value
        // reflects the half-bandwidth, gate-latency-shifted service
        // curve.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Gate_Control_List      => "0:5000:0x80;5000:5000:0x7F";
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device src_d
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);

        // We must see a WcttTasGated Info, not WcttDeferred.
        let tas_gated = diags
            .iter()
            .find(|d| d.message.starts_with("WcttTasGated"))
            .unwrap_or_else(|| panic!("expected WcttTasGated: {:#?}", diags));
        assert!(tas_gated.severity == Severity::Info);
        assert!(tas_gated.message.contains("CoS 7"));
        assert!(tas_gated.message.contains("50%"));

        let deferred = diags.iter().find(|d| d.message.starts_with("WcttDeferred"));
        assert!(
            deferred.is_none(),
            "WcttDeferred should not fire when GCL+CoS are present: {:#?}",
            diags
        );

        // Bound: D = T_K + σ/R_K. With T_K = 5 us, σ = 1500 bytes,
        // R_K = 500 Mbps: σ/R_K = 1500·8·10^12 / 500·10^6 = 24·10^6 ps =
        // 24 us. Total = 29 us.
        let bound = diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap_or_else(|| panic!("expected WcttBound: {:#?}", diags));
        assert!(
            bound.message.contains("41144000 ps"),
            "expected 41.144 us TAS bound (29 us gated + 12.144 us quantization), got: {}",
            bound.message
        );
    }

    // ── Test 12: TAS bound is strictly larger than ungated ─────────
    #[test]
    fn tas_gated_bound_exceeds_ungated_bound_at_half_bandwidth() {
        // Comparison: a 1 Gbps line-rate, ungated, gives D = 12 us
        // (single-hop test 2 above). Same topology under TAS with 50%
        // open and 5 us gate latency gives D = 5 us (latency) + 24 us
        // (σ/R_K at 500 Mbps) = 29 us. The TAS bound must be strictly
        // larger than the ungated bound: gate shaping is always at
        // least as restrictive as line-rate service.
        let ungated_src = r#"
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

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let ungated = instantiate(ungated_src, "Net", "Sys", "impl");
        let ungated_diags = WcttAnalysis.analyze(&ungated);
        let ungated_bound = ungated_diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap();
        // v0.9.1 soundness: 12 µs (NC) + 12.144 µs (atomic-frame
        // quantization, 1518 B at 1 Gbps) = 24.144 µs.
        assert!(ungated_bound.message.contains("24144000 ps"));

        // Same model, but with TSN+GCL applied (50% open for CoS 7).
        // The bound must exceed the ungated 12 us.
        let gated_src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Gate_Control_List      => "0:5000:0x80;5000:5000:0x7F";
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device src_d
    features
      net : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end src_d;
  device implementation src_d.impl
  end src_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device src_d.impl;
      b  : device d.impl;
    connections
      c_sw_a : bus access sw -> a.net;
      c_sw_b : bus access sw -> b.net;
      data1  : port a.out_p -> b.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let gated = instantiate(gated_src, "Net", "Sys", "impl");
        let gated_diags = WcttAnalysis.analyze(&gated);
        let gated_bound = gated_diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound"))
            .unwrap();
        // v0.9.1 soundness: 29 µs gated NC + 12.144 µs quantization = 41.144 µs.
        assert!(gated_bound.message.contains("41144000 ps"));

        // Strictly: 41.144 µs > 24.144 µs — the gated bound is more
        // pessimistic (and correctly so) than the ungated line-rate bound.
    }

    // ── Test 13: TSN switch without GCL still defers ────────────────
    #[test]
    fn tsn_switch_without_gcl_keeps_deferred_path() {
        // TSN switch but no Gate_Control_List declared on the bus.
        // The dispatch must fall back to the v0.8.0 WcttDeferred path
        // (no TAS service curve; no per-hop contribution).
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);
        let deferred = diags
            .iter()
            .find(|d| d.message.starts_with("WcttDeferred"))
            .unwrap_or_else(|| panic!("expected WcttDeferred: {:#?}", diags));
        assert!(deferred.severity == Severity::Info);
        assert!(
            diags.iter().all(|d| !d.message.starts_with("WcttTasGated")),
            "WcttTasGated should not fire when GCL is absent: {:#?}",
            diags
        );
    }

    // ── Test 14: TSN switch with GCL but stream lacks CoS defers ────
    #[test]
    fn tsn_with_gcl_but_no_stream_cos_still_defers() {
        // Bus has a Gate_Control_List, but the source device does not
        // declare Spar_TSN::Class_of_Service. Without the CoS we
        // cannot pick a window mask; fall back to WcttDeferred.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 1000000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Gate_Control_List      => "0:5000:0x80;5000:5000:0x7F";
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      a  : device d.impl;
      b  : device d.impl;
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
        let diags = WcttAnalysis.analyze(&inst);
        let deferred = diags
            .iter()
            .find(|d| d.message.starts_with("WcttDeferred"))
            .unwrap_or_else(|| panic!("expected WcttDeferred: {:#?}", diags));
        assert!(deferred.severity == Severity::Info);
    }

    // ── Test 15: TSN preemption shrinks the per-hop blocking term ────
    #[test]
    fn tsn_preemption_emits_applied_diagnostic_and_computes_bound() {
        // 1-switch TSN network. Bus = 100 Mbps with
        // `Frame_Preemption => true`; one express stream
        // (`Frame_Preemption => true` on the source) and one
        // preemptable stream (no `Frame_Preemption`, CoS = 0).
        // The express stream must:
        //   - get a `WcttPreemptionApplied` Info diagnostic
        //   - get a finite `WcttBound` (no `WcttDeferred`)
        // The preemptable stream stays opaque (`WcttDeferred`).
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 100000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Frame_Preemption       => true;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device d
    features
      net : requires bus access;
      out_p : out data port;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  device express_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Frame_Preemption => true;
  end express_d;
  device implementation express_d.impl
  end express_d.impl;

  device pre_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 0;
  end pre_d;
  device implementation pre_d.impl
  end pre_d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      ex : device express_d.impl;
      pr : device pre_d.impl;
      ex_dst : device d.impl;
      pr_dst : device d.impl;
    connections
      c_sw_ex     : bus access sw -> ex.net;
      c_sw_pr     : bus access sw -> pr.net;
      c_sw_ex_dst : bus access sw -> ex_dst.net;
      c_sw_pr_dst : bus access sw -> pr_dst.net;
      data_ex     : port ex.out_p -> ex_dst.in_p;
      data_pr     : port pr.out_p -> pr_dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        // The express stream must produce a `WcttPreemptionApplied`
        // Info diagnostic.
        let applied: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttPreemptionApplied"))
            .collect();
        assert_eq!(
            applied.len(),
            1,
            "expected exactly one WcttPreemptionApplied for the express \
             stream: {:#?}",
            diags
        );
        assert!(applied[0].severity == Severity::Info);
        // Numbers reported in nanoseconds in the diagnostic message.
        // Legacy: 1518 B · 8 / 100 Mbps = 121_440 ns.
        // With preemption: 68 B · 8 / 100 Mbps = 5_440 ns.
        assert!(
            applied[0].message.contains("121440 ns"),
            "expected legacy blocking 121440 ns: {}",
            applied[0].message
        );
        assert!(
            applied[0].message.contains("5440 ns"),
            "expected preempted blocking 5440 ns: {}",
            applied[0].message
        );

        // The express stream gets a `WcttBound` (computed bound).
        let express_bound = diags
            .iter()
            .find(|d| d.message.starts_with("WcttBound") && d.message.contains("data_ex"))
            .unwrap_or_else(|| panic!("expected WcttBound for data_ex: {:#?}", diags));
        assert!(express_bound.severity == Severity::Info);

        // The preemptable stream still falls through to `WcttDeferred`.
        let deferred = diags
            .iter()
            .find(|d| d.message.starts_with("WcttDeferred") && d.message.contains("data_pr"))
            .unwrap_or_else(|| panic!("expected WcttDeferred for data_pr: {:#?}", diags));
        assert!(deferred.severity == Severity::Info);
    }

    // ── Test 16: bus without Frame_Preemption keeps deferred path ────
    #[test]
    fn tsn_without_bus_preemption_keeps_deferred() {
        // Even a stream that declares `Frame_Preemption => true` on its
        // source falls through to `WcttDeferred` when the bus has not
        // opted in (`Frame_Preemption` unset on the bus). This matches
        // the c4 spec's "explicit-then-default order" — both sides must
        // explicitly say yes for the small fragment-blocking term to
        // apply, and a missing bus property is the safe `false` default.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 100000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device express_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Frame_Preemption => true;
  end express_d;
  device implementation express_d.impl
  end express_d.impl;

  device d
    features
      net   : requires bus access;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      ex : device express_d.impl;
      ex_dst : device d.impl;
    connections
      c_sw_ex     : bus access sw -> ex.net;
      c_sw_ex_dst : bus access sw -> ex_dst.net;
      data_ex     : port ex.out_p -> ex_dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        // No `WcttPreemptionApplied` because the bus is silent.
        let applied: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttPreemptionApplied"))
            .collect();
        assert!(
            applied.is_empty(),
            "WcttPreemptionApplied must not fire when the bus omits \
             Frame_Preemption: {:#?}",
            diags
        );
        // `WcttDeferred` fires instead.
        assert!(
            diags.iter().any(|d| d.message.starts_with("WcttDeferred")),
            "expected WcttDeferred when bus has no Frame_Preemption: {:#?}",
            diags
        );
    }

    // ── Test 17: stream without Frame_Preemption keeps deferred ─────
    #[test]
    fn tsn_with_low_cos_stream_keeps_deferred() {
        // Bus declares `Frame_Preemption => true` but the source
        // stream is preemptable (CoS = 0). No `WcttPreemptionApplied`
        // is emitted; the stream falls through to `WcttDeferred`.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 100000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Frame_Preemption       => true;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device pre_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 0;
  end pre_d;
  device implementation pre_d.impl
  end pre_d.impl;

  device d
    features
      net   : requires bus access;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      pr : device pre_d.impl;
      pr_dst : device d.impl;
    connections
      c_sw_pr     : bus access sw -> pr.net;
      c_sw_pr_dst : bus access sw -> pr_dst.net;
      data_pr     : port pr.out_p -> pr_dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);

        let applied: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttPreemptionApplied"))
            .collect();
        assert!(
            applied.is_empty(),
            "WcttPreemptionApplied must not fire for non-express \
             streams: {:#?}",
            diags
        );
        let deferred: Vec<&AnalysisDiagnostic> = diags
            .iter()
            .filter(|d| d.message.starts_with("WcttDeferred"))
            .collect();
        assert!(
            !deferred.is_empty(),
            "expected WcttDeferred for non-express stream: {:#?}",
            diags
        );
    }

    // ── Test 18: high-CoS stream is express by default ──────────────
    #[test]
    fn tsn_high_cos_stream_is_express_by_default() {
        // No `Frame_Preemption` set on the source, but `Class_of_Service
        // => 7` makes it express via the default heuristic. With the
        // bus also declaring `Frame_Preemption => true`, preemption
        // applies and `WcttPreemptionApplied` is emitted.
        let src = r#"
package Net
public

  bus eth
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Output_Rate        => 100000000 bitsps;
      Spar_Network::Forwarding_Latency => 0 us .. 0 us;
      Spar_Network::Queue_Depth        => 1;
      Spar_TSN::Frame_Preemption       => true;
  end eth;
  bus implementation eth.impl
  end eth.impl;

  device cos_d
    features
      net   : requires bus access;
      out_p : out data port;
    properties
      Spar_TSN::Class_of_Service => 7;
  end cos_d;
  device implementation cos_d.impl
  end cos_d.impl;

  device d
    features
      net   : requires bus access;
      in_p  : in data port;
  end d;
  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      sw : bus eth.impl;
      hi : device cos_d.impl;
      hi_dst : device d.impl;
    connections
      c_sw_hi     : bus access sw -> hi.net;
      c_sw_hi_dst : bus access sw -> hi_dst.net;
      data_hi     : port hi.out_p -> hi_dst.in_p;
    properties
      Deployment_Properties::Actual_Connection_Binding => (reference (sw));
  end Sys.impl;
end Net;
"#;
        let inst = instantiate(src, "Net", "Sys", "impl");
        let diags = WcttAnalysis.analyze(&inst);
        assert!(
            diags
                .iter()
                .any(|d| d.message.starts_with("WcttPreemptionApplied")),
            "high-CoS stream must opt into preemption by default: {:#?}",
            diags
        );
    }
}
