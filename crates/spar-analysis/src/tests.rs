//! Tests for the spar-analysis crate.

use la_arena::Arena;
use rustc_hash::FxHashMap;

use spar_hir_def::instance::*;
use spar_hir_def::item_tree::*;
use spar_hir_def::name::Name;

use crate::completeness::CompletenessAnalysis;
use crate::connectivity::ConnectivityAnalysis;
use crate::hierarchy::HierarchyAnalysis;
use crate::{Analysis, AnalysisDiagnostic, AnalysisRunner, Severity};

// ── Test helpers ────────────────────────────────────────────────────

/// Builder for constructing `SystemInstance` values in tests.
struct TestInstanceBuilder {
    components: Arena<ComponentInstance>,
    features: Arena<FeatureInstance>,
    connections: Arena<ConnectionInstance>,
    diagnostics: Vec<InstanceDiagnostic>,
}

impl TestInstanceBuilder {
    fn new() -> Self {
        Self {
            components: Arena::default(),
            features: Arena::default(),
            connections: Arena::default(),
            diagnostics: Vec::new(),
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

    fn add_connection(
        &mut self,
        name: &str,
        kind: ConnectionKind,
        is_bidirectional: bool,
        owner: ComponentInstanceIdx,
    ) -> ConnectionInstanceIdx {
        let idx = self.connections.alloc(ConnectionInstance {
            name: Name::new(name),
            kind,
            is_bidirectional,
            owner,
            src: None,
            dst: None,
        });
        self.components[owner].connections.push(idx);
        idx
    }

    fn set_children(&mut self, parent: ComponentInstanceIdx, children: Vec<ComponentInstanceIdx>) {
        self.components[parent].children = children;
    }

    fn add_diagnostic(&mut self, message: &str, path: Vec<&str>) {
        self.diagnostics.push(InstanceDiagnostic {
            message: message.to_string(),
            path: path.into_iter().map(Name::new).collect(),
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
            diagnostics: self.diagnostics,
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: Vec::new(),
        }
    }
}

/// Count diagnostics by severity.
fn count_by_severity(diags: &[AnalysisDiagnostic], severity: Severity) -> usize {
    diags.iter().filter(|d| d.severity == severity).count()
}

/// Find diagnostics containing a substring.
fn diags_containing<'a>(diags: &'a [AnalysisDiagnostic], substring: &str) -> Vec<&'a AnalysisDiagnostic> {
    diags
        .iter()
        .filter(|d| d.message.contains(substring))
        .collect()
}

// ── Connectivity Analysis Tests ─────────────────────────────────────

#[test]
fn connectivity_valid_model_with_connections() {
    // A well-connected system: root has ports, and parent has connections.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child_a = b.add_component("sensor", ComponentCategory::System, "Sensor", Some("basic"), "Pkg", Some(root));
    let child_b = b.add_component("controller", ComponentCategory::System, "Controller", Some("basic"), "Pkg", Some(root));

    // sensor has an out port, controller has an in port
    b.add_feature("reading", FeatureKind::DataPort, Some(Direction::Out), child_a);
    b.add_feature("input", FeatureKind::DataPort, Some(Direction::In), child_b);

    // root has a connection between them
    b.add_connection("c1", ConnectionKind::Port, false, root);
    b.set_children(root, vec![child_a, child_b]);

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Children's ports are "covered" because parent has connections.
    // No unconnected port warnings expected.
    let port_warnings: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("no incoming connection") || d.message.contains("no outgoing connection"))
        .collect();
    assert!(
        port_warnings.is_empty(),
        "expected no unconnected port warnings, got: {:?}",
        port_warnings
    );
}

#[test]
fn connectivity_unconnected_ports() {
    // A system where children have ports but no connections exist.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("sensor", ComponentCategory::System, "Sensor", Some("basic"), "Pkg", Some(root));

    b.add_feature("data_in", FeatureKind::DataPort, Some(Direction::In), child);
    b.add_feature("data_out", FeatureKind::DataPort, Some(Direction::Out), child);
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Should warn about both unconnected ports.
    let in_warnings = diags_containing(&diags, "no incoming connection");
    let out_warnings = diags_containing(&diags, "no outgoing connection");
    assert_eq!(in_warnings.len(), 1, "expected 1 input warning: {:?}", diags);
    assert_eq!(out_warnings.len(), 1, "expected 1 output warning: {:?}", diags);
}

#[test]
fn connectivity_dangling_connections() {
    // A system where parent has connections but child has no features.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("empty_sub", ComponentCategory::System, "Empty", None, "Pkg", Some(root));

    b.add_connection("c1", ConnectionKind::Port, false, root);
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Should warn about child having no features despite parent having connections.
    let dangling = diags_containing(&diags, "no features but parent");
    assert!(
        !dangling.is_empty(),
        "expected dangling connection warning: {:?}",
        diags
    );
}

#[test]
fn connectivity_event_ports() {
    // Event ports should also be checked.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("handler", ComponentCategory::System, "Handler", None, "Pkg", Some(root));

    b.add_feature("alarm", FeatureKind::EventPort, Some(Direction::In), child);
    b.add_feature("status", FeatureKind::EventDataPort, Some(Direction::Out), child);
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Should have warnings for both event ports.
    let in_warnings = diags_containing(&diags, "no incoming connection");
    let out_warnings = diags_containing(&diags, "no outgoing connection");
    assert_eq!(in_warnings.len(), 1);
    assert_eq!(out_warnings.len(), 1);
}

#[test]
fn connectivity_access_features_ignored() {
    // Access features (data access, bus access) should NOT trigger port warnings.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("mem", ComponentCategory::Memory, "RAM", None, "Pkg", Some(root));

    b.add_feature("bus_acc", FeatureKind::BusAccess, Some(Direction::In), child);
    b.add_feature("data_acc", FeatureKind::DataAccess, Some(Direction::Out), child);
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = ConnectivityAnalysis;
    let diags = analysis.analyze(&instance);

    // Access features are not ports — no "no incoming/outgoing connection" warnings.
    let port_warnings: Vec<_> = diags
        .iter()
        .filter(|d| d.message.contains("no incoming connection") || d.message.contains("no outgoing connection"))
        .collect();
    assert!(
        port_warnings.is_empty(),
        "access features should not trigger port warnings: {:?}",
        port_warnings
    );
}

// ── Hierarchy Analysis Tests ────────────────────────────────────────

#[test]
fn hierarchy_valid_containment() {
    // system > process > thread is valid.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let proc = b.add_component("proc1", ComponentCategory::Process, "Proc", Some("impl"), "Pkg", Some(root));
    let thread = b.add_component("t1", ComponentCategory::Thread, "Worker", Some("impl"), "Pkg", Some(proc));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thread]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    // Should have no containment errors.
    let errors = count_by_severity(&diags, Severity::Error);
    assert_eq!(errors, 0, "expected no errors: {:?}", diags);
}

#[test]
fn hierarchy_invalid_containment_thread_in_system() {
    // system > thread is invalid (thread must be inside process).
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let thread = b.add_component("t1", ComponentCategory::Thread, "Worker", None, "Pkg", Some(root));
    b.set_children(root, vec![thread]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error && d.message.contains("cannot contain"))
        .collect();
    assert_eq!(errors.len(), 1, "expected 1 containment error: {:?}", diags);
    assert!(errors[0].message.contains("thread"), "error should mention thread");
}

#[test]
fn hierarchy_invalid_containment_process_in_thread() {
    // thread > process is invalid.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let proc = b.add_component("proc1", ComponentCategory::Process, "Proc", Some("impl"), "Pkg", Some(root));
    let thread = b.add_component("t1", ComponentCategory::Thread, "Worker", Some("impl"), "Pkg", Some(proc));
    let bad_proc = b.add_component("bad_proc", ComponentCategory::Process, "BadProc", None, "Pkg", Some(thread));
    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thread]);
    b.set_children(thread, vec![bad_proc]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "expected 1 containment error: {:?}", diags);
    assert!(errors[0].message.contains("thread"));
    assert!(errors[0].message.contains("process"));
}

#[test]
fn hierarchy_abstract_can_contain_anything() {
    // abstract can contain any category.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::Abstract, "Container", Some("impl"), "Pkg", None);
    let sys = b.add_component("sys", ComponentCategory::System, "Sys", None, "Pkg", Some(root));
    let thread = b.add_component("t", ComponentCategory::Thread, "T", None, "Pkg", Some(root));
    let mem = b.add_component("m", ComponentCategory::Memory, "M", None, "Pkg", Some(root));
    b.set_children(root, vec![sys, thread, mem]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let errors = count_by_severity(&diags, Severity::Error);
    assert_eq!(errors, 0, "abstract should accept any child: {:?}", diags);
}

#[test]
fn hierarchy_empty_implementation_warning() {
    // A system implementation with no subcomponents should emit info.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let infos = diags_containing(&diags, "no subcomponents");
    assert_eq!(infos.len(), 1, "expected 1 empty impl info: {:?}", diags);
}

#[test]
fn hierarchy_data_empty_impl_no_warning() {
    // A data implementation with no subcomponents should NOT warn (trivially empty).
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let data = b.add_component("d1", ComponentCategory::Data, "MyData", Some("impl"), "Pkg", Some(root));
    b.set_children(root, vec![data]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    // Only root should warn about empty impl, not the data component.
    let empty_warnings = diags_containing(&diags, "no subcomponents");
    // root has children so no warning; data is trivially empty
    // Actually root has 1 child, so root won't warn. Data is trivially empty.
    for w in &empty_warnings {
        assert!(
            !w.message.contains("MyData"),
            "data should not warn about empty impl: {:?}",
            w
        );
    }
}

#[test]
fn hierarchy_deep_nesting_warning() {
    // Build a chain deeper than 8 levels.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);

    let mut prev = root;
    // Create a chain: root > s1 > s2 > ... > s9
    for i in 1..=9 {
        let name = format!("s{}", i);
        let child = b.add_component(
            &name,
            ComponentCategory::System,
            "Sub",
            Some("impl"),
            "Pkg",
            Some(prev),
        );
        b.set_children(prev, vec![child]);
        prev = child;
    }

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    // s9 is at depth 9 (> 8), should warn.
    let depth_warnings = diags_containing(&diags, "nesting depth");
    assert!(
        !depth_warnings.is_empty(),
        "expected deep nesting warning: {:?}",
        diags
    );
}

// Helpers that won't compile — let me fix the deep nesting test
// by removing the incorrect intermediate code.

#[test]
fn hierarchy_processor_containment() {
    // Processor can contain memory, bus, virtual processor, virtual bus, abstract.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let cpu = b.add_component("cpu", ComponentCategory::Processor, "CPU", Some("impl"), "Pkg", Some(root));
    let mem = b.add_component("cache", ComponentCategory::Memory, "Cache", None, "Pkg", Some(cpu));
    let vp = b.add_component("vp1", ComponentCategory::VirtualProcessor, "VP", None, "Pkg", Some(cpu));
    b.set_children(root, vec![cpu]);
    b.set_children(cpu, vec![mem, vp]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let errors = count_by_severity(&diags, Severity::Error);
    assert_eq!(errors, 0, "valid processor containment: {:?}", diags);
}

#[test]
fn hierarchy_processor_cannot_contain_thread() {
    // Processor cannot contain thread.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let cpu = b.add_component("cpu", ComponentCategory::Processor, "CPU", Some("impl"), "Pkg", Some(root));
    let thread = b.add_component("t1", ComponentCategory::Thread, "Worker", None, "Pkg", Some(cpu));
    b.set_children(root, vec![cpu]);
    b.set_children(cpu, vec![thread]);

    let instance = b.build(root);
    let analysis = HierarchyAnalysis;
    let diags = analysis.analyze(&instance);

    let errors = count_by_severity(&diags, Severity::Error);
    assert_eq!(errors, 1, "processor cannot contain thread: {:?}", diags);
}

// ── Completeness Analysis Tests ─────────────────────────────────────

#[test]
fn completeness_type_only_subcomponent() {
    // A subcomponent with only a type reference (no implementation).
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("sensor", ComponentCategory::System, "Sensor", None, "Pkg", Some(root));
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    // Should note that Sensor has no implementation in scope.
    let type_only = diags_containing(&diags, "no implementation in scope");
    assert!(
        !type_only.is_empty(),
        "expected type-only info: {:?}",
        diags
    );
}

#[test]
fn completeness_featureless_component() {
    // A system subcomponent with no features.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("sub", ComponentCategory::System, "Sub", None, "Pkg", Some(root));
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    let featureless = diags_containing(&diags, "has no features");
    assert!(
        !featureless.is_empty(),
        "expected featureless info: {:?}",
        diags
    );
}

#[test]
fn completeness_data_featureless_no_warning() {
    // A data component with no features should not warn (trivially featureless).
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let data = b.add_component("d1", ComponentCategory::Data, "Payload", None, "Pkg", Some(root));
    b.set_children(root, vec![data]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    let featureless = diags_containing(&diags, "has no features");
    for d in &featureless {
        assert!(
            !d.message.contains("Payload"),
            "data should not warn about featureless: {:?}",
            d
        );
    }
}

#[test]
fn completeness_unresolved_type() {
    // A component with empty type_name (unresolved).
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("unknown", ComponentCategory::System, "", None, "Pkg", Some(root));
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    let unresolved = diags_containing(&diags, "no classifier reference");
    assert!(
        !unresolved.is_empty(),
        "expected unresolved type warning: {:?}",
        diags
    );
}

#[test]
fn completeness_instance_diagnostics_forwarded() {
    // Instance-level diagnostics from instantiation should be forwarded.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    b.add_diagnostic("unresolved implementation: Pkg::Missing.impl", vec!["root"]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    let forwarded = diags_containing(&diags, "unresolved implementation");
    assert_eq!(
        forwarded.len(),
        1,
        "expected forwarded instance diagnostic: {:?}",
        diags
    );
    assert_eq!(forwarded[0].severity, Severity::Error);
}

#[test]
fn completeness_well_formed_model() {
    // A well-formed model with implementations and features should
    // produce only minimal info-level diagnostics.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let child = b.add_component("sensor", ComponentCategory::System, "Sensor", Some("basic"), "Pkg", Some(root));

    b.add_feature("reading", FeatureKind::DataPort, Some(Direction::Out), child);
    b.add_feature("cmd_in", FeatureKind::DataPort, Some(Direction::In), root);
    b.add_connection("c1", ConnectionKind::Port, false, root);
    b.set_children(root, vec![child]);

    let instance = b.build(root);
    let analysis = CompletenessAnalysis;
    let diags = analysis.analyze(&instance);

    let errors = count_by_severity(&diags, Severity::Error);
    let warnings = count_by_severity(&diags, Severity::Warning);
    assert_eq!(errors, 0, "well-formed model should have no errors: {:?}", diags);
    assert_eq!(warnings, 0, "well-formed model should have no warnings: {:?}", diags);
}

// ── AnalysisRunner Tests ────────────────────────────────────────────

#[test]
fn runner_collects_all_diagnostics() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    // Add a thread directly in system (containment error) with no features (completeness info).
    let thread = b.add_component("t1", ComponentCategory::Thread, "Worker", None, "Pkg", Some(root));
    b.set_children(root, vec![thread]);

    let instance = b.build(root);

    let mut runner = AnalysisRunner::new();
    runner.register(Box::new(ConnectivityAnalysis));
    runner.register(Box::new(HierarchyAnalysis));
    runner.register(Box::new(CompletenessAnalysis));

    let diags = runner.run_all(&instance);

    // Should have diagnostics from multiple analyses.
    let analyses: std::collections::HashSet<_> = diags.iter().map(|d| d.analysis.clone()).collect();
    assert!(
        analyses.len() >= 2,
        "expected diagnostics from multiple analyses, got: {:?}",
        analyses
    );

    // Should have at least one containment error from hierarchy.
    let hierarchy_errors: Vec<_> = diags
        .iter()
        .filter(|d| d.analysis == "hierarchy" && d.severity == Severity::Error)
        .collect();
    assert!(
        !hierarchy_errors.is_empty(),
        "expected hierarchy containment error: {:?}",
        diags
    );
}

#[test]
fn runner_empty_no_analyses() {
    let mut b = TestInstanceBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let instance = b.build(root);

    let runner = AnalysisRunner::new();
    let diags = runner.run_all(&instance);

    assert!(diags.is_empty(), "no analyses registered, no diagnostics expected");
}

#[test]
fn runner_valid_model_minimal_diagnostics() {
    // A valid, well-connected model should produce no errors and no warnings.
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let proc = b.add_component("proc", ComponentCategory::Process, "Proc", Some("impl"), "Pkg", Some(root));
    let thread = b.add_component("worker", ComponentCategory::Thread, "Worker", Some("impl"), "Pkg", Some(proc));

    // Give everyone features.
    b.add_feature("cmd_in", FeatureKind::DataPort, Some(Direction::In), root);
    b.add_feature("status_out", FeatureKind::DataPort, Some(Direction::Out), root);
    b.add_feature("data_in", FeatureKind::DataPort, Some(Direction::In), proc);
    b.add_feature("data_out", FeatureKind::DataPort, Some(Direction::Out), proc);
    b.add_feature("work_in", FeatureKind::DataPort, Some(Direction::In), thread);

    // Add connections.
    b.add_connection("c1", ConnectionKind::Port, false, root);
    b.add_connection("c2", ConnectionKind::Port, false, proc);

    b.set_children(root, vec![proc]);
    b.set_children(proc, vec![thread]);

    let instance = b.build(root);

    let mut runner = AnalysisRunner::new();
    runner.register(Box::new(ConnectivityAnalysis));
    runner.register(Box::new(HierarchyAnalysis));
    runner.register(Box::new(CompletenessAnalysis));

    let diags = runner.run_all(&instance);

    let errors = count_by_severity(&diags, Severity::Error);
    let warnings = count_by_severity(&diags, Severity::Warning);
    assert_eq!(errors, 0, "valid model should have no errors: {:?}", diags);
    assert_eq!(warnings, 0, "valid model should have no warnings: {:?}", diags);
}

// ── Containment rule unit tests ─────────────────────────────────────

#[test]
fn containment_rules_comprehensive() {
    use crate::hierarchy::is_valid_containment;
    use ComponentCategory::*;

    // System valid children
    assert!(is_valid_containment(System, System));
    assert!(is_valid_containment(System, Process));
    assert!(is_valid_containment(System, Device));
    assert!(is_valid_containment(System, Memory));
    assert!(is_valid_containment(System, Bus));
    assert!(is_valid_containment(System, Processor));
    assert!(is_valid_containment(System, VirtualProcessor));
    assert!(is_valid_containment(System, VirtualBus));
    assert!(is_valid_containment(System, Data));
    assert!(is_valid_containment(System, Abstract));

    // System invalid children
    assert!(!is_valid_containment(System, Thread));
    assert!(!is_valid_containment(System, ThreadGroup));
    assert!(!is_valid_containment(System, Subprogram));
    assert!(!is_valid_containment(System, SubprogramGroup));

    // Process valid children
    assert!(is_valid_containment(Process, Thread));
    assert!(is_valid_containment(Process, ThreadGroup));
    assert!(is_valid_containment(Process, Data));
    assert!(is_valid_containment(Process, Subprogram));
    assert!(is_valid_containment(Process, SubprogramGroup));
    assert!(is_valid_containment(Process, Abstract));

    // Process invalid children
    assert!(!is_valid_containment(Process, System));
    assert!(!is_valid_containment(Process, Process));
    assert!(!is_valid_containment(Process, Processor));
    assert!(!is_valid_containment(Process, Memory));

    // Thread valid children
    assert!(is_valid_containment(Thread, Data));
    assert!(is_valid_containment(Thread, Subprogram));
    assert!(is_valid_containment(Thread, Abstract));

    // Thread invalid children
    assert!(!is_valid_containment(Thread, Thread));
    assert!(!is_valid_containment(Thread, Process));
    assert!(!is_valid_containment(Thread, System));

    // ThreadGroup valid children
    assert!(is_valid_containment(ThreadGroup, Thread));
    assert!(is_valid_containment(ThreadGroup, ThreadGroup));
    assert!(is_valid_containment(ThreadGroup, Data));
    assert!(is_valid_containment(ThreadGroup, Subprogram));
    assert!(is_valid_containment(ThreadGroup, Abstract));

    // Processor valid children
    assert!(is_valid_containment(Processor, Memory));
    assert!(is_valid_containment(Processor, Bus));
    assert!(is_valid_containment(Processor, VirtualProcessor));
    assert!(is_valid_containment(Processor, VirtualBus));
    assert!(is_valid_containment(Processor, Abstract));

    // Processor invalid children
    assert!(!is_valid_containment(Processor, Thread));
    assert!(!is_valid_containment(Processor, Process));
    assert!(!is_valid_containment(Processor, System));

    // Abstract can contain anything
    assert!(is_valid_containment(Abstract, System));
    assert!(is_valid_containment(Abstract, Thread));
    assert!(is_valid_containment(Abstract, Processor));
    assert!(is_valid_containment(Abstract, Data));

    // Anything can contain abstract
    assert!(is_valid_containment(Thread, Abstract));
    assert!(is_valid_containment(Data, Abstract));
    assert!(is_valid_containment(Bus, Abstract));
}

// ── Path helper tests ───────────────────────────────────────────────

#[test]
fn component_path_builds_correctly() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let mid = b.add_component("mid", ComponentCategory::Process, "Mid", Some("impl"), "Pkg", Some(root));
    let leaf = b.add_component("leaf", ComponentCategory::Thread, "Leaf", None, "Pkg", Some(mid));
    b.set_children(root, vec![mid]);
    b.set_children(mid, vec![leaf]);

    let instance = b.build(root);
    let path = crate::component_path(&instance, leaf);
    assert_eq!(path, vec!["root", "mid", "leaf"]);
}

#[test]
fn component_depth_calculated_correctly() {
    let mut b = TestInstanceBuilder::new();

    let root = b.add_component("root", ComponentCategory::System, "Top", Some("impl"), "Pkg", None);
    let mid = b.add_component("mid", ComponentCategory::Process, "Mid", Some("impl"), "Pkg", Some(root));
    let leaf = b.add_component("leaf", ComponentCategory::Thread, "Leaf", None, "Pkg", Some(mid));
    b.set_children(root, vec![mid]);
    b.set_children(mid, vec![leaf]);

    let instance = b.build(root);
    assert_eq!(crate::component_depth(&instance, root), 0);
    assert_eq!(crate::component_depth(&instance, mid), 1);
    assert_eq!(crate::component_depth(&instance, leaf), 2);
}
