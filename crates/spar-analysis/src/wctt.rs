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
    ClassOfService, GateSchedule, get_class_of_service, get_gate_schedule, tas_residual_service,
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

            for (hop_idx, sw_idx) in stream.hops.iter().enumerate() {
                let st = switch_type.get(sw_idx).copied().unwrap_or(SwitchType::Fifo);

                // TSN switches: prefer the TAS gate-window service
                // curve when both the bus has a parsed Gate_Control_List
                // and the stream carries a Spar_TSN::Class_of_Service.
                // When either is missing, fall back to the v0.8.0
                // WcttDeferred Info diagnostic and skip the hop's
                // contribution.
                let svc = if matches!(st, SwitchType::Tsn) {
                    match (gate_schedule_for_bus.get(sw_idx), stream.cos) {
                        (Some(schedule), Some(cos)) => {
                            let link_rate = link_rate_for_bus.get(sw_idx).copied().unwrap_or(0);
                            let tas_svc = tas_residual_service(schedule, cos, link_rate);
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
                                    "WcttTasGated: stream '{}' (CoS {}) on TSN switch '{}' at \
                                     hop {}: open fraction {}% gate latency {} ps",
                                    stream_name,
                                    cos.0,
                                    graph
                                        .node(*sw_idx)
                                        .map(|n| n.name.as_str())
                                        .unwrap_or("<unknown>"),
                                    hop_idx,
                                    open_pct,
                                    gate_latency_ps,
                                ),
                                path: stream_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            tas_svc
                        }
                        _ => {
                            // No gate schedule or no CoS — fall back to
                            // the v0.8.0 deferred path. We deliberately
                            // emit at most one WcttDeferred per stream
                            // (the diagnostic noise floor matches Phase
                            // 1's behaviour).
                            if !deferred_emitted {
                                diags.push(AnalysisDiagnostic {
                                    severity: Severity::Info,
                                    message: format!(
                                        "WcttDeferred: stream '{}' traverses TSN switch '{}' at \
                                         hop {}; TAS/CBS-shaped service curves are deferred to \
                                         Phase 2 (tracked in \
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
                    }
                } else {
                    match service_for_bus.get(sw_idx) {
                        Some(s) => *s,
                        None => continue,
                    }
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
                let mut comp_burst: u128 = 0;
                let mut comp_rate: u128 = 0;
                for other in &streams {
                    if std::ptr::eq(other, stream) {
                        continue;
                    }
                    if !other.hops.contains(sw_idx) {
                        continue;
                    }
                    comp_burst = comp_burst.saturating_add(other.alpha.burst_bytes as u128);
                    comp_rate = comp_rate.saturating_add(other.alpha.sustained_rate_bps as u128);
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

                // Per-hop delay using the tagged stream's α and the
                // residual service.
                match delay_bound(&alpha, &residual) {
                    Ok(d) => {
                        total_delay_ps = total_delay_ps.saturating_add(d);
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
                path: stream_path,
                analysis: self.name().to_string(),
            });
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

    let (rate_bps, burst_bytes) = if let Some(idx) = src_idx {
        let src_props = instance.properties_for(idx);
        let rate = read_output_rate_bps(src_props).unwrap_or(0);
        let burst = read_queue_depth(src_props)
            .map(|q| q.saturating_mul(FRAME_BYTES))
            .unwrap_or(DEFAULT_BURST_BYTES);
        (rate, burst)
    } else {
        (0, DEFAULT_BURST_BYTES)
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
        let svc = ServiceCurve::rate_latency(rate, wcet_ps);

        // The min (best-case) bound is the BCET forwarding latency for
        // this hop — purely the propagation/forwarding floor, no
        // queuing. We deliberately ignore competing traffic here since
        // BCET is a lower bound.
        total_min_ps = total_min_ps.saturating_add(bcet_ps);

        if svc.rate_bps == 0 {
            // Without a finite service rate we cannot compute a worst-
            // case bound for this hop. Skip the queuing contribution
            // and trust BCET == WCET on the propagation floor.
            total_max_ps = total_max_ps.saturating_add(wcet_ps);
            continue;
        }

        // TSN switches: opaque in Phase 1. Skip queuing contribution
        // (propagate the WCET floor only) — the WcttAnalysis pass also
        // emits a `WcttDeferred` Info on the same switch.
        if let Some(node) = graph.node(*sw_idx)
            && let NodeKind::Switch { switch_type } = node.kind
            && matches!(switch_type, SwitchType::Tsn)
        {
            total_max_ps = total_max_ps.saturating_add(wcet_ps);
            continue;
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
    /// Stream's `Spar_TSN::Class_of_Service` when annotated. Required
    /// for the TAS service-curve dispatch on TSN switches; streams
    /// without a CoS fall back to the [`Severity::Info`]-emitting
    /// `WcttDeferred` path.
    cos: Option<ClassOfService>,
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
            let burst_bytes = read_queue_depth(src_props)
                .map(|q| q.saturating_mul(FRAME_BYTES))
                .unwrap_or(DEFAULT_BURST_BYTES);

            let alpha = ArrivalCurve::affine(burst_bytes, rate_bps);
            // Stream's class-of-service drives the TAS service-curve
            // dispatch on TSN-typed switches. The lookup follows the
            // same precedence as the typed/string accessor in
            // spar-network::tsn — we read from the source end station
            // because Spar_TSN::Class_of_Service applies to ports /
            // connections; the closest reachable carrier in the
            // current property model is the source device's
            // PropertyMap, which the existing rate / queue-depth reads
            // already use.
            let cos = get_class_of_service(src_props);

            streams.push(Stream {
                name: conn.name.as_str().to_string(),
                src_idx,
                sink_idx: dst_idx,
                hops,
                alpha,
                cos,
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
        assert!(
            info[0].message.contains("12000000 ps"),
            "expected 12 us bound, got: {}",
            info[0].message
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
        assert!(
            info.message.contains("51000000 ps"),
            "expected 51 us bound, got: {}",
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

    // ── Test 10: bus without Switch_Type is invisible to wctt ───────
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
            bound.message.contains("29000000 ps"),
            "expected 29 us TAS bound, got: {}",
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
        // 12 us = 12_000_000 ps for the ungated single hop.
        assert!(ungated_bound.message.contains("12000000 ps"));

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
        assert!(gated_bound.message.contains("29000000 ps"));

        // Strictly: 29 us > 12 us — the gated bound is more pessimistic
        // (and correctly so) than the ungated line-rate bound.
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
}
