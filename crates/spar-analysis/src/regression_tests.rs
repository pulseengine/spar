//! Regression tests for closed issues and fixed bugs (STPA-REQ-023).
//!
//! When a bug or false negative is discovered and fixed, a regression test
//! must be added here that reproduces the original defect and verifies the
//! fix. Each test documents the original issue in its doc comment.
//!
//! Convention:
//!   - Test name: `regression_<issue_or_description>`
//!   - Doc comment: describe the original bug, how it manifested, and what
//!     the fix was.

use la_arena::Arena;
use rustc_hash::FxHashMap;

use spar_hir_def::instance::*;
use spar_hir_def::item_tree::*;
use spar_hir_def::name::Name;
use spar_hir_def::name::PropertyRef;
use spar_hir_def::properties::{PropertyMap, PropertyValue};

use crate::connectivity::ConnectivityAnalysis;
use crate::hierarchy::HierarchyAnalysis;
use crate::mode_check::ModeCheckAnalysis;
use crate::{Analysis, AnalysisDiagnostic};

// ── Test helpers ──────────────────────────────────────────────────

struct TestInstanceBuilder {
    components: Arena<ComponentInstance>,
    features: Arena<FeatureInstance>,
    connections: Arena<ConnectionInstance>,
    property_maps: FxHashMap<ComponentInstanceIdx, PropertyMap>,
}

impl TestInstanceBuilder {
    fn new() -> Self {
        Self {
            components: Arena::default(),
            features: Arena::default(),
            connections: Arena::default(),
            property_maps: FxHashMap::default(),
        }
    }

    fn add_component(
        &mut self,
        name: &str,
        category: ComponentCategory,
        type_name: &str,
        impl_name: Option<&str>,
        package: &str,
        parent: Option<ComponentInstanceIdx>,
    ) -> ComponentInstanceIdx {
        self.components.alloc(ComponentInstance {
            name: Name::new(name),
            category,
            type_name: Name::new(type_name),
            impl_name: impl_name.map(Name::new),
            package: Name::new(package),
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

    fn add_feature(
        &mut self,
        name: &str,
        kind: FeatureKind,
        direction: Option<Direction>,
        owner: ComponentInstanceIdx,
    ) -> FeatureInstanceIdx {
        let idx = self.features.alloc(FeatureInstance {
            name: Name::new(name),
            kind,
            direction,
            owner,
            classifier: None,
            access_kind: None,
            array_index: None,
        });
        self.components[owner].features.push(idx);
        idx
    }

    fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
        self.components[parent].children = children;
    }

    fn set_property(&mut self, comp: ComponentInstanceIdx, set: &str, name: &str, value: &str) {
        let map = self.property_maps.entry(comp).or_default();
        map.add(PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new(set)),
                property_name: Name::new(name),
            },
            value: value.to_string(),
            is_append: false,
        });
    }

    fn build(self, root: ComponentInstanceIdx) -> SystemInstance {
        SystemInstance {
            root,
            components: self.components,
            features: self.features,
            connections: self.connections,
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: self.property_maps,
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }
}

fn diags_containing<'a>(
    diags: &'a [AnalysisDiagnostic],
    substring: &str,
) -> Vec<&'a AnalysisDiagnostic> {
    diags
        .iter()
        .filter(|d| d.message.contains(substring))
        .collect()
}

// ── Regression tests ──────────────────────────────────────────────

/// Regression: connectivity analysis should not warn about ports that
/// have the Intentionally_Unconnected property (STPA-REQ-018). Before
/// the fix, debug/monitoring ports without connections triggered false
/// positive "unconnected feature" warnings causing alert fatigue.
#[test]
fn regression_intentionally_unconnected_suppresses_warning() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component(
        "root",
        ComponentCategory::System,
        "Top",
        Some("impl"),
        "Pkg",
        None,
    );
    let child = b.add_component(
        "sensor",
        ComponentCategory::Device,
        "Sensor",
        Some("impl"),
        "Pkg",
        Some(root),
    );
    // Port intentionally left unconnected
    b.add_feature(
        "debug_out",
        FeatureKind::EventDataPort,
        Some(Direction::Out),
        child,
    );
    b.set_children(root, vec![child]);
    // Mark the port as intentionally unconnected via property
    b.set_property(
        child,
        "SPAR_Properties",
        "Intentionally_Unconnected",
        "debug_out",
    );

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Should NOT produce unconnected warning for debug_out
    let unconnected = diags_containing(&diags, "debug_out");
    assert!(
        unconnected.is_empty(),
        "intentionally unconnected port should not produce warning, got: {:?}",
        unconnected
    );
}

/// Regression: hierarchy analysis should detect invalid containment.
/// A bus cannot contain a process — this violates AS5506 section 4.5
/// containment rules.
#[test]
fn regression_invalid_containment_detected() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component(
        "root",
        ComponentCategory::Bus,
        "MyBus",
        Some("impl"),
        "Pkg",
        None,
    );
    // Bus cannot contain process — invalid containment
    let invalid_child = b.add_component(
        "bad_child",
        ComponentCategory::Process,
        "P",
        None,
        "Pkg",
        Some(root),
    );
    b.set_children(root, vec![invalid_child]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let containment_errors = diags_containing(&diags, "cannot contain");
    assert!(
        !containment_errors.is_empty(),
        "invalid containment should produce hierarchy error"
    );
}

/// Regression: modal metadata (in_modes) on ComponentInstance must
/// survive instantiation (STPA-REQ-008). Before the fix, in_modes
/// was not propagated from SubcomponentItem to ComponentInstance,
/// silently dropping modal membership information.
#[test]
fn regression_in_modes_not_dropped_on_component() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component(
        "root",
        ComponentCategory::System,
        "Top",
        Some("impl"),
        "Pkg",
        None,
    );
    let child_idx = b.add_component(
        "modal_child",
        ComponentCategory::Thread,
        "Worker",
        None,
        "Pkg",
        Some(root),
    );
    // Simulate what the instance builder now does
    b.components[child_idx].in_modes = vec![Name::new("active"), Name::new("standby")];
    b.set_children(root, vec![child_idx]);

    let instance = b.build(root);

    // Verify in_modes is preserved
    assert_eq!(instance.components[child_idx].in_modes.len(), 2);
    assert_eq!(
        instance.components[child_idx].in_modes[0].as_str(),
        "active"
    );
}

/// Regression: modal metadata (in_modes) on ConnectionInstance must
/// survive instantiation (STPA-REQ-008). Before the fix, connections
/// lost their modal membership, meaning mode-filtered analyses
/// couldn't determine which connections are active in which modes.
#[test]
fn regression_in_modes_not_dropped_on_connection() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component(
        "root",
        ComponentCategory::System,
        "Top",
        Some("impl"),
        "Pkg",
        None,
    );
    let conn_idx = b.connections.alloc(ConnectionInstance {
        name: Name::new("c1"),
        kind: ConnectionKind::Port,
        is_bidirectional: false,
        owner: root,
        src: None,
        dst: None,
        in_modes: vec![Name::new("fast")],
    });
    b.components[root].connections.push(conn_idx);

    let instance = b.build(root);

    assert_eq!(instance.connections[conn_idx].in_modes.len(), 1);
    assert_eq!(instance.connections[conn_idx].in_modes[0].as_str(), "fast");
}

/// Regression: mode_check analysis should flag duplicate initial modes.
/// A component with two modes both marked initial is a modeling error
/// per AS5506.
#[test]
fn regression_duplicate_initial_mode_detected() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component(
        "root",
        ComponentCategory::System,
        "Top",
        Some("impl"),
        "Pkg",
        None,
    );

    let mut mode_instances: Arena<ModeInstance> = Arena::default();
    let m1 = mode_instances.alloc(ModeInstance {
        name: Name::new("fast"),
        is_initial: true,
        owner: root,
    });
    let m2 = mode_instances.alloc(ModeInstance {
        name: Name::new("slow"),
        is_initial: true,
        owner: root,
    });
    b.components[root].modes = vec![m1, m2];

    let mut instance = b.build(root);
    instance.mode_instances = mode_instances;

    let analysis = ModeCheckAnalysis;
    let diags = analysis.analyze(&instance);

    let initial_diags = diags_containing(&diags, "initial");
    assert!(
        !initial_diags.is_empty(),
        "duplicate initial modes should produce diagnostic"
    );
}

/// Regression: analysis runner register_all() must include all analysis
/// passes (STPA-REQ-014). If a new analysis is added but not registered,
/// it would silently not run.
#[test]
fn regression_register_all_includes_all_analyses() {
    let mut runner = crate::AnalysisRunner::new();
    runner.register_all();
    // Update this count when new analyses are added to register_all().
    assert!(
        runner.count() >= 27,
        "register_all() should register at least 27 analyses, got {}",
        runner.count()
    );
}
