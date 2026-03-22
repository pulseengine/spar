//! Hierarchy validation analysis.
//!
//! Validates the AADL component containment rules from AS5506 section 4.5
//! and checks for structural issues in the component hierarchy.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;

use crate::{Analysis, AnalysisDiagnostic, Severity, component_depth, component_path};

/// Maximum recommended nesting depth before we emit a warning.
const MAX_RECOMMENDED_DEPTH: usize = 8;

/// Validates hierarchy structure and AADL containment rules.
///
/// Checks:
/// - Component categories follow AADL containment rules (AS5506 section 4.5)
/// - Warns about empty implementations (no subcomponents)
/// - Warns about deeply nested hierarchies (>8 levels)
pub struct HierarchyAnalysis;

impl Analysis for HierarchyAnalysis {
    fn name(&self) -> &str {
        "hierarchy"
    }

    fn analyze(&self, instance: &SystemInstance) -> Vec<AnalysisDiagnostic> {
        // Severity rationale (STPA-REQ-016):
        //   Error   — invalid containment per AS5506 section 4.5
        //   Warning — nesting depth exceeds 8 levels
        //   Info    — empty implementation with no subcomponents
        let mut diags = Vec::new();

        for (comp_idx, comp) in instance.all_components() {
            let path = component_path(instance, comp_idx);

            // Check containment rules for each child.
            for &child_idx in &comp.children {
                let child = instance.component(child_idx);
                if !is_valid_containment(comp.category, child.category) {
                    let child_path = component_path(instance, child_idx);
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Error,
                        message: format!(
                            "{} component '{}' cannot contain {} component '{}'",
                            comp.category, comp.name, child.category, child.name
                        ),
                        path: child_path,
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Warn about components with an implementation but no subcomponents.
            // Only warn if the component has an impl_name (meaning it was
            // instantiated from an implementation, not just a type).
            if comp.impl_name.is_some() && comp.children.is_empty() {
                // Data and abstract components commonly have no subcomponents.
                // Subprogram/SubprogramGroup similarly may have no children.
                let trivially_empty = matches!(
                    comp.category,
                    ComponentCategory::Data
                        | ComponentCategory::Abstract
                        | ComponentCategory::Subprogram
                        | ComponentCategory::SubprogramGroup
                        | ComponentCategory::Bus
                        | ComponentCategory::VirtualBus
                        | ComponentCategory::Device
                );
                if !trivially_empty {
                    diags.push(AnalysisDiagnostic {
                        severity: Severity::Info,
                        message: format!("implementation '{}' has no subcomponents", comp.name),
                        path: path.clone(),
                        analysis: self.name().to_string(),
                    });
                }
            }

            // Check nesting depth.
            let depth = component_depth(instance, comp_idx);
            if depth > MAX_RECOMMENDED_DEPTH {
                diags.push(AnalysisDiagnostic {
                    severity: Severity::Warning,
                    message: format!(
                        "component '{}' is at nesting depth {} (recommended max: {})",
                        comp.name, depth, MAX_RECOMMENDED_DEPTH
                    ),
                    path,
                    analysis: self.name().to_string(),
                });
            }
        }

        diags
    }
}

/// Check AADL containment rules per AS5506 section 4.5.
///
/// Returns `true` if `parent_cat` is allowed to contain `child_cat`.
///
/// The rules are:
/// - **system**: system, process, device, memory, bus, processor, virtual processor, virtual bus, abstract, data
/// - **process**: thread, thread group, data, abstract, subprogram, subprogram group
/// - **thread**: data, subprogram, abstract
/// - **thread group**: thread, thread group, data, subprogram, abstract
/// - **processor**: memory, bus, virtual processor, virtual bus, abstract
/// - **virtual processor**: virtual processor, virtual bus, abstract
/// - **memory**: memory, bus, abstract
/// - **bus**: virtual bus, abstract
/// - **virtual bus**: virtual bus, abstract
/// - **device**: bus, virtual bus, data, abstract
/// - **subprogram**: data, abstract
/// - **subprogram group**: subprogram, subprogram group, data, abstract
/// - **data**: data, subprogram, abstract
/// - **abstract**: (can contain anything — it's the universal category)
pub fn is_valid_containment(parent: ComponentCategory, child: ComponentCategory) -> bool {
    use ComponentCategory::*;

    // Abstract can contain anything.
    if parent == Abstract {
        return true;
    }

    // Abstract can be contained by anything.
    if child == Abstract {
        return true;
    }

    match parent {
        System => matches!(
            child,
            System
                | Process
                | Device
                | Memory
                | Bus
                | Processor
                | VirtualProcessor
                | VirtualBus
                | Data
        ),
        Process => matches!(
            child,
            Thread | ThreadGroup | Data | Subprogram | SubprogramGroup
        ),
        Thread => matches!(child, Data | Subprogram),
        ThreadGroup => matches!(child, Thread | ThreadGroup | Data | Subprogram),
        Processor => matches!(child, Memory | Bus | VirtualProcessor | VirtualBus),
        VirtualProcessor => matches!(child, VirtualProcessor | VirtualBus),
        Memory => matches!(child, Memory | Bus),
        Bus => matches!(child, VirtualBus),
        VirtualBus => matches!(child, VirtualBus),
        Device => matches!(child, Bus | VirtualBus | Data),
        Subprogram => matches!(child, Data),
        SubprogramGroup => matches!(child, Subprogram | SubprogramGroup | Data),
        Data => matches!(child, Data | Subprogram),
        Abstract => true, // handled above, but for exhaustiveness
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::name::Name;

    struct TestBuilder {
        components: Arena<ComponentInstance>,
        features: Arena<FeatureInstance>,
        connections: Arena<ConnectionInstance>,
    }

    impl TestBuilder {
        fn new() -> Self {
            Self {
                components: Arena::default(),
                features: Arena::default(),
                connections: Arena::default(),
            }
        }

        fn add_component(
            &mut self,
            name: &str,
            category: ComponentCategory,
            parent: Option<ComponentInstanceIdx>,
            impl_name: Option<&str>,
        ) -> ComponentInstanceIdx {
            self.components.alloc(ComponentInstance {
                name: Name::new(name),
                category,
                type_name: Name::new(name),
                impl_name: impl_name.map(Name::new),
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
                property_maps: FxHashMap::default(),
                semantic_connections: Vec::new(),
                system_operation_modes: Vec::new(),
            }
        }
    }

    #[test]
    fn valid_containment_no_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let proc = b.add_component("proc", ComponentCategory::Process, Some(root), Some("impl"));
        b.set_children(root, vec![proc]);

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "valid containment: {:?}", errors);
    }

    #[test]
    fn invalid_containment_error() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let thread = b.add_component("t1", ComponentCategory::Thread, Some(root), Some("impl"));
        b.set_children(root, vec![thread]);

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error && d.message.contains("cannot contain"))
            .collect();
        assert_eq!(errors.len(), 1, "system cannot contain thread: {:?}", diags);
    }

    #[test]
    fn empty_impl_non_trivial_category_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        // System with impl_name but no children → info
        let sub = b.add_component("sub", ComponentCategory::System, Some(root), Some("impl"));
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Info && d.message.contains("has no subcomponents"))
            .collect();
        assert_eq!(
            infos.len(),
            1,
            "empty impl should produce info: {:?}",
            diags
        );
    }

    #[test]
    fn empty_impl_data_category_no_info() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let data = b.add_component("data", ComponentCategory::Data, Some(root), Some("impl"));
        b.set_children(root, vec![data]);

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info
                    && d.message.contains("has no subcomponents")
                    && d.message.contains("data")
            })
            .collect();
        assert!(
            infos.is_empty(),
            "data should be trivially empty: {:?}",
            infos
        );
    }

    #[test]
    fn deep_nesting_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let mut parent = root;
        // Create a chain of 10 nested systems (depth > MAX_RECOMMENDED_DEPTH=8)
        for i in 0..10 {
            let child = b.add_component(
                &format!("s{i}"),
                ComponentCategory::System,
                Some(parent),
                Some("impl"),
            );
            b.set_children(parent, vec![child]);
            parent = child;
        }

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let depth_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("nesting depth"))
            .collect();
        assert!(
            !depth_warns.is_empty(),
            "deep nesting should warn: {:?}",
            depth_warns
        );
    }

    #[test]
    fn depth_exactly_at_limit_no_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        let mut parent = root;
        // Create chain of exactly MAX_RECOMMENDED_DEPTH=8 levels
        for i in 0..MAX_RECOMMENDED_DEPTH {
            let child = b.add_component(
                &format!("s{i}"),
                ComponentCategory::System,
                Some(parent),
                Some("impl"),
            );
            b.set_children(parent, vec![child]);
            parent = child;
        }

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let depth_warns: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && d.message.contains("nesting depth"))
            .collect();
        assert!(
            depth_warns.is_empty(),
            "exactly at limit should not warn: {:?}",
            depth_warns
        );
    }

    #[test]
    fn no_impl_name_no_empty_warning() {
        let mut b = TestBuilder::new();
        let root = b.add_component("root", ComponentCategory::System, None, Some("impl"));
        // sub has no impl_name, so no "empty implementation" info
        let sub = b.add_component("sub", ComponentCategory::System, Some(root), None);
        b.set_children(root, vec![sub]);

        let inst = b.build(root);
        let diags = HierarchyAnalysis.analyze(&inst);
        let infos: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.severity == Severity::Info
                    && d.message.contains("has no subcomponents")
                    && d.message.contains("sub")
            })
            .collect();
        assert!(
            infos.is_empty(),
            "no impl_name = no empty warning: {:?}",
            infos
        );
    }

    // ── Containment table unit tests ────────────────────────────────

    #[test]
    fn containment_abstract_parent() {
        assert!(is_valid_containment(
            ComponentCategory::Abstract,
            ComponentCategory::Thread
        ));
        assert!(is_valid_containment(
            ComponentCategory::Abstract,
            ComponentCategory::System
        ));
    }

    #[test]
    fn containment_abstract_child() {
        assert!(is_valid_containment(
            ComponentCategory::Thread,
            ComponentCategory::Abstract
        ));
        assert!(is_valid_containment(
            ComponentCategory::Bus,
            ComponentCategory::Abstract
        ));
    }
}
