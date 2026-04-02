//! Tests for AI/ML analysis pass.

use spar_hir_def::instance::{ComponentInstance, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::{Name, PropertyRef};
use spar_hir_def::properties::{PropertyMap, PropertyValue};

use crate::{Analysis, Severity};

use super::AiMlAnalysis;

// ── Helpers ────────────────────────────────────────────────────────

fn make_props(entries: &[(&str, &str, &str)]) -> PropertyMap {
    let mut props = PropertyMap::new();
    for &(set, name, value) in entries {
        props.add(PropertyValue {
            name: PropertyRef {
                property_set: if set.is_empty() {
                    None
                } else {
                    Some(Name::new(set))
                },
                property_name: Name::new(name),
            },
            value: value.to_string(),
            is_append: false,
        });
    }
    props
}

/// Build a minimal SystemInstance with a single thread (or process) and given properties.
fn single_component_instance(
    category: ComponentCategory,
    props: PropertyMap,
) -> SystemInstance {
    use la_arena::Arena;
    use rustc_hash::FxHashMap;

    let mut components = Arena::new();
    let root_idx = components.alloc(ComponentInstance {
        name: Name::new("root"),
        category: ComponentCategory::System,
        type_name: Name::new("TestSys"),
        impl_name: Some(Name::new("TestSys.Impl")),
        package: Name::new("TestPkg"),
        parent: None,
        children: Vec::new(),
        features: Vec::new(),
        connections: Vec::new(),
        flows: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        array_index: None,
        in_modes: Vec::new(),
    });

    let child_idx = components.alloc(ComponentInstance {
        name: Name::new("ml_comp"),
        category,
        type_name: Name::new("MlType"),
        impl_name: None,
        package: Name::new("TestPkg"),
        parent: Some(root_idx),
        children: Vec::new(),
        features: Vec::new(),
        connections: Vec::new(),
        flows: Vec::new(),
        modes: Vec::new(),
        mode_transitions: Vec::new(),
        array_index: None,
        in_modes: Vec::new(),
    });

    // Set up parent-child relationship
    components[root_idx].children.push(child_idx);

    let mut property_maps = FxHashMap::default();
    property_maps.insert(child_idx, props);
    property_maps.insert(root_idx, PropertyMap::new());

    SystemInstance {
        root: root_idx,
        components,
        features: Arena::new(),
        connections: Arena::new(),
        flow_instances: Arena::new(),
        end_to_end_flows: Arena::new(),
        mode_instances: Arena::new(),
        mode_transition_instances: Arena::new(),
        diagnostics: Vec::new(),
        property_maps,
        semantic_connections: Vec::new(),
        system_operation_modes: Vec::new(),
    }
}

// ── Check 1: Inference deadline ────────────────────────────────────

#[test]
fn inference_within_deadline_no_diagnostic() {
    let props = make_props(&[
        ("AI_ML", "Inference_Latency", "20 ms .. 60 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    // No errors — 60ms worst case within 80ms deadline
    assert!(
        diags.iter().all(|d| d.severity != Severity::Error),
        "unexpected error: {:?}",
        diags
    );
}

#[test]
fn inference_exceeds_deadline_error() {
    let props = make_props(&[
        ("AI_ML", "Inference_Latency", "20 ms .. 100 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("exceeds")),
        "expected deadline error: {:?}",
        diags
    );
}

#[test]
fn inference_without_deadline_warns() {
    let props = make_props(&[("AI_ML", "Inference_Latency", "20 ms .. 60 ms")]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Warning
            && d.message.contains("no Deadline")),
        "expected missing deadline warning: {:?}",
        diags
    );
}

// ── Check 2: Fallback coverage ─────────────────────────────────────

#[test]
fn no_fallback_strategy_warns() {
    let props = make_props(&[("AI_ML", "Inference_Latency", "20 ms .. 60 ms")]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("no Fallback_Strategy")),
        "expected fallback warning: {:?}",
        diags
    );
}

#[test]
fn with_fallback_strategy_no_coverage_warning() {
    let props = make_props(&[
        ("AI_ML", "Inference_Latency", "20 ms .. 60 ms"),
        ("AI_ML", "Fallback_Strategy", "Safe_Stop"),
        ("Timing_Properties", "Deadline", "80 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags
            .iter()
            .all(|d| !d.message.contains("no Fallback_Strategy")),
        "unexpected fallback warning: {:?}",
        diags
    );
}

// ── Check 3: Fallback timing ───────────────────────────────────────

#[test]
fn fallback_timing_exceeds_deadline() {
    // 60ms inference + 25ms fallback = 85ms > 80ms deadline
    let props = make_props(&[
        ("AI_ML", "Inference_Latency", "20 ms .. 60 ms"),
        ("AI_ML", "Fallback_Strategy", "Previous_Output"),
        ("AI_ML", "Fallback_Latency", "25 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("fallback")),
        "expected fallback timing error: {:?}",
        diags
    );
}

#[test]
fn fallback_timing_within_deadline() {
    // 60ms inference + 10ms fallback = 70ms < 80ms deadline
    let props = make_props(&[
        ("AI_ML", "Inference_Latency", "20 ms .. 60 ms"),
        ("AI_ML", "Fallback_Strategy", "Previous_Output"),
        ("AI_ML", "Fallback_Latency", "10 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags
            .iter()
            .all(|d| !(d.severity == Severity::Error && d.message.contains("fallback"))),
        "unexpected fallback timing error: {:?}",
        diags
    );
}

// ── Check 4: OOD coverage ──────────────────────────────────────────

#[test]
fn confidence_without_ood_warns() {
    let props = make_props(&[
        ("AI_ML", "Confidence_Threshold", "0.85"),
        ("AI_ML", "Inference_Latency", "20 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
        ("AI_ML", "Fallback_Strategy", "Safe_Stop"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().any(|d| d.message.contains("OOD_Detection")),
        "expected OOD warning: {:?}",
        diags
    );
}

#[test]
fn confidence_with_ood_no_warning() {
    let props = make_props(&[
        ("AI_ML", "Confidence_Threshold", "0.85"),
        ("AI_ML", "OOD_Detection_Enabled", "true"),
        ("AI_ML", "Inference_Latency", "20 ms"),
        ("Timing_Properties", "Deadline", "80 ms"),
        ("AI_ML", "Fallback_Strategy", "Safe_Stop"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().all(|d| !d.message.contains("OOD_Detection")),
        "unexpected OOD warning: {:?}",
        diags
    );
}

// ── Check 5: Model deployment ──────────────────────────────────────

#[test]
fn model_format_without_binding_errors() {
    let props = make_props(&[
        ("AI_ML", "Model_Format", "ONNX"),
        ("AI_ML", "Model_Version", "yolov8-v3"),
    ]);
    let inst = single_component_instance(ComponentCategory::Process, props);
    let diags = AiMlAnalysis.analyze(&inst);
    assert!(
        diags.iter().any(|d| d.severity == Severity::Error
            && d.message.contains("Actual_Processor_Binding")),
        "expected deployment error: {:?}",
        diags
    );
}

// ── Non-AI components are skipped ──────────────────────────────────

#[test]
fn non_ai_component_produces_no_diagnostics() {
    let props = make_props(&[
        ("Timing_Properties", "Period", "10 ms"),
        ("Timing_Properties", "Deadline", "10 ms"),
    ]);
    let inst = single_component_instance(ComponentCategory::Thread, props);
    let diags = AiMlAnalysis.analyze(&inst);
    // Only the root system, which has no AI properties
    assert!(
        diags.is_empty(),
        "expected no diagnostics for non-AI component: {:?}",
        diags
    );
}
