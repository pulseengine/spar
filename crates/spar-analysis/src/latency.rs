//! End-to-end flow latency analysis.
//!
//! Computes latency bounds for end-to-end flows in the instance model.
//! For each E2E flow, traces through the flow segments and computes
//! best-case and worst-case latency based on execution times and
//! sampling delays (periods) at connection crossings.
//!
//! Follows the OSATE flow latency analysis approach:
//! - Best case: sum of execution times only
//! - Worst case: sum of execution times + sum of periods (sampling delays)
//!
//! # WCET + WCTT alternation (Track D, v0.8.0)
//!
//! When a connection segment crosses a switched bus annotated with
//! `Spar_Network::*`, the per-hop contribution comes from the WCTT
//! analysis (NC-derived bound on the bus) instead of the sampling-delay
//! placeholder. The chain therefore alternates RTA-derived WCET on
//! compute hops (thread-to-thread on the same processor) with
//! WCTT-derived bounds on network hops (connections bound to a switched
//! bus). Models with no `Spar_Network::*` annotations fall back to the
//! existing scalar `Bus_Properties::Latency` / `Transmission_Time` path
//! and produce byte-identical output to v0.7.0.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, FlowSegment};

use crate::property_accessors::{
    ProcessorGroups, get_execution_time, get_execution_time_range, get_latency_budget,
    get_processor_binding, get_timing_property,
};
use crate::wctt::compute_network_hop_latency;
use crate::{Analysis, AnalysisDiagnostic, Severity, component_path};

/// End-to-end flow latency analysis.
pub struct LatencyAnalysis;

impl Analysis for LatencyAnalysis {
    fn name(&self) -> &str {
        "latency"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Warning — component without timing properties, worst-case latency exceeds bound
        //   Info    — end-to-end flow latency range, modal awareness note
        let mut diags = Vec::new();
        let mut groups = ProcessorGroups::default();

        for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
            let owner_path = component_path(instance, e2e.owner);

            if e2e.segments.is_empty() {
                continue; // flow_check already reports this
            }
            if e2e.nested_segment().is_some() {
                continue; // flow_check rejects it: not AADL, would resolve to the wrong component
            }

            // Walk segments: odd indices are connections, even indices are
            // subcomponent flow references ("subcomp.flow_name")
            let mut best_case_ps: u64 = 0;
            let mut worst_case_ps: u64 = 0;
            // Provenance tracking (spar#455). A latency aggregated from no
            // timing is not a derived value, and reporting it as INFO
            // `[0.000 ms .. 0.000 ms]` lets a declared budget look satisfied by
            // a number the analysis never computed. The figure is derived if a
            // positive quantity was added (`worst_case_ps > 0`) OR an execution
            // time was read — `Compute_Execution_Time` is the one input that
            // may be legitimately zero (`0 ns`), so a zero worst-case with an
            // exec present is a derived zero, not an absence, and must stay
            // reportable. Every other contributor (network-hop bound, sampling
            // delay, inter-processor overhead) is positive in practice, so
            // `worst_case_ps > 0` already covers it.
            let mut saw_exec = false;
            let mut missing_timing = Vec::new();
            let mut connection_count: u64 = 0;
            let mut prev_processor: Option<String> = None;

            // Read inter-processor overhead from the owner (applied at each
            // processor boundary crossing — only when the connection is
            // *not* served by a Spar_Network-annotated switched bus).
            let owner_props_for_overhead = instance.properties_for(e2e.owner);
            let inter_proc_overhead_ps = get_inter_processor_overhead(owner_props_for_overhead);

            // Track per-chain hop typology for the LatencyHopMixed marker.
            // We deliberately count compute hops only on chains that *also*
            // see a network hop, because in the no-network case the
            // existing v0.7.0 traces don't distinguish hop kinds — keeping
            // the chain-level diagnostic byte-identical when no
            // `Spar_Network::*` is set is the v0.7.0 non-regression
            // contract.
            let mut network_hops: Vec<NetworkHopAnnotation> = Vec::new();
            let mut compute_hops: Vec<ComputeHopAnnotation> = Vec::new();
            let mut chain_unservable = false;
            // The last network hop's worst-case bound (picoseconds) — used
            // to gate the legacy sampling-delay / inter-processor-overhead
            // code paths on the *next* component segment so we never
            // double-count the network contribution.
            let mut suppress_next_legacy_overhead = false;

            // Expand the flow into resolved steps, descending into the flow
            // IMPLEMENTATION of any `sub.flow` that has one
            // (REQ-ANALYSIS-FLOW-THREAD-DESCENT-001).
            let steps = expand_flow_steps(instance, e2e.owner, &e2e.segments);

            for step in &steps {
                if let FlowStep::Conn { ctx, name } = step {
                    // Connection segment.
                    connection_count += 1;
                    let conn_name = name.as_str();

                    // Try the WCTT-derived bound first. When the
                    // connection has no `Actual_Connection_Binding` to a
                    // Spar_Network-annotated switched bus, this returns
                    // None and we fall through to the legacy
                    // sampling-delay + inter-processor-overhead path on
                    // the next component segment.
                    if let Some(hop_lat) = compute_network_hop_latency(instance, *ctx, conn_name) {
                        if hop_lat.unservable {
                            // Mirror wctt.rs's `WcttUnservable` Error and
                            // stop aggregating this chain — we cannot
                            // honestly compose a finite end-to-end bound
                            // when one of the network hops is starved.
                            diags.push(AnalysisDiagnostic {
                                severity: Severity::Error,
                                message: format!(
                                    "end-to-end flow '{}': network hop '{}' is unservable \
                                     (competing flows saturate the residual service); \
                                     latency aggregation aborted [network hop]",
                                    e2e.name, conn_name,
                                ),
                                path: owner_path.clone(),
                                analysis: self.name().to_string(),
                            });
                            chain_unservable = true;
                            break;
                        }
                        best_case_ps = best_case_ps.saturating_add(hop_lat.min_ps);
                        worst_case_ps = worst_case_ps.saturating_add(hop_lat.max_ps);
                        network_hops.push(NetworkHopAnnotation {
                            connection_name: conn_name.to_string(),
                            min_ps: hop_lat.min_ps,
                            max_ps: hop_lat.max_ps,
                        });
                        suppress_next_legacy_overhead = true;
                    } else {
                        suppress_next_legacy_overhead = false;
                    }

                    continue;
                }

                let FlowStep::Comp {
                    idx: child,
                    name: subcomp_name,
                } = step
                else {
                    continue;
                };

                if let Some(child_idx) = *child {
                    let child_comp = instance.component(child_idx);
                    let child_props = instance.properties_for(child_idx);

                    // Execution-time contribution: the BEST case takes the
                    // range's MINIMUM, the worst case its maximum
                    // (REQ-ANALYSIS-LATENCY-BEST-CASE-001). This used to add
                    // `get_execution_time` — the worst case — to BOTH, so a
                    // flow of two `20 ms .. 40 ms` components reported
                    // `[80 .. 80]` instead of `[40 .. 80]`: a lower bound that
                    // was really an upper bound, which no below-minimum check
                    // could be built on.
                    //
                    // The worst case is unchanged bit for bit — both accessors
                    // read the same property and parse the maximum identically
                    // — so every upper-bound and budget comparison is untouched.
                    // If a range's MINIMUM fails to parse, fall back to the
                    // worst case for both ends rather than lose an execution
                    // time this flow has today.
                    let exec_range = get_execution_time_range(child_props)
                        .or_else(|| get_execution_time(child_props).map(|w| (w, w)));
                    // Get period for sampling delay
                    let period_ps = get_timing_property(child_props, "Period");

                    if let Some((bcet, wcet)) = exec_range {
                        best_case_ps = best_case_ps.saturating_add(bcet);
                        worst_case_ps = worst_case_ps.saturating_add(wcet);
                        saw_exec = true;
                    } else {
                        missing_timing.push(child_comp.name.as_str().to_string());
                    }

                    // Add sampling delay for connections after the first
                    // component, *unless* the preceding connection was
                    // already accounted for by a WCTT bound (network hop):
                    // adding the sampling period on top of the NC bound
                    // would double-count the queuing component.
                    if connection_count > 0
                        && !suppress_next_legacy_overhead
                        && let Some(period) = period_ps
                    {
                        worst_case_ps = worst_case_ps.saturating_add(period);
                    }

                    // Track processor binding for inter-processor overhead.
                    // Keyed by the processor the binding RESOLVES to, so two
                    // spellings of one processor are no crossing and two
                    // processors sharing a name are one (#498).
                    let cur_binding = get_processor_binding(child_props)
                        .map(|b| groups.key(instance, child_idx, Some(&b)));
                    let mut crossed_boundary = false;
                    if let Some(ref cur) = cur_binding
                        && let Some(ref prev) = prev_processor
                        && !cur.eq_ignore_ascii_case(prev)
                    {
                        crossed_boundary = true;
                        // Processor boundary crossing — add overhead only
                        // when we did *not* already account for the
                        // crossing through a WCTT-derived network hop.
                        if !suppress_next_legacy_overhead
                            && let Some(overhead) = inter_proc_overhead_ps
                        {
                            worst_case_ps = worst_case_ps.saturating_add(overhead);
                        }
                    }

                    // Compute-hop annotation: a flow segment whose
                    // component shares a processor with its predecessor
                    // (or is the first segment with a binding). Only
                    // tracked here for `LatencyHopMixed` — the per-hop
                    // diagnostic is emitted only on chains that also see
                    // a network hop.
                    if !crossed_boundary && prev_processor.is_some() {
                        compute_hops.push(ComputeHopAnnotation {
                            component_name: child_comp.name.as_str().to_string(),
                            // the WORST case, exactly as before this change
                            exec_ps: exec_range.map(|(_, wcet)| wcet).unwrap_or(0),
                        });
                    }

                    if cur_binding.is_some() {
                        prev_processor = cur_binding;
                    }
                } else {
                    missing_timing.push(subcomp_name.clone());
                }

                // Reset the WCTT-suppression latch now that the next
                // component has consumed it.
                suppress_next_legacy_overhead = false;
            }

            if chain_unservable {
                continue;
            }

            // Report missing timing properties
            if !missing_timing.is_empty() {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "end-to-end flow '{}': components without timing properties: {}",
                        e2e.name,
                        missing_timing.join(", ")
                    ),
                    path: owner_path.clone(),
                    analysis: self.name().to_string(),
                });
            }

            // Provenance (spar#455): a figure built from no timing was never
            // derived, so do not render it as a latency range. Report it as an
            // Error — "not computed" — rather than an INFO carrying `0.000`,
            // which a reader (or a budget check) would otherwise take for a
            // satisfied bound. Nothing was derived when the worst case is still
            // zero AND no execution time was read (an exec may legitimately be
            // `0 ns`). The `missing timing properties` warning above already
            // names which components lacked timing.
            if worst_case_ps == 0 && !saw_exec {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "end-to-end flow '{}' latency: not computed — no element on \
                         the flow carries timing properties, so no latency figure \
                         could be derived",
                        e2e.name,
                    ),
                    path: owner_path.clone(),
                    analysis: self.name().to_string(),
                });
                continue;
            }

            // Report latency range
            let best_ms = best_case_ps as f64 / 1_000_000_000.0;
            let worst_ms = worst_case_ps as f64 / 1_000_000_000.0;

            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "end-to-end flow '{}' latency: [{:.3} ms .. {:.3} ms]",
                    e2e.name, best_ms, worst_ms,
                ),
                path: owner_path.clone(),
                analysis: self.name().to_string(),
            });

            // Per-hop diagnostics and the mixed-hop marker only fire on
            // chains that exercised at least one network hop. Without a
            // network hop the chain is byte-identical to v0.7.0.
            if !network_hops.is_empty() {
                for hop in &network_hops {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "end-to-end flow '{}': connection '{}' [network hop] [{:.3} ms .. {:.3} ms]",
                            e2e.name,
                            hop.connection_name,
                            hop.min_ps as f64 / 1_000_000_000.0,
                            hop.max_ps as f64 / 1_000_000_000.0,
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
                for hop in &compute_hops {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "end-to-end flow '{}': component '{}' [compute hop] {:.3} ms",
                            e2e.name,
                            hop.component_name,
                            hop.exec_ps as f64 / 1_000_000_000.0,
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }

                if !compute_hops.is_empty() {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!(
                            "LatencyHopMixed: end-to-end flow '{}' alternates RTA-derived \
                             WCET on {} compute hop{} with WCTT-derived bounds on {} \
                             network hop{}",
                            e2e.name,
                            compute_hops.len(),
                            if compute_hops.len() == 1 { "" } else { "s" },
                            network_hops.len(),
                            if network_hops.len() == 1 { "" } else { "s" },
                        ),
                        path: owner_path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Compare against the declared budget (#455). The flow's OWN
            // `{ Latency => .. }` association is read first — that is where
            // AADL puts a flow's budget — and the OWNER component's `Latency`
            // only when the flow declares none, which keeps models written
            // against the old owner-only lookup working.
            //
            // ERROR, not Warning: a budget breach that exits 0 is a budget
            // nobody can gate on, the same defect REQ-CLI-ANALYZE-EXITCODE-001
            // fixed one layer down. This is a deliberate behaviour change for
            // models that are over budget today and were told so only at
            // Warning.
            //
            // BOTH ENDS. The minimum became checkable once `best_case_ps`
            // became a real best case (REQ-ANALYSIS-LATENCY-BEST-CASE-001);
            // before that it summed WORST-case execution times and a minimum
            // check over it would have stayed silent on real violations.
            // `best_case_ps` is a LOWER bound on any realised latency, so
            // `best >= min` GUARANTEES the minimum is met, while `best < min`
            // means the flow CAN complete faster than its budget allows. A
            // scalar budget has no minimum and is never checked below.
            //
            // `>` / `<`, not `>=` / `<=`: a flow that lands exactly on a
            // bound meets it.
            let owner_props = instance.properties_for(e2e.owner);
            let budget =
                get_latency_budget(&e2e.properties).or_else(|| get_latency_budget(owner_props));
            if let Some(budget) = budget
                && let Some(min_ps) = budget.min_ps
                && best_case_ps < min_ps
            {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "end-to-end flow '{}' best-case latency {:.3} ms is below the declared \
                         minimum {:.3} ms — the flow can complete faster than its budget allows",
                        e2e.name,
                        best_ms,
                        min_ps as f64 / 1_000_000_000.0,
                    ),
                    path: owner_path.clone(),
                    analysis: self.name().to_string(),
                });
            }
            if let Some(budget) = budget
                && worst_case_ps > budget.max_ps
            {
                let bound_ms = budget.max_ps as f64 / 1_000_000_000.0;
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Error,
                    message: format!(
                        "end-to-end flow '{}' worst-case latency {:.3} ms exceeds bound {:.3} ms",
                        e2e.name, worst_ms, bound_ms,
                    ),
                    path: owner_path,
                    analysis: self.name().to_string(),
                });
            }
        }

        // STPA-REQ-016: Multi-processor awareness — check if E2E flows cross
        // processor boundaries without communication overhead properties.
        // When a flow path crosses a processor boundary, inter-processor
        // communication adds latency that may not be captured by the basic
        // execution_time + sampling_delay model.
        let has_multi_processors = {
            let proc_count = instance
                .all_components()
                .filter(|(_, c)| {
                    matches!(
                        c.category,
                        ComponentCategory::Processor | ComponentCategory::VirtualProcessor
                    )
                })
                .count();
            proc_count >= 2
        };

        if has_multi_processors {
            for (_e2e_idx, e2e) in instance.end_to_end_flows.iter() {
                let owner_path = component_path(instance, e2e.owner);

                // Collect processor bindings for each flow segment component
                let mut prev_binding: Option<String> = None;
                let mut crosses_boundary = false;

                for step in expand_flow_steps(instance, e2e.owner, &e2e.segments) {
                    // Component steps only; connections carry no binding. The
                    // expansion is the one the latency walk above used, so a
                    // crossing it applied is the crossing this note reports.
                    if let FlowStep::Comp {
                        idx: Some(child_idx),
                        ..
                    } = step
                    {
                        let child_props = instance.properties_for(child_idx);
                        // The same key as the latency walk above (#498).
                        let binding = get_processor_binding(child_props)
                            .map(|b| groups.key(instance, child_idx, Some(&b)));

                        if let Some(ref cur) = binding
                            && let Some(ref prev) = prev_binding
                            && !cur.eq_ignore_ascii_case(prev)
                        {
                            crosses_boundary = true;
                        }
                        if binding.is_some() {
                            prev_binding = binding;
                        }
                    }
                }

                if crosses_boundary {
                    // Check if the owner has a communication overhead property
                    let owner_props = instance.properties_for(e2e.owner);
                    let has_comm_overhead = owner_props
                        .get("Timing_Properties", "Transmission_Time")
                        .is_some()
                        || owner_props.get("", "Transmission_Time").is_some()
                        || owner_props
                            .get("SPAR_Properties", "Inter_Processor_Overhead")
                            .is_some()
                        || owner_props.get("", "Inter_Processor_Overhead").is_some();

                    if !has_comm_overhead {
                        diags.push(AnalysisDiagnostic {
                            severity: Severity::Info,
                            message: format!(
                                "end-to-end flow '{}' crosses processor boundaries but no \
                                 Transmission_Time or Inter_Processor_Overhead property is set; \
                                 latency estimate may understate actual inter-processor \
                                 communication delay",
                                e2e.name,
                            ),
                            path: owner_path,
                            analysis: self.name().to_string(),
                        });
                    }
                }
            }
        }

        // STPA-REQ-017: Note modal awareness
        if !instance.system_operation_modes.is_empty() {
            diags.push(AnalysisDiagnostic {
                severity: Severity::Info,
                message: format!(
                    "{} analysis used default property values; {} system operation mode(s) exist but modal property evaluation is not yet fully supported",
                    self.name(),
                    instance.system_operation_modes.len()
                ),
                path: vec!["root".to_string()],
                analysis: self.name().to_string(),
            });
        }

        diags
    }
}

/// Deepest chain of `sub.flow` -> flow implementation -> `sub.flow` descents
/// followed. Descent only ever moves to a CHILD, so the instance hierarchy
/// already bounds it (a cycle is impossible); this is a defensive ceiling
/// against a pathologically deep model, past which a flow is read as a leaf.
const MAX_FLOW_DESCENT_DEPTH: usize = 16;

/// One resolved element of an end-to-end flow, after descent.
#[derive(Debug, PartialEq, Eq)]
enum FlowStep {
    /// A connection, looked up in `ctx` (the component whose scope declares it:
    /// the flow's owner, or the component whose flow implementation named it).
    Conn {
        ctx: ComponentInstanceIdx,
        name: String,
    },
    /// A component whose own timing is read; `idx` is `None` when the name
    /// resolves to no child.
    Comp {
        idx: Option<ComponentInstanceIdx>,
        name: String,
    },
}

fn find_child(
    instance: &SystemInstance,
    ctx: ComponentInstanceIdx,
    name: &str,
) -> Option<ComponentInstanceIdx> {
    instance.component(ctx).children.iter().copied().find(|&c| {
        instance
            .component(c)
            .name
            .as_str()
            .eq_ignore_ascii_case(name)
    })
}

/// Segments of `comp`'s implementation of flow spec `flow`, if it has one.
fn flow_impl_segments<'a>(
    instance: &'a SystemInstance,
    comp: ComponentInstanceIdx,
    flow: &str,
) -> Option<&'a [FlowSegment]> {
    instance
        .component(comp)
        .flows
        .iter()
        .map(|&f| &instance.flow_instances[f])
        .find(|f| f.name.as_str().eq_ignore_ascii_case(flow))
        .and_then(|f| f.impl_segments.as_deref())
}

/// Flatten an end-to-end flow's segments (alternating `sub.flow` / connection)
/// into steps. A `sub.flow` whose child carries an implementation of `flow` is
/// replaced by that implementation's steps, resolved in the CHILD's scope; the
/// child's own timing is then not read (its threads' is). A child with no
/// implementation of the flow, or whose implementation names no component, is a
/// leaf and keeps the pre-descent behaviour.
fn expand_flow_steps(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    segments: &[spar_hir_def::name::Name],
) -> Vec<FlowStep> {
    let mut out = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        if i % 2 == 1 {
            out.push(FlowStep::Conn {
                ctx: owner,
                name: seg.as_str().to_string(),
            });
        } else {
            push_comp_step(instance, owner, seg.as_str(), 0, &mut out);
        }
    }
    out
}

/// The components an end-to-end flow passes through, in order, resolved exactly
/// as `LatencyAnalysis` resolves them: a `sub.flow` segment whose child carries
/// an implementation of `flow` is replaced by the components that
/// implementation names (descending), anything else contributes the child
/// itself. Segments that resolve to no child are omitted.
///
/// Callers must first reject a flow with [`EndToEndFlowInstance::nested_segment`]
/// (latency skips such a flow); on one this would resolve the wrong component.
///
/// [`EndToEndFlowInstance::nested_segment`]: spar_hir_def::instance::EndToEndFlowInstance::nested_segment
pub fn flow_components(
    instance: &SystemInstance,
    e2e: &spar_hir_def::instance::EndToEndFlowInstance,
) -> Vec<ComponentInstanceIdx> {
    expand_flow_steps(instance, e2e.owner, &e2e.segments)
        .into_iter()
        .filter_map(|s| match s {
            FlowStep::Comp { idx, .. } => idx,
            FlowStep::Conn { .. } => None,
        })
        .collect()
}

fn push_comp_step(
    instance: &SystemInstance,
    ctx: ComponentInstanceIdx,
    seg: &str,
    depth: usize,
    out: &mut Vec<FlowStep>,
) {
    let (sub, flow) = match seg.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (seg, None),
    };
    let child = find_child(instance, ctx, sub);
    if let (Some(child_idx), Some(flow)) = (child, flow)
        && depth < MAX_FLOW_DESCENT_DEPTH
        && let Some(segs) = flow_impl_segments(instance, child_idx, flow)
    {
        let before = out.len();
        let last = segs.len().saturating_sub(1);
        for (k, fs) in segs.iter().enumerate() {
            match &fs.subcomponent {
                Some(inner) => push_comp_step(
                    instance,
                    child_idx,
                    &format!("{}.{}", inner, fs.element),
                    depth + 1,
                    out,
                ),
                None => {
                    // An unqualified element is the component's own feature at
                    // either end (`pin`, `pout`), contributing nothing; in the
                    // interior it is a connection of the child. The
                    // connection touching an own feature is the inner leg of
                    // the connection the enclosing flow already counted
                    // (`c1 : s.o -> app.pin` and `c_in : pin -> caller.i` are
                    // one semantic hop), so it is not counted a second time.
                    let is_end = k == 0 || k == last;
                    let boundary = (k == 1 && segs[0].subcomponent.is_none())
                        || (k + 1 == last && segs[last].subcomponent.is_none());
                    if !is_end && !boundary {
                        out.push(FlowStep::Conn {
                            ctx: child_idx,
                            name: fs.element.as_str().to_string(),
                        });
                    }
                }
            }
        }
        if out[before..]
            .iter()
            .any(|s| matches!(s, FlowStep::Comp { .. }))
        {
            return;
        }
        out.truncate(before);
    }
    out.push(FlowStep::Comp {
        idx: child,
        name: sub.to_string(),
    });
}

/// Per-hop bookkeeping for chains that mix WCET and WCTT contributions.
#[derive(Debug, Clone)]
struct NetworkHopAnnotation {
    connection_name: String,
    min_ps: u64,
    max_ps: u64,
}

#[derive(Debug, Clone)]
struct ComputeHopAnnotation {
    component_name: String,
    exec_ps: u64,
}

/// Read inter-processor communication overhead in picoseconds.
///
/// Checks `SPAR_Properties::Inter_Processor_Overhead`,
/// `Timing_Properties::Inter_Processor_Overhead` (unqualified fallback),
/// then `Timing_Properties::Transmission_Time`. Returns `None` when none
/// of these properties are set.
fn get_inter_processor_overhead(props: &spar_hir_def::properties::PropertyMap) -> Option<u64> {
    use crate::property_accessors::extract_time_ps;
    use spar_hir_def::property_value::parse_time_value;

    // Try SPAR_Properties::Inter_Processor_Overhead (typed path).
    if let Some(expr) = props
        .get_typed("SPAR_Properties", "Inter_Processor_Overhead")
        .or_else(|| props.get_typed("", "Inter_Processor_Overhead"))
        && let Some(ps) = extract_time_ps(expr)
    {
        return Some(ps);
    }

    // String fallback for Inter_Processor_Overhead.
    if let Some(raw) = props
        .get("SPAR_Properties", "Inter_Processor_Overhead")
        .or_else(|| props.get("", "Inter_Processor_Overhead"))
        && let Some(ps) = parse_time_value(raw)
    {
        return Some(ps);
    }

    // Try Timing_Properties::Inter_Processor_Overhead.
    if let Some(val) = get_timing_property(props, "Inter_Processor_Overhead") {
        return Some(val);
    }

    // Fall back to Transmission_Time.
    get_timing_property(props, "Transmission_Time")
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::*;
    use spar_hir_def::name::{Name, PropertyRef};
    use spar_hir_def::properties::{PropertyMap, PropertyValue};

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
        flow_instances: Arena<FlowInstance>,
        end_to_end_flows: Arena<EndToEndFlowInstance>,
        property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
                flow_instances: Arena::default(),
                end_to_end_flows: Arena::default(),
                property_maps: FxHashMap::default(),
            }
        }

        fn add_component(
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

        fn set_children(
            &mut self,
            parent: ComponentInstanceIdx,
            children: Vec<ComponentInstanceIdx>,
        ) {
            self.components[parent].children = children;
        }

        fn add_connection_inst(&mut self, name: &str, owner: ComponentInstanceIdx) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: None,
                dst: None,
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
        }

        /// Add a connection instance with named source/destination
        /// subcomponent endpoints — needed by the WCTT integration tests
        /// because `compute_network_hop_latency` walks the connection's
        /// `src` / `dst` to identify the source end station.
        fn add_connection_with_endpoints(
            &mut self,
            name: &str,
            owner: ComponentInstanceIdx,
            src_sub: &str,
            src_feat: &str,
            dst_sub: &str,
            dst_feat: &str,
        ) {
            let idx = self.connections.alloc(ConnectionInstance {
                name: Name::new(name),
                kind: ConnectionKind::Port,
                is_bidirectional: false,
                owner,
                src: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(src_sub)),
                    feature: Name::new(src_feat),
                }),
                dst: Some(ConnectionEnd {
                    subcomponent: Some(Name::new(dst_sub)),
                    feature: Name::new(dst_feat),
                }),
                in_modes: Vec::new(),
            });
            self.components[owner].connections.push(idx);
        }

        fn add_e2e(&mut self, name: &str, owner: ComponentInstanceIdx, segments: Vec<&str>) {
            self.end_to_end_flows.alloc(EndToEndFlowInstance {
                name: Name::new(name),
                owner,
                segments: segments.into_iter().map(Name::new).collect(),
                properties: Default::default(),
            });
        }

        /// Put a property on an END-TO-END FLOW itself — the
        /// `{ Latency => .. }` block — rather than on a component.
        fn set_e2e_property(&mut self, flow: &str, set: &str, name: &str, value: &str) {
            let (_, f) = self
                .end_to_end_flows
                .iter_mut()
                .find(|(_, f)| f.name.as_str() == flow)
                .unwrap_or_else(|| panic!("no flow {flow}"));
            f.properties.add(PropertyValue {
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

        fn set_property(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
            let map = self.property_maps.entry(comp).or_default();
            map.add(PropertyValue {
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
                features: self.features,
                connections: self.connections,
                flow_instances: self.flow_instances,
                end_to_end_flows: self.end_to_end_flows,
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

    #[test]
    fn latency_simple_flow_best_worst_case() {
        // sensor -> c1 -> controller -> c2 -> actuator
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor, ctrl, actuator]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);

        // E2E flow: sensor.src -> c1 -> controller.pass -> c2 -> actuator.sink
        b.add_e2e(
            "e2e_control",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );

        // Set timing properties
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        b.set_property(
            actuator,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(actuator, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report one latency range: {:?}",
            diags
        );

        // Best case: 1 + 2 + 1 = 4 ms
        assert!(
            infos[0].message.contains("4.000 ms"),
            "best case should be 4ms: {}",
            infos[0].message
        );

        // Worst case: 4 ms exec + 20 ms (controller period after c1) + 10 ms (actuator period after c2) = 34 ms
        assert!(
            infos[0].message.contains("34.000 ms"),
            "worst case should be 34ms: {}",
            infos[0].message
        );
    }

    /// #455 item 3: a flow that reaches inside a subcomponent
    /// (`app.caller.lock_flow`) is rejected by flow_check, so latency must
    /// say NOTHING about it. Before, only the first dotted part was kept and
    /// both thread references resolved to process `app`, giving the doubled
    /// `components without timing properties: app, app` warning and a
    /// `not computed` Error about components the flow never named. A legal
    /// flow on the same instance is still reported, so this cannot pass by
    /// the analysis going quiet altogether.
    #[test]
    fn latency_skips_a_flow_reaching_inside_a_subcomponent() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let app = b.add_component("app", ComponentCategory::Process, Some(root));
        let caller = b.add_component("caller", ComponentCategory::Thread, Some(app));
        let consumer = b.add_component("consumer", ComponentCategory::Thread, Some(app));
        let src = b.add_component("src", ComponentCategory::Device, Some(root));
        let dst = b.add_component("dst", ComponentCategory::Device, Some(root));
        b.set_children(app, vec![caller, consumer]);
        b.set_children(root, vec![app, src, dst]);
        b.add_connection_inst("c1", root);
        for (c, t) in [
            (caller, "5 ms .. 10 ms"),
            (consumer, "7 ms .. 12 ms"),
            (src, "1 ms"),
            (dst, "2 ms"),
        ] {
            b.set_property(c, "Timing_Properties", "Compute_Execution_Time", t);
        }
        b.add_e2e(
            "nested",
            root,
            vec![
                "app.caller.lock_flow",
                "app.c_hand",
                "app.consumer.consumer_flow",
            ],
        );
        b.add_e2e("legal", root, vec!["src.f", "c1", "dst.f"]);

        let diags = LatencyAnalysis.analyze(&b.build(root));

        assert!(
            diags.iter().all(|d| !d.message.contains("'nested'")),
            "latency must not report on a rejected flow: {diags:?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("'legal'") && d.message.contains("3.000 ms")),
            "the legal flow must still be reported: {diags:?}"
        );
    }

    #[test]
    fn latency_flow_missing_timing_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // Only set properties on sensor, not controller
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("without timing"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "should warn about missing timing: {:?}",
            diags
        );
        assert!(
            warnings[0].message.contains("controller"),
            "warning should mention controller: {}",
            warnings[0].message
        );
    }

    /// spar#455 kill-criterion: an end-to-end flow on which NO element carries
    /// any timing must not report a derived latency of `[0.000 ms .. 0.000 ms]`
    /// as INFO. With no contribution the figure was never computed, so it is an
    /// Error ("not computed"), never an INFO carrying a zero a budget check
    /// would read as satisfied.
    ///
    /// Non-vacuity: under the pre-fix behaviour this flow produced exactly that
    /// INFO and no Error, so this test fails against the old code — it is the
    /// bug's own repro.
    #[test]
    fn latency_zero_contribution_flow_errors_not_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let src = b.add_component("src", ComponentCategory::Device, Some(root));
        let dst = b.add_component("dst", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![src, dst]);
        b.add_connection_inst("c1", root);

        // A well-formed flow, but not one component on it declares any timing.
        b.add_e2e("req_to_done", root, vec!["src.out", "c1", "dst.in"]);

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // No derived latency range may be reported.
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency: ["))
            .collect();
        assert!(
            infos.is_empty(),
            "a figure derived from zero contributions must not be reported as \
             an INFO latency range: {diags:?}"
        );

        // Instead there is an Error stating nothing could be derived.
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("not computed"))
            .collect();
        assert_eq!(
            errors.len(),
            1,
            "a flow with no timing must raise a 'not computed' error: {diags:?}"
        );
        assert!(
            errors[0].message.contains("req_to_done"),
            "the error must name the flow: {}",
            errors[0].message
        );
    }

    /// Discrimination / non-vacuity counterpart to the kill-criterion test: a
    /// flow whose elements declare `0 ns` execution is a legitimately-zero
    /// latency, derived from real contributions. It stays reportable as INFO
    /// `[0.000 ms .. 0.000 ms]` and must NOT be turned into an error. This is
    /// why the fix counts contributions rather than testing whether the sum is
    /// zero — a check on the sum alone would mis-fire here, and this test would
    /// catch that regression.
    #[test]
    fn latency_explicit_zero_execution_is_reported_not_errored() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let src = b.add_component("src", ComponentCategory::Device, Some(root));
        let dst = b.add_component("dst", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![src, dst]);
        b.add_connection_inst("c1", root);
        b.add_e2e("zero_flow", root, vec!["src.out", "c1", "dst.in"]);

        // Both ends declare a real, explicitly-zero execution time.
        b.set_property(src, "Timing_Properties", "Compute_Execution_Time", "0 ns");
        b.set_property(dst, "Timing_Properties", "Compute_Execution_Time", "0 ns");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency: ["))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "a legitimately-zero derived latency must still be reported: {diags:?}"
        );
        assert!(
            infos[0].message.contains("0.000 ms .. 0.000 ms"),
            "the derived figure is zero: {}",
            infos[0].message
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("not computed"))
            .collect();
        assert!(
            errors.is_empty(),
            "a derived zero is not a 'not computed' error: {diags:?}"
        );
    }

    /// Locks the discriminator on a NON-execution contribution: a flow whose
    /// only timing is a sampling delay (a `Period`, no `Compute_Execution_Time`
    /// anywhere) is still derived from a real timing quantity, so it is
    /// reported, not errored. This guards against a future change that counted
    /// only execution times as contributions — the "not computed" error must
    /// key on all four contribution kinds, not just WCET.
    #[test]
    fn latency_period_only_flow_is_reported_not_errored() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let src = b.add_component("src", ComponentCategory::Device, Some(root));
        let dst = b.add_component("dst", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![src, dst]);
        b.add_connection_inst("c1", root);
        b.add_e2e("period_flow", root, vec!["src.out", "c1", "dst.in"]);

        // Only a Period is declared — a sampling delay, but no execution time.
        // The sampling delay applies to `dst` because it follows a connection.
        b.set_property(dst, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency: ["))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "a flow whose only timing is a sampling delay is still derived: {diags:?}"
        );
        assert!(
            infos[0].message.contains("10.000 ms"),
            "the sampling delay must feed the worst case: {}",
            infos[0].message
        );

        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("not computed"))
            .collect();
        assert!(
            errors.is_empty(),
            "a figure derived from a sampling delay is not 'not computed': {diags:?}"
        );
    }

    #[test]
    fn latency_exceeds_bound_warns() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set a latency bound of 10ms on the root (owner of E2E flow)
        b.set_property(root, "Timing_Properties", "Latency", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Worst case: 5 + 5 exec + 20 sampling = 30ms > 10ms bound
        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("exceeds bound"))
            .collect();
        assert_eq!(
            bound_warns.len(),
            1,
            "should warn about exceeding bound: {:?}",
            diags
        );
    }

    #[test]
    fn latency_no_e2e_flows_no_diags() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);
        assert!(
            diags.is_empty(),
            "no E2E flows should produce no diagnostics: {:?}",
            diags
        );
    }

    // ── STPA-REQ-016: Inter-processor communication overhead ─────

    #[test]
    fn cross_processor_flow_without_overhead_info() {
        // STPA-REQ-016: E2E flow crossing processor boundary without overhead prop
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert_eq!(
            cross_proc.len(),
            1,
            "should info about cross-processor flow: {:?}",
            diags
        );
        assert_eq!(cross_proc[0].severity, Severity::Info);
        assert!(
            cross_proc[0].message.contains("Inter_Processor_Overhead"),
            "should mention overhead property: {}",
            cross_proc[0].message
        );
    }

    #[test]
    fn same_processor_flow_no_overhead_info() {
        // No inter-processor advisory when all on same processor
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_same",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)", // Same processor
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert!(
            cross_proc.is_empty(),
            "same-processor flow should not trigger: {:?}",
            cross_proc
        );
    }

    #[test]
    fn cross_processor_flow_with_overhead_no_info() {
        // STPA-REQ-016: No info when overhead property is set
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![cpu1, cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Set transmission time on owner to indicate overhead is accounted for
        b.set_property(root, "Timing_Properties", "Transmission_Time", "1 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let cross_proc: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("processor boundaries"))
            .collect();
        assert!(
            cross_proc.is_empty(),
            "overhead property set — no advisory: {:?}",
            cross_proc
        );
    }

    #[test]
    fn latency_single_component_flow() {
        // E2E flow with a single segment (no connections)
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "2 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency"))
            .collect();
        assert_eq!(infos.len(), 1);
        // Single component, no connections: best = worst = 2ms
        assert!(
            infos[0].message.contains("2.000 ms"),
            "should be 2ms: {}",
            infos[0].message
        );
    }

    // ── Boundary tests for latency bound checking ─────────────────

    #[test]
    fn latency_exactly_at_bound_no_warning() {
        // Worst-case latency exactly equals bound — should NOT warn
        // Kills `>` → `>=` mutant on `worst_case_ps > latency_bound`
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // sensor: exec=3ms, period=10ms; controller: exec=2ms, period=20ms
        // Worst case: 3 + 2 exec + 20 sampling (controller after c1) = 25ms
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "3 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set bound exactly equal to worst case: 25ms
        b.set_property(root, "Timing_Properties", "Latency", "25 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("exceeds bound"))
            .collect();
        assert!(
            bound_warns.is_empty(),
            "latency exactly at bound should NOT warn (only > bound): {:?}",
            bound_warns
        );
    }

    #[test]
    fn latency_one_unit_over_bound_warns() {
        // Worst-case latency is 1ms over bound — should warn.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        // sensor: exec=3ms, period=10ms; controller: exec=2ms, period=20ms
        // Worst case: 3 + 2 exec + 20 sampling = 25ms
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "3 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");

        // Set bound 1ms under worst case: 24ms < 25ms
        b.set_property(root, "Timing_Properties", "Latency", "24 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("exceeds bound"))
            .collect();
        assert_eq!(
            bound_warns.len(),
            1,
            "latency 1ms over bound should warn: {:?}",
            diags
        );
        assert!(
            bound_warns[0].message.contains("25.000 ms"),
            "should show worst-case latency: {}",
            bound_warns[0].message
        );
        assert!(
            bound_warns[0].message.contains("24.000 ms"),
            "should show bound: {}",
            bound_warns[0].message
        );
    }

    #[test]
    fn latency_sampling_delay_formula() {
        // Verify that sampling delay is added for connections AFTER the first component.
        // 3-component flow: A -> c1 -> B -> c2 -> C
        // Best case = exec_A + exec_B + exec_C
        // Worst case = exec_A + exec_B + period_B + exec_C + period_C
        // (period_B added because c1 is before B, period_C because c2 is before C)
        // Sensor (first component) does NOT get sampling delay.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let a = b.add_component("comp_a", ComponentCategory::Device, Some(root));
        let bb = b.add_component("comp_b", ComponentCategory::Thread, Some(root));
        let c = b.add_component("comp_c", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![a, bb, c]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);

        b.add_e2e(
            "e2e_abc",
            root,
            vec!["comp_a.src", "c1", "comp_b.pass", "c2", "comp_c.sink"],
        );

        // A: exec=2ms, period=5ms
        b.set_property(a, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(a, "Timing_Properties", "Period", "5 ms");

        // B: exec=3ms, period=10ms
        b.set_property(bb, "Timing_Properties", "Compute_Execution_Time", "3 ms");
        b.set_property(bb, "Timing_Properties", "Period", "10 ms");

        // C: exec=1ms, period=8ms
        b.set_property(c, "Timing_Properties", "Compute_Execution_Time", "1 ms");
        b.set_property(c, "Timing_Properties", "Period", "8 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Best case: 2 + 3 + 1 = 6ms
        // Worst case: 2 + 3 + 10 (B sampling) + 1 + 8 (C sampling) = 24ms
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "should report one latency range: {:?}",
            diags
        );
        assert!(
            infos[0].message.contains("6.000 ms"),
            "best case should be 6ms: {}",
            infos[0].message
        );
        assert!(
            infos[0].message.contains("24.000 ms"),
            "worst case should be 24ms (exec + sampling for B and C): {}",
            infos[0].message
        );
    }

    #[test]
    fn latency_within_bound_no_warning() {
        // Worst-case latency well under bound — should NOT warn
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");

        // Bound = 100ms, worst case = 1ms — no warning
        b.set_property(root, "Timing_Properties", "Latency", "100 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let bound_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("exceeds bound"))
            .collect();
        assert!(
            bound_warns.is_empty(),
            "latency well within bound should not warn: {:?}",
            bound_warns
        );
    }

    // ── #455 / REQ-ANALYSIS-LATENCY-BUDGET-001 ──────────────────────────
    //
    // Same fixture as `latency_exceeds_bound_warns`: worst case = 5 + 5 ms
    // execution + 20 ms sampling = 30 ms, best case (a sum of WCETs) = 10 ms.
    // Each test varies ONE dimension of the budget declaration, so a mutant of
    // that dimension is attributable to exactly one test.
    fn budget_fixture(set: &str, value: Option<&str>) -> Vec<AnalysisDiagnostic> {
        budget_fixture_with_flow(set, value, None)
    }

    /// As `budget_fixture`, plus an optional budget on the FLOW's own block.
    fn budget_fixture_with_flow(
        set: &str,
        owner_value: Option<&str>,
        flow_value: Option<&str>,
    ) -> Vec<AnalysisDiagnostic> {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);
        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "5 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        if let Some(v) = owner_value {
            b.set_property(root, set, "Latency", v);
        }
        if let Some(v) = flow_value {
            b.set_e2e_property("e2e_flow", "", "Latency", v);
        }
        let inst = b.build(root);
        LatencyAnalysis.analyze(&inst)
    }

    fn breaches(d: &[AnalysisDiagnostic]) -> Vec<&AnalysisDiagnostic> {
        d.iter()
            .filter(|x| x.message.contains("exceeds bound"))
            .collect()
    }

    /// A RANGE budget is how `Latency` is declared — the standard types it
    /// `Time_Range` — and the previous scalar-only lookup read it as "no budget
    /// declared". #455's own `300 ns .. 250 us` could not have fired.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_range_budget_is_read_and_a_breach_is_an_error() {
        let d = budget_fixture("Timing_Properties", Some("1 ms .. 10 ms"));
        let b = breaches(&d);
        assert_eq!(
            b.len(),
            1,
            "30 ms against a 1..10 ms budget must breach: {d:#?}"
        );
        assert_eq!(
            b[0].severity,
            Severity::Error,
            "a breach that exits 0 is a budget nobody can gate on"
        );
        assert!(
            b[0].message.contains("exceeds bound 10.000 ms"),
            "{}",
            b[0].message
        );
    }

    /// The pair for the test above: without it, a build that flagged every
    /// ranged budget would pass.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_range_budget_that_is_met_is_silent() {
        let d = budget_fixture("Timing_Properties", Some("1 ms .. 100 ms"));
        assert!(
            breaches(&d).is_empty(),
            "30 ms meets a 1..100 ms budget: {d:#?}"
        );
    }

    /// `Latency` is a `Communication_Properties` property. The old lookup
    /// searched `Timing_Properties` and then the empty set, so the CORRECTLY
    /// qualified spelling was the one spelling that could never be found.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_budget_under_its_standard_property_set_is_found() {
        let d = budget_fixture("Communication_Properties", Some("10 ms"));
        assert_eq!(
            breaches(&d).len(),
            1,
            "qualified Latency must be read: {d:#?}"
        );
    }

    /// An absent budget is not a violation. Pinned because the cheapest wrong
    /// fix — defaulting a missing bound to zero — would flag every flow.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn an_absent_budget_is_not_a_violation() {
        let d = budget_fixture("Timing_Properties", None);
        assert!(
            breaches(&d).is_empty(),
            "no budget declared, nothing to breach: {d:#?}"
        );
    }

    /// Clause (b): a budget on the FLOW's own block — where AADL puts it, and
    /// exactly how #455 declared `300 ns .. 250 us` — is read. With no owner
    /// budget at all, only the flow's can have fired.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_budget_on_the_flow_itself_is_read() {
        let d = budget_fixture_with_flow("Timing_Properties", None, Some("1 ms .. 10 ms"));
        let b = breaches(&d);
        assert_eq!(
            b.len(),
            1,
            "30 ms against the flow's own 10 ms must breach: {d:#?}"
        );
        assert_eq!(b[0].severity, Severity::Error);
    }

    /// The flow's own budget WINS over the owner's — in both directions,
    /// because either one alone is passable by a wrong precedence. Here the
    /// flow is met and the owner would breach: reading the owner first fires.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn the_flows_own_budget_takes_precedence_when_it_is_met() {
        let d = budget_fixture_with_flow("Timing_Properties", Some("1 ms"), Some("1 ms .. 100 ms"));
        assert!(
            breaches(&d).is_empty(),
            "the flow's own 100 ms is met; the owner's 1 ms must not override it: {d:#?}"
        );
    }

    /// ...and here the flow breaches while the owner would be met: reading the
    /// owner first stays silent on a real violation.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn the_flows_own_budget_takes_precedence_when_it_is_breached() {
        let d = budget_fixture_with_flow("Timing_Properties", Some("500 ms"), Some("10 ms"));
        let b = breaches(&d);
        assert_eq!(b.len(), 1, "the flow's own 10 ms is breached: {d:#?}");
        assert!(
            b[0].message.contains("exceeds bound 10.000 ms"),
            "{}",
            b[0].message
        );
    }

    // ── REQ-ANALYSIS-LATENCY-BEST-CASE-001 ──────────────────────────────
    //
    // Before this, NO test used a RANGED execution time, so the whole suite
    // was blind to both ends of the reported range: a mutant that made the
    // WORST case take the minimum — an optimistic, unsafe bound on every model
    // with a ranged execution time — survived all 944 analysis tests.

    /// Two components, no Period (so no sampling term), each a ranged CET.
    fn range_fixture(cet: &str) -> Vec<AnalysisDiagnostic> {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![sensor, ctrl]);
        b.add_connection_inst("c1", root);
        b.add_e2e(
            "e2e_flow",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );
        b.set_property(sensor, "Timing_Properties", "Compute_Execution_Time", cet);
        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", cet);
        let inst = b.build(root);
        LatencyAnalysis.analyze(&inst)
    }

    fn reported_range(d: &[AnalysisDiagnostic]) -> String {
        d.iter()
            .find(|x| x.severity == Severity::Info && x.message.contains("latency: ["))
            .map(|x| x.message.clone())
            .unwrap_or_else(|| panic!("no derived latency reported: {d:#?}"))
    }

    /// The LOWER end is a best case: 20 + 20, not the 40 + 40 the old code
    /// summed. Without this the lower bound was an upper bound, and no
    /// below-minimum check could be built on it.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BEST-CASE-001
    #[test]
    fn a_ranged_execution_time_gives_a_real_best_case() {
        let r = reported_range(&range_fixture("20 ms .. 40 ms"));
        assert!(r.contains("[40.000 ms .."), "best case is 20 + 20 ms: {r}");
    }

    /// The UPPER end is untouched — this is the safety property of the change.
    /// Pinned separately because a mutant that made the worst case take the
    /// MINIMUM is optimistic, and it survived the entire suite before this.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BEST-CASE-001
    #[test]
    fn the_worst_case_is_still_the_sum_of_maxima() {
        let r = reported_range(&range_fixture("20 ms .. 40 ms"));
        assert!(r.contains(".. 80.000 ms]"), "worst case is 40 + 40 ms: {r}");
    }

    /// A range whose MINIMUM does not parse must not cost the flow its
    /// execution time. The old accessor read only the maximum and so accepted
    /// it; falling back to that keeps every model that computed a latency
    /// before still computing one — here at [40 .. 40], not "not computed".
    // rivet: verifies REQ-ANALYSIS-LATENCY-BEST-CASE-001
    #[test]
    fn a_malformed_minimum_does_not_lose_the_execution_time() {
        let d = range_fixture("garbage .. 20 ms");
        assert!(
            !d.iter().any(|x| x.message.contains("not computed")),
            "the maximum still parses, so the latency is still derivable: {d:#?}"
        );
        assert!(reported_range(&d).contains("[40.000 ms .. 40.000 ms]"));
    }

    fn below_minimum(d: &[AnalysisDiagnostic]) -> Vec<&AnalysisDiagnostic> {
        d.iter()
            .filter(|x| x.message.contains("below the declared minimum"))
            .collect()
    }

    /// The minimum is now enforced. It was withheld while `best_case_ps` was a
    /// sum of WORST-case execution times — a test pinned that no below-minimum
    /// diagnostic shipped until that changed, and it failed the moment this
    /// check was added, which is what it was for. Fixture: best case 5 + 5 =
    /// 10 ms against a declared minimum of 50 ms.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_flow_that_can_beat_its_declared_minimum_is_an_error() {
        let d = budget_fixture("Timing_Properties", Some("50 ms .. 100 ms"));
        let b = below_minimum(&d);
        assert_eq!(
            b.len(),
            1,
            "best case 10 ms is under the 50 ms minimum: {d:#?}"
        );
        assert_eq!(b[0].severity, Severity::Error);
        assert!(
            b[0].message.contains("minimum 50.000 ms"),
            "{}",
            b[0].message
        );
        assert!(
            breaches(&d).is_empty(),
            "worst case 30 ms is within 100 ms: {d:#?}"
        );
    }

    /// The boundary: a best case EXACTLY at the minimum meets it. Without this
    /// a `<=` would pass every other test.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_flow_exactly_at_its_declared_minimum_meets_it() {
        let d = budget_fixture("Timing_Properties", Some("10 ms .. 100 ms"));
        assert!(
            below_minimum(&d).is_empty(),
            "best case 10 ms meets a 10 ms minimum: {d:#?}"
        );
    }

    /// A scalar budget is an upper bound only and has NO minimum. Read as
    /// `min == max`, every flow under a scalar budget would fail its own
    /// "minimum" — here 10 ms against 100 ms.
    // rivet: verifies REQ-ANALYSIS-LATENCY-BUDGET-001
    #[test]
    fn a_scalar_budget_has_no_minimum_to_breach() {
        let d = budget_fixture("Timing_Properties", Some("100 ms"));
        assert!(
            below_minimum(&d).is_empty(),
            "a scalar budget sets no minimum: {d:#?}"
        );
    }
    #[test]
    fn latency_no_sampling_delay_for_first_component() {
        // Verify the first component in a flow does NOT get sampling delay added.
        // Single component flow: best = worst = exec only, no period added.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor]);

        b.add_e2e("simple", root, vec!["sensor.src"]);
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "100 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);
        // Both best and worst case should be 5ms (period NOT added for first component)
        assert!(
            infos[0].message.contains("[5.000 ms .. 5.000 ms]"),
            "first component should not get sampling delay: {}",
            infos[0].message
        );
    }

    // ── Inter-processor overhead applied to latency ────────────────

    #[test]
    fn inter_processor_overhead_added_to_worst_case() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Set inter-processor overhead on the owner.
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should report latency: {:?}", diags);

        // Best case: 1 + 2 = 3 ms (overhead not in best case)
        assert!(
            infos[0].message.contains("3.000 ms"),
            "best case should be 3ms: {}",
            infos[0].message
        );

        // Worst case: 1 + 2 exec + 20 sampling + 5 overhead = 28 ms
        assert!(
            infos[0].message.contains("28.000 ms"),
            "worst case should include 5ms overhead: {}",
            infos[0].message
        );
    }

    #[test]
    fn inter_processor_overhead_transmission_time_fallback() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Use Transmission_Time instead of Inter_Processor_Overhead.
        b.set_property(root, "Timing_Properties", "Transmission_Time", "3 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should report latency: {:?}", diags);

        // Worst case: 1 + 2 exec + 20 sampling + 3 overhead = 26 ms
        assert!(
            infos[0].message.contains("26.000 ms"),
            "worst case should include 3ms Transmission_Time: {}",
            infos[0].message
        );
    }

    #[test]
    fn no_overhead_when_same_processor() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_same",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);

        // Worst case without overhead: 1 + 2 exec + 20 sampling = 23 ms
        assert!(
            infos[0].message.contains("23.000 ms"),
            "same-processor flow should NOT include overhead: {}",
            infos[0].message
        );
    }

    #[test]
    fn no_overhead_property_no_overhead_added() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);

        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1);

        // Worst case without overhead: 1 + 2 exec + 20 sampling = 23 ms
        assert!(
            infos[0].message.contains("23.000 ms"),
            "no overhead property = no overhead in latency: {}",
            infos[0].message
        );
    }

    // ── Track D commit 6: WCTT integration tests ─────────────────────
    //
    // The next block of tests verifies that `LatencyAnalysis::analyze`
    // alternates RTA-derived WCET on compute hops with WCTT-derived
    // bounds on network hops, and that models without `Spar_Network::*`
    // annotations stay byte-identical to v0.7.0 (the non-regression
    // contract on which Track D Phase 1 stands).

    /// Build a compute-thread-on-cpu1 → connection → compute-thread-on-cpu2
    /// chain wrapping a single switched bus. Used by tests 3, 4, 6, 7.
    /// Parameters select whether the bus is annotated as a
    /// `Spar_Network::Switch_Type` switch and whether a
    /// `Bus_Properties::Latency` scalar fallback is set.
    fn build_two_hop_chain(annotate_switch: bool) -> SystemInstance {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        if annotate_switch {
            b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
            b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
            b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
            b.set_property(sw, "Spar_Network", "Queue_Depth", "1");

            // Owner-level connection binding: c1 binds to switch sw.
            b.set_property(
                root,
                "Deployment_Properties",
                "Actual_Connection_Binding",
                "(reference (sw))",
            );
        }

        b.build(root)
    }

    /// Test 1: non-regression — model without `Spar_Network::*` produces
    /// byte-identical latency output to current main.
    #[test]
    fn no_spar_network_models_unchanged_v07() {
        // Reuse the existing inter_processor_overhead model: cross-CPU
        // chain with a `SPAR_Properties::Inter_Processor_Overhead`
        // annotation but *no* `Spar_Network::Switch_Type`. Output must
        // include neither a `[network hop]` nor a `LatencyHopMixed`
        // marker — that is the byte-level non-regression contract.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sensor, ctrl]);
        b.add_connection_inst("c1", root);
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // No network-hop annotations and no LatencyHopMixed marker.
        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "non-regression: should not emit `[network hop]` without Spar_Network: {}",
                d.message
            );
            assert!(
                !d.message.contains("[compute hop]"),
                "non-regression: should not emit `[compute hop]` without Spar_Network: {}",
                d.message
            );
            assert!(
                !d.message.contains("LatencyHopMixed"),
                "non-regression: should not emit `LatencyHopMixed` without Spar_Network: {}",
                d.message
            );
        }

        // Existing v0.7.0 output is preserved.
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .collect();
        assert_eq!(infos.len(), 1, "should still report one chain: {:?}", diags);
        // 1 + 2 exec + 20 sampling + 5 overhead = 28 ms (matches the
        // existing `inter_processor_overhead_added_to_worst_case` test).
        assert!(
            infos[0].message.contains("28.000 ms"),
            "non-regression: worst case must still be 28ms: {}",
            infos[0].message
        );
    }

    /// Test 2: chain entirely on one CPU; no network hop. Even if other
    /// components in the model declare `Spar_Network::*`, a chain whose
    /// connection is unbound to any switched bus must not emit any
    /// network-hop annotation.
    #[test]
    fn compute_only_chain_unchanged() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        // A switched bus exists in the model but is *not* bound by c1.
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "e2e_same_cpu",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );

        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        // No Actual_Connection_Binding on owner — c1 is unbound to sw.

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "no binding to sw → no network hop: {}",
                d.message
            );
        }
        assert!(
            !diags.iter().any(|d| d.message.contains("LatencyHopMixed")),
            "no network hops → no LatencyHopMixed marker"
        );
    }

    /// Test 3: chain crosses a switched bus → the per-hop network bound
    /// matches what `wctt::compute_network_hop_latency` returns.
    #[test]
    fn single_network_hop_uses_wctt() {
        let inst = build_two_hop_chain(true);

        let diags = LatencyAnalysis.analyze(&inst);
        // Helper invocation result must equal the per-hop annotation
        // emitted by latency.rs.
        let owner = inst.root;
        let hop = compute_network_hop_latency(&inst, owner, "c1")
            .expect("c1 binds to switch sw → Some(NetworkHopLatency)");
        // Expected: forwarding latency 0 + σ/R, where σ = MTU = 1500 B,
        // R = 1 Gbps → 12_000_000 ps = 12 us = 0.012 ms.
        assert_eq!(hop.max_ps, 12_000_000, "12 us per-hop bound expected");
        assert!(
            !hop.unservable,
            "single-stream model is not unservable: {:?}",
            hop
        );

        let net_hop_diag = diags
            .iter()
            .find(|d| d.message.contains("[network hop]"))
            .unwrap_or_else(|| panic!("expected [network hop] diag, got: {:?}", diags));
        // 12 us = 0.012 ms — formatted with 3 decimals.
        assert!(
            net_hop_diag.message.contains("0.012 ms"),
            "[network hop] must report 0.012 ms WCTT bound: {}",
            net_hop_diag.message
        );
    }

    /// Test 4: three-stage chain demonstrating WCET+WCTT alternation.
    /// sensor (cpu1) → switched-bus connection → controller (cpu2) →
    /// switched-bus connection → actuator (cpu2). Both connections
    /// inherit the owner-level `Actual_Connection_Binding` to `sw`
    /// (canonical AADL behaviour — bindings inherit unless overridden
    /// per-connection), so both are network hops; actuator on the
    /// same CPU as controller is a compute hop. We therefore expect
    /// The `[compute hop]` figure is a WORST case, and must stay one after
    /// REQ-ANALYSIS-LATENCY-BEST-CASE-001 split the range into its two ends.
    ///
    /// Pinned because, before this, the printed millisecond value had NEVER
    /// been asserted — `compute_then_network_then_compute` checks only the hop
    /// COUNT, and every compute-hop fixture used a scalar execution time where
    /// minimum and maximum coincide. A mutant reporting the hop's MINIMUM
    /// survived the whole suite. Same chain as that test, but the compute hop
    /// (the actuator, sharing cpu2 with the controller) runs `1 ms .. 3 ms`.
    #[test]
    fn a_compute_hop_reports_its_worst_case_execution_time() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl, actuator]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_connection_with_endpoints("c2", root, "controller", "out_p", "actuator", "in_p");
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );
        for (comp, cpu, cet) in [
            (sensor, "cpu1", "1 ms"),
            (ctrl, "cpu2", "1 ms"),
            (actuator, "cpu2", "1 ms .. 3 ms"),
        ] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", cet);
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
            b.set_property(
                comp,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({})", cpu),
            );
        }
        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );
        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);
        let hop = diags
            .iter()
            .find(|d| d.message.contains("'actuator' [compute hop]"))
            .unwrap_or_else(|| panic!("expected the actuator compute hop: {diags:#?}"));
        assert!(
            hop.message.contains("[compute hop] 3.000 ms"),
            "a compute hop is charged its WORST case, 3 ms, not its 1 ms minimum: {}",
            hop.message
        );
    }

    /// `2 network hop` + `1 compute hop` in the LatencyHopMixed marker.
    /// This is the canonical "WCET+WCTT alternation is live" model.
    #[test]
    fn compute_then_network_then_compute() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl, actuator]);

        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_connection_with_endpoints("c2", root, "controller", "out_p", "actuator", "in_p");
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );

        for (comp, cpu) in [(sensor, "cpu1"), (ctrl, "cpu2"), (actuator, "cpu2")] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
            b.set_property(
                comp,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({})", cpu),
            );
        }

        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Chain has both kinds of hops → LatencyHopMixed Info emitted.
        let mixed = diags
            .iter()
            .find(|d| d.message.contains("LatencyHopMixed"))
            .unwrap_or_else(|| panic!("expected LatencyHopMixed: {:?}", diags));
        assert_eq!(mixed.severity, Severity::Info);
        assert!(
            mixed.message.contains("1 compute hop"),
            "actuator shares cpu2 with controller = 1 compute hop: {}",
            mixed.message
        );
        assert!(
            mixed.message.contains("2 network hops"),
            "c1 + c2 inherit owner-level binding to sw = 2 network hops: {}",
            mixed.message
        );
    }

    /// Test 5: bus has `Bus_Properties::Latency`-style scalar but no
    /// `Spar_Network::*` annotations → fallback to legacy scalar
    /// (Inter_Processor_Overhead) preserves v0.7.0 behaviour.
    #[test]
    fn network_hop_falls_back_to_bus_latency_when_no_switch_type() {
        // Smoke-check the helper: an unannotated `build_two_hop_chain`
        // must not produce any network-hop annotation, and the chain
        // must fall through to the legacy v0.7.0 path.
        let inst_smoke = build_two_hop_chain(false /* unannotated */);
        let smoke = LatencyAnalysis.analyze(&inst_smoke);
        assert!(
            smoke.iter().all(|d| !d.message.contains("[network hop]")),
            "unannotated bus must not produce a [network hop] diag: {:?}",
            smoke
        );

        // Now build the cross-CPU chain again with the legacy
        // `SPAR_Properties::Inter_Processor_Overhead` scalar set, which
        // is the "scalar Bus_Properties::Latency placeholder" the
        // integration design refers to. Verify the worst-case bound
        // reproduces the v0.7.0 number (28 ms — same as the existing
        // `inter_processor_overhead_added_to_worst_case` test).
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "e2e_cross",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );
        // Legacy scalar fallback — applied because there's no
        // Spar_Network::Switch_Type on `sw`.
        b.set_property(root, "SPAR_Properties", "Inter_Processor_Overhead", "5 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        // Same number as `inter_processor_overhead_added_to_worst_case`.
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        assert!(
            info.message.contains("28.000 ms"),
            "fallback to scalar overhead: 1+2 exec + 20 sampling + 5 overhead = 28ms, got: {}",
            info.message
        );
        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "no Spar_Network::Switch_Type → no network hop: {}",
                d.message
            );
        }
    }

    /// Test 6: same model as test 5 but with `Spar_Network::Switch_Type`
    /// added → the bound becomes NC-derived (12 us) instead of the
    /// scalar `Inter_Processor_Overhead` (5 ms).
    #[test]
    fn network_hop_uses_wctt_when_switch_type_present() {
        let inst = build_two_hop_chain(true);

        let diags = LatencyAnalysis.analyze(&inst);
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        // 1 + 2 exec + 0.012 ms WCTT + 0 sampling delay (suppressed
        // because the connection consumed the WCTT bound) = 3.012 ms.
        assert!(
            info.message.contains("3.012 ms"),
            "WCTT-derived bound: 1+2 exec + 0.012 ms WCTT = 3.012 ms, got: {}",
            info.message
        );
        let net_hop = diags
            .iter()
            .find(|d| d.message.contains("[network hop]"))
            .unwrap();
        assert!(net_hop.message.contains("0.012 ms"));
    }

    /// Test 7: chain with both hop types → `LatencyHopMixed` Info appears.
    #[test]
    fn latency_hop_mixed_diagnostic_emitted() {
        // Same fixture as compute_then_network_then_compute. Verify
        // explicitly that the LatencyHopMixed string is the unique
        // marker for chains that exercise both hop types.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let a = b.add_component("a", ComponentCategory::Thread, Some(root));
        let bb = b.add_component("b", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, sw, sensor, a, bb]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "a", "in_p");
        b.add_connection_with_endpoints("c2", root, "a", "out_p", "b", "in_p");
        b.add_e2e(
            "mixed",
            root,
            vec!["sensor.src", "c1", "a.pass", "c2", "b.sink"],
        );

        for (comp, cpu) in [(sensor, "cpu1"), (a, "cpu2"), (bb, "cpu2")] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
            b.set_property(
                comp,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({})", cpu),
            );
        }
        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let mixed: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("LatencyHopMixed"))
            .collect();
        assert_eq!(mixed.len(), 1, "exactly one LatencyHopMixed: {:?}", diags);
        assert_eq!(mixed[0].severity, Severity::Info);
    }

    /// Test 8: `compute_network_hop_latency` returns `unservable = true`
    /// → `latency.rs` emits an error and stops aggregating.
    #[test]
    fn wctt_unservable_propagates_to_latency() {
        // Two streams whose sustained rates each equal the server rate
        // (so the residual service for each is exhausted by the other).
        // The wctt unit tests cover the same model under the name
        // `competing_flow_exceeds_rate_emits_unservable`.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let sw = b.add_component("sw", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let other_src = b.add_component("hot_src", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let other_dst = b.add_component("hot_dst", ComponentCategory::Device, Some(root));
        b.set_children(
            root,
            vec![_cpu1, _cpu2, sw, sensor, other_src, ctrl, other_dst],
        );
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_connection_with_endpoints("c2", root, "hot_src", "out_p", "hot_dst", "in_p");
        b.add_e2e(
            "saturated",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );

        for comp in [sensor, ctrl, other_src, other_dst] {
            b.set_property(comp, "Timing_Properties", "Compute_Execution_Time", "1 ms");
            b.set_property(comp, "Timing_Properties", "Period", "10 ms");
        }
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );

        // Saturate the 1 kbps switch with two 1 kbps sources.
        b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        b.set_property(sw, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        b.set_property(sensor, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(sensor, "Spar_Network", "Queue_Depth", "1");
        b.set_property(other_src, "Spar_Network", "Output_Rate", "1000 bitsps");
        b.set_property(other_src, "Spar_Network", "Queue_Depth", "1");
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let err = diags
            .iter()
            .find(|d| d.severity == Severity::Error && d.message.contains("unservable"))
            .unwrap_or_else(|| panic!("expected unservable Error: {:?}", diags));
        assert!(
            err.message.contains("[network hop]"),
            "unservable error annotates the offending hop: {}",
            err.message
        );
        // Aggregation aborted — no chain-level "latency: [..]" Info for
        // the saturated chain.
        let info_count = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .count();
        assert_eq!(
            info_count, 0,
            "unservable chain must not emit a latency range: {:?}",
            diags
        );
    }

    /// Test 9: middle hop has no `Spar_Network::*` annotations →
    /// fall back to scalar without crashing.
    #[test]
    fn unannotated_bus_in_chain_falls_back() {
        // Single chain with a connection that names an unannotated
        // bus in its binding. `compute_network_hop_latency` returns
        // `None` and the analysis falls through to the legacy
        // sampling-delay path. No panic, no `[network hop]` annotation.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let _cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
        let _cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
        let plain_bus = b.add_component("plain_bus", ComponentCategory::Bus, Some(root));
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        b.set_children(root, vec![_cpu1, _cpu2, plain_bus, sensor, ctrl]);
        b.add_connection_with_endpoints("c1", root, "sensor", "out_p", "controller", "in_p");
        b.add_e2e(
            "fallback",
            root,
            vec!["sensor.src", "c1", "controller.sink"],
        );
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(
            sensor,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu1)",
        );
        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            ctrl,
            "Deployment_Properties",
            "Actual_Processor_Binding",
            "reference (cpu2)",
        );
        b.set_property(
            root,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (plain_bus))",
        );
        // plain_bus has no Spar_Network::Switch_Type — the binding
        // resolves to a non-switch and the helper returns None.

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        for d in &diags {
            assert!(
                !d.message.contains("[network hop]"),
                "unannotated bus → no network hop: {}",
                d.message
            );
        }
        // Latency is still produced (legacy path).
        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap_or_else(|| panic!("expected latency Info, got {:?}", diags));
        assert!(info.message.contains("ms"));
    }

    /// Test 10: existing fixture-style chain stays unchanged. We
    /// re-exercise the seven main pre-Track-D scenarios in one assert
    /// block to guard against accidental drift in chains the v0.7.0
    /// suite already covers.
    #[test]
    fn existing_latency_fixtures_unchanged() {
        // Re-uses the pattern of `latency_simple_flow_best_worst_case`
        // and asserts the headline numbers (4 ms best, 34 ms worst) plus
        // the absence of any Track-D-specific markers.
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let sensor = b.add_component("sensor", ComponentCategory::Device, Some(root));
        let ctrl = b.add_component("controller", ComponentCategory::Thread, Some(root));
        let actuator = b.add_component("actuator", ComponentCategory::Device, Some(root));
        b.set_children(root, vec![sensor, ctrl, actuator]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);
        b.add_e2e(
            "e2e_control",
            root,
            vec!["sensor.src", "c1", "controller.pass", "c2", "actuator.sink"],
        );
        b.set_property(
            sensor,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(sensor, "Timing_Properties", "Period", "10 ms");
        b.set_property(ctrl, "Timing_Properties", "Compute_Execution_Time", "2 ms");
        b.set_property(ctrl, "Timing_Properties", "Period", "20 ms");
        b.set_property(
            actuator,
            "Timing_Properties",
            "Compute_Execution_Time",
            "1 ms",
        );
        b.set_property(actuator, "Timing_Properties", "Period", "10 ms");

        let inst = b.build(root);
        let diags = LatencyAnalysis.analyze(&inst);

        let info = diags
            .iter()
            .find(|d| d.severity == Severity::Info && d.message.contains("latency:"))
            .unwrap();
        assert!(info.message.contains("4.000 ms"), "best case 4ms preserved");
        assert!(
            info.message.contains("34.000 ms"),
            "worst case 34ms preserved"
        );
        for d in &diags {
            assert!(
                !d.message.contains("LatencyHopMixed"),
                "existing fixture must not gain LatencyHopMixed: {}",
                d.message
            );
        }
    }

    // ── REQ-ANALYSIS-FLOW-THREAD-DESCENT-001 ────────────────────────────
    //
    // A system-level flow through `app.proc_flow` descends into the flow
    // IMPLEMENTATION on `app`, so the threads' execution times, not `app`'s
    // absent one, reach the latency.

    fn seg(sub: Option<&str>, element: &str) -> FlowSegment {
        FlowSegment {
            subcomponent: sub.map(Name::new),
            element: Name::new(element),
        }
    }

    fn add_flow_spec(
        b: &mut TestBuilder,
        owner: ComponentInstanceIdx,
        name: &str,
        impl_segments: Option<Vec<FlowSegment>>,
    ) {
        let idx = b.flow_instances.alloc(FlowInstance {
            name: Name::new(name),
            kind: FlowKind::Path,
            owner,
            impl_segments,
        });
        b.components[owner].flows.push(idx);
    }

    struct Descent {
        b: TestBuilder,
        root: ComponentInstanceIdx,
        app: ComponentInstanceIdx,
        caller: ComponentInstanceIdx,
        consumer: ComponentInstanceIdx,
    }

    /// root { s, app { caller 5..10, consumer 7..12 }, k };
    /// `s.src -> c1 -> app.proc_flow -> c2 -> k.snk`. `s` and `k` carry no
    /// timing (devices), exactly as in the CLI oracle.
    fn descent_model(impl_segments: Option<Vec<FlowSegment>>) -> Descent {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let s = b.add_component("s", ComponentCategory::Device, Some(root));
        let app = b.add_component("app", ComponentCategory::Process, Some(root));
        let k = b.add_component("k", ComponentCategory::Device, Some(root));
        let caller = b.add_component("caller", ComponentCategory::Thread, Some(app));
        let consumer = b.add_component("consumer", ComponentCategory::Thread, Some(app));
        b.set_children(root, vec![s, app, k]);
        b.set_children(app, vec![caller, consumer]);
        b.add_connection_inst("c1", root);
        b.add_connection_inst("c2", root);
        b.add_connection_inst("c_in", app);
        b.add_connection_with_endpoints("c_hand", app, "caller", "req", "consumer", "inp");
        b.add_connection_inst("c_out", app);
        b.set_property(
            caller,
            "Timing_Properties",
            "Compute_Execution_Time",
            "5 ms .. 10 ms",
        );
        b.set_property(
            consumer,
            "Timing_Properties",
            "Compute_Execution_Time",
            "7 ms .. 12 ms",
        );
        add_flow_spec(&mut b, app, "proc_flow", impl_segments);
        b.add_e2e(
            "req_to_done",
            root,
            vec!["s.src", "c1", "app.proc_flow", "c2", "k.snk"],
        );
        Descent {
            b,
            root,
            app,
            caller,
            consumer,
        }
    }

    fn full_impl() -> Vec<FlowSegment> {
        vec![
            seg(None, "pin"),
            seg(None, "c_in"),
            seg(Some("caller"), "lock_flow"),
            seg(None, "c_hand"),
            seg(Some("consumer"), "consumer_flow"),
            seg(None, "c_out"),
            seg(None, "pout"),
        ]
    }

    fn has_latency(diags: &[AnalysisDiagnostic], range: &str) -> bool {
        diags
            .iter()
            .any(|d| d.severity == Severity::Info && d.message.ends_with(range))
    }

    fn missing_line(diags: &[AnalysisDiagnostic]) -> String {
        diags
            .iter()
            .find(|d| d.message.contains("without timing properties"))
            .unwrap_or_else(|| panic!("no missing-timing warning: {diags:?}"))
            .message
            .clone()
    }

    /// `flow_components` resolves a flow as latency does: through `app`'s flow
    /// implementation to the threads (not `app`), in order; a leaf `app` (no
    /// implementation) stays itself.
    #[test]
    fn flow_components_descends_like_latency() {
        let m = descent_model(Some(full_impl()));
        let (root, app) = (m.root, m.app);
        let inst = m.b.build(root);
        let e2e = inst.end_to_end_flows.iter().next().unwrap().1;
        let comps = flow_components(&inst, e2e);
        let names: Vec<&str> = comps
            .iter()
            .map(|&c| inst.component(c).name.as_str())
            .collect();
        assert_eq!(names, ["s", "caller", "consumer", "k"]);
        assert!(!comps.contains(&app));

        let m = descent_model(None);
        let root = m.root;
        let inst = m.b.build(root);
        let e2e = inst.end_to_end_flows.iter().next().unwrap().1;
        let names: Vec<&str> = flow_components(&inst, e2e)
            .iter()
            .map(|&c| inst.component(c).name.as_str())
            .collect();
        assert_eq!(names, ["s", "app", "k"]);
    }

    #[test]
    fn descent_derives_the_figure_from_the_threads_inside_the_process() {
        let m = descent_model(Some(full_impl()));
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));

        assert!(
            has_latency(&diags, "'req_to_done' latency: [12.000 ms .. 22.000 ms]"),
            "BCET 5+7, WCET 10+12: {diags:?}"
        );
        assert!(
            missing_line(&diags).ends_with("properties: s, k"),
            "exactly the two devices, in order, and not `app`: {diags:?}"
        );
        assert!(
            diags.iter().all(|d| !d.message.contains("not computed")),
            "{diags:?}"
        );
    }

    /// Names are matched case-insensitively everywhere else in this file.
    #[test]
    fn descent_matches_subcomponent_and_flow_names_case_insensitively() {
        let mut m = descent_model(Some(full_impl()));
        for (_, e) in m.b.end_to_end_flows.iter_mut() {
            e.segments = ["S.src", "c1", "APP.PROC_FLOW", "c2", "K.snk"]
                .iter()
                .map(|s| Name::new(s))
                .collect();
        }
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(has_latency(&diags, "[12.000 ms .. 22.000 ms]"), "{diags:?}");
    }

    /// The sampling delay of a thread inside the process is added exactly as
    /// for the in-implementation spelling: once per thread reached after a
    /// connection. Worst = 22 + 3 (caller, after c1) + 100 (consumer, after
    /// c_hand).
    #[test]
    fn descent_adds_sampling_delay_for_threads_after_connections() {
        let mut m = descent_model(Some(full_impl()));
        m.b.set_property(m.caller, "Timing_Properties", "Period", "3 ms");
        m.b.set_property(m.consumer, "Timing_Properties", "Period", "100 ms");
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            has_latency(&diags, "[12.000 ms .. 125.000 ms]"),
            "{diags:?}"
        );
    }

    /// A flow that STARTS at the process has no connection before its first
    /// thread, so that thread adds no sampling delay; the second (after the
    /// interior `c_hand`) does. The boundary `c_in` / `c_out` are not counted.
    #[test]
    fn descent_from_the_first_segment_counts_only_interior_connections() {
        let mut m = descent_model(Some(full_impl()));
        m.b.set_property(m.caller, "Timing_Properties", "Period", "3 ms");
        m.b.set_property(m.consumer, "Timing_Properties", "Period", "100 ms");
        for (_, e) in m.b.end_to_end_flows.iter_mut() {
            e.segments = vec![Name::new("app.proc_flow")];
        }
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            has_latency(&diags, "[12.000 ms .. 122.000 ms]"),
            "{diags:?}"
        );
    }

    #[test]
    fn a_child_without_a_flow_implementation_is_read_as_a_leaf_as_before() {
        // The flow spec exists but nothing implements it.
        let mut m = descent_model(None);
        m.b.set_property(m.app, "Timing_Properties", "Compute_Execution_Time", "4 ms");
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            has_latency(&diags, "[4.000 ms .. 4.000 ms]"),
            "app's own 4 ms, threads not read: {diags:?}"
        );
        assert!(
            missing_line(&diags).ends_with("properties: s, k"),
            "{diags:?}"
        );
    }

    #[test]
    fn a_child_with_no_flow_spec_at_all_is_read_as_a_leaf_as_before() {
        let mut m = descent_model(Some(full_impl()));
        m.b.components[m.app].flows.clear();
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            missing_line(&diags).ends_with("properties: s, app, k"),
            "walk stops at app: {diags:?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.severity == Severity::Error && d.message.contains("not computed")),
            "{diags:?}"
        );
    }

    /// An implementation that names no component (only own features and a
    /// connection) gives descent nothing to read: fall back to the child's own
    /// timing rather than silently contributing 0.
    #[test]
    fn an_implementation_naming_no_component_falls_back_to_the_child() {
        let imp = vec![seg(None, "pin"), seg(None, "c_hand"), seg(None, "pout")];
        let mut m = descent_model(Some(imp));
        m.b.set_property(m.app, "Timing_Properties", "Compute_Execution_Time", "4 ms");
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(has_latency(&diags, "[4.000 ms .. 4.000 ms]"), "{diags:?}");
    }

    /// An implementation segment naming a thread that is not a child of the
    /// process is reported missing, by the inner name, rather than dropped.
    #[test]
    fn an_unresolved_inner_component_is_reported_missing() {
        let imp = vec![
            seg(None, "pin"),
            seg(None, "c_in"),
            seg(Some("ghost"), "f"),
            seg(None, "c_hand"),
            seg(Some("consumer"), "consumer_flow"),
            seg(None, "pout"),
        ];
        let m = descent_model(Some(imp));
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            missing_line(&diags).ends_with("properties: s, ghost, k"),
            "{diags:?}"
        );
    }

    /// Descent follows nested flow implementations: `app.f` -> `inner.f` ->
    /// thread. `inner` and `app` carry (wrong) 99 / 98 ms that must never be
    /// read.
    #[test]
    fn descent_recurses_through_nested_flow_implementations() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let app = b.add_component("app", ComponentCategory::Process, Some(root));
        let inner = b.add_component("inner", ComponentCategory::Process, Some(app));
        let t = b.add_component("t", ComponentCategory::Thread, Some(inner));
        b.set_children(root, vec![app]);
        b.set_children(app, vec![inner]);
        b.set_children(inner, vec![t]);
        b.set_property(
            t,
            "Timing_Properties",
            "Compute_Execution_Time",
            "2 ms .. 3 ms",
        );
        b.set_property(
            inner,
            "Timing_Properties",
            "Compute_Execution_Time",
            "99 ms",
        );
        b.set_property(app, "Timing_Properties", "Compute_Execution_Time", "98 ms");
        add_flow_spec(&mut b, app, "f", Some(vec![seg(Some("inner"), "f")]));
        add_flow_spec(&mut b, inner, "f", Some(vec![seg(Some("t"), "g")]));
        b.add_e2e("e", root, vec!["app.f"]);
        let diags = LatencyAnalysis.analyze(&b.build(root));
        assert!(has_latency(&diags, "[2.000 ms .. 3.000 ms]"), "{diags:?}");
    }

    /// The descent ceiling: a 20-deep chain of flow implementations is followed
    /// to `MAX_FLOW_DESCENT_DEPTH` and the component reached there is read as a
    /// leaf. Component `n<i>` has exec `i` ms, so which one was read is visible.
    #[test]
    fn descent_stops_at_the_depth_ceiling_and_reads_a_leaf() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None);
        let mut parent = root;
        let mut ids = Vec::new();
        for i in 1..=20 {
            let c = b.add_component(&format!("n{i}"), ComponentCategory::Process, Some(parent));
            b.set_children(parent, vec![c]);
            b.set_property(
                c,
                "Timing_Properties",
                "Compute_Execution_Time",
                &format!("{i} ms"),
            );
            ids.push(c);
            parent = c;
        }
        for (i, &id) in ids.iter().enumerate().take(19) {
            add_flow_spec(
                &mut b,
                id,
                "f",
                Some(vec![seg(Some(&format!("n{}", i + 2)), "f")]),
            );
        }
        b.add_e2e("deep", root, vec!["n1.f"]);
        let diags = LatencyAnalysis.analyze(&b.build(root));
        // n1 (depth 0) .. n16 (depth 15) descend; n17 (depth 16) is the leaf.
        assert!(has_latency(&diags, "[17.000 ms .. 17.000 ms]"), "{diags:?}");
    }

    /// Make `sw` a switched bus and have `app` (the process, NOT the flow's
    /// owner) bind its connections to it. Only a connection looked up in
    /// `app`'s scope can find them.
    fn bind_app_connections_to_switch(m: &mut Descent) {
        let sw =
            m.b.add_component("sw", ComponentCategory::Bus, Some(m.root));
        m.b.components[m.root].children.push(sw);
        m.b.set_property(sw, "Spar_Network", "Switch_Type", "FIFO");
        m.b.set_property(sw, "Spar_Network", "Output_Rate", "1000000000 bitsps");
        m.b.set_property(sw, "Spar_Network", "Forwarding_Latency", "0 us .. 0 us");
        m.b.set_property(sw, "Spar_Network", "Queue_Depth", "1");
        m.b.set_property(
            m.app,
            "Deployment_Properties",
            "Actual_Connection_Binding",
            "(reference (sw))",
        );
    }

    fn network_hop_connections(diags: &[AnalysisDiagnostic]) -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.message.contains("[network hop]"))
            .map(|d| d.message.clone())
            .collect()
    }

    /// `c_hand` is declared in the PROCESS and bound there. The flow's owner
    /// (`root`) has no `c_hand`, so a lookup in the owner finds nothing. The
    /// boundary connections `c_in` / `c_out` are bound too, but they are the
    /// inner legs of `c1` / `c2` and must not be counted a second time.
    #[test]
    fn a_connection_inside_the_process_is_looked_up_in_the_process() {
        let mut m = descent_model(Some(full_impl()));
        bind_app_connections_to_switch(&mut m);
        let root = m.root;
        let inst = m.b.build(root);
        let hop = compute_network_hop_latency(&inst, m.app, "c_hand")
            .expect("precondition: c_hand binds to sw in app's scope");
        let diags = LatencyAnalysis.analyze(&inst);
        let net = network_hop_connections(&diags);
        assert_eq!(net.len(), 1, "only the interior connection: {diags:?}");
        assert!(net[0].contains("connection 'c_hand'"), "{net:?}");
        let ms = hop.max_ps as f64 / 1_000_000_000.0;
        assert!(net[0].contains(&format!("{ms:.3} ms")), "{net:?}");
        // 22 ms of execution plus exactly the one interior hop's bound.
        let worst = (22_000_000_000 + hop.max_ps) as f64 / 1_000_000_000.0;
        assert!(
            has_latency(&diags, &format!(".. {worst:.3} ms]")),
            "{diags:?}"
        );
    }

    /// With no leading or trailing own feature, EVERY connection of the
    /// implementation is interior, including the ones next to the ends.
    #[test]
    fn without_own_features_every_connection_in_the_implementation_counts() {
        let imp = vec![
            seg(Some("caller"), "lock_flow"),
            seg(None, "c_hand"),
            seg(Some("consumer"), "consumer_flow"),
        ];
        let mut m = descent_model(Some(imp));
        bind_app_connections_to_switch(&mut m);
        let root = m.root;
        let diags = LatencyAnalysis.analyze(&m.b.build(root));
        let net = network_hop_connections(&diags);
        assert_eq!(net.len(), 1, "{diags:?}");
        assert!(net[0].contains("connection 'c_hand'"), "{net:?}");
    }

    /// Own features at only ONE end: the connection touching that end is a
    /// boundary leg and is skipped; the other end's neighbour is interior.
    #[test]
    fn a_boundary_connection_is_skipped_at_either_end_independently() {
        // Leading feature only: `c_in` skipped, `c_out` (trailing, next to a
        // component) counted.
        let lead = vec![
            seg(None, "pin"),
            seg(None, "c_in"),
            seg(Some("caller"), "lock_flow"),
            seg(None, "c_out"),
            seg(Some("consumer"), "consumer_flow"),
        ];
        let mut m = descent_model(Some(lead));
        bind_app_connections_to_switch(&mut m);
        let root = m.root;
        let diags = LatencyAnalysis.analyze(&m.b.build(root));
        let net = network_hop_connections(&diags);
        assert_eq!(net.len(), 1, "{diags:?}");
        assert!(net[0].contains("connection 'c_out'"), "{net:?}");

        // Trailing feature only: `c_out` skipped, `c_in` counted.
        let trail = vec![
            seg(Some("caller"), "lock_flow"),
            seg(None, "c_in"),
            seg(Some("consumer"), "consumer_flow"),
            seg(None, "c_out"),
            seg(None, "pout"),
        ];
        let mut m = descent_model(Some(trail));
        bind_app_connections_to_switch(&mut m);
        let root = m.root;
        let diags = LatencyAnalysis.analyze(&m.b.build(root));
        let net = network_hop_connections(&diags);
        assert_eq!(net.len(), 1, "{diags:?}");
        assert!(net[0].contains("connection 'c_in'"), "{net:?}");
    }

    /// The processor-crossing note walks the SAME expansion: threads inside the
    /// process on different processors cross a boundary the latency walk
    /// applied, so the note must fire (no overhead property is set).
    #[test]
    fn the_processor_crossing_note_sees_threads_inside_the_process() {
        let mut m = descent_model(Some(full_impl()));
        let cpu1 =
            m.b.add_component("cpu1", ComponentCategory::Processor, Some(m.root));
        let cpu2 =
            m.b.add_component("cpu2", ComponentCategory::Processor, Some(m.root));
        m.b.components[m.root].children.push(cpu1);
        m.b.components[m.root].children.push(cpu2);
        for (t, cpu) in [(m.caller, "cpu1"), (m.consumer, "cpu2")] {
            m.b.set_property(
                t,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                &format!("reference ({cpu})"),
            );
        }
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("'req_to_done' crosses processor boundaries")),
            "{diags:?}"
        );
    }

    /// ...and a flow whose inner threads share a processor does not.
    #[test]
    fn the_processor_crossing_note_is_silent_when_inner_threads_share_a_processor() {
        let mut m = descent_model(Some(full_impl()));
        let cpu1 =
            m.b.add_component("cpu1", ComponentCategory::Processor, Some(m.root));
        let cpu2 =
            m.b.add_component("cpu2", ComponentCategory::Processor, Some(m.root));
        m.b.components[m.root].children.push(cpu1);
        m.b.components[m.root].children.push(cpu2);
        for t in [m.caller, m.consumer] {
            m.b.set_property(
                t,
                "Deployment_Properties",
                "Actual_Processor_Binding",
                "reference (cpu1)",
            );
        }
        let diags = LatencyAnalysis.analyze(&m.b.build(m.root));
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("crosses processor boundaries")),
            "{diags:?}"
        );
    }
}
