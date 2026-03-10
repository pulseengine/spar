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
