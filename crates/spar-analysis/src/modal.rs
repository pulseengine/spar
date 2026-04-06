//! Modal property evaluation helper (STPA-REQ-017).
//!
//! Provides utilities for checking whether a system instance has
//! System Operation Modes (SOMs) and retrieving mode names. Analyses
//! use these helpers to note when they used default (non-modal) property
//! values despite SOMs being defined.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance, SystemOperationMode};
use spar_hir_def::name::Name;

/// Check if an instance has any system operation modes.
pub fn has_modes(instance: &SystemInstance) -> bool {
    !instance.system_operation_modes.is_empty()
}

/// Get mode names for iteration.
pub fn mode_names(instance: &SystemInstance) -> Vec<String> {
    instance
        .system_operation_modes
        .iter()
        .map(|som| som.name.clone())
        .collect()
}

/// Check whether an element is active in a given mode context.
///
/// Rules:
/// - If `in_modes` is empty the element is active in **all** modes (non-modal).
/// - If `current_mode` is `None` (no mode context) the element is always active.
/// - Otherwise the element is active when `current_mode` matches any entry in
///   `in_modes` (case-insensitive comparison).
///
/// Check whether a component instance is active in a given System Operation Mode (SOM).
///
/// A component is active in a SOM when:
/// 1. Its `in_modes` list is empty (non-modal — active in all modes), or
/// 2. The SOM selects a mode on the component's parent, and that mode name
///    appears in the component's `in_modes` list.
///
/// If the component's parent has no mode selection in the SOM (i.e. the parent
/// is not a modal component), the component is treated as active.
pub fn is_component_active_in_som(
    instance: &SystemInstance,
    comp_idx: ComponentInstanceIdx,
    som: &SystemOperationMode,
) -> bool {
    let comp = instance.component(comp_idx);

    // Non-modal component: always active.
    if comp.in_modes.is_empty() {
        return true;
    }

    // Find the mode that the SOM selects for this component's parent.
    let parent_idx = match comp.parent {
        Some(p) => p,
        // Root component with in_modes set — unusual, treat as active.
        None => return true,
    };

    // Look through the SOM's mode selections for one owned by the parent.
    for &(_sel_comp, mode_inst_idx) in &som.mode_selections {
        let mode_inst = &instance.mode_instances[mode_inst_idx];
        if mode_inst.owner == parent_idx {
            // Found the mode selected on the parent — check if it matches.
            return comp
                .in_modes
                .iter()
                .any(|m| m.as_str().eq_ignore_ascii_case(mode_inst.name.as_str()));
        }
    }

    // Parent has no mode selection in this SOM — component is active.
    true
}

/// Check whether a connection is active in a given System Operation Mode (SOM).
///
/// A connection is active when its `in_modes` list is empty (non-modal) or
/// the SOM selects a mode on its owner that matches one of the connection's
/// mode names.
pub fn is_connection_active_in_som(
    instance: &SystemInstance,
    conn_owner: ComponentInstanceIdx,
    conn_in_modes: &[Name],
    som: &SystemOperationMode,
) -> bool {
    // Non-modal connection: always active.
    if conn_in_modes.is_empty() {
        return true;
    }

    // Find the mode that the SOM selects for the connection's owner.
    for &(_sel_comp, mode_inst_idx) in &som.mode_selections {
        let mode_inst = &instance.mode_instances[mode_inst_idx];
        if mode_inst.owner == conn_owner {
            return conn_in_modes
                .iter()
                .any(|m| m.as_str().eq_ignore_ascii_case(mode_inst.name.as_str()));
        }
    }

    // Owner has no mode selection in this SOM — connection is active.
    true
}

pub fn is_active_in_mode(in_modes: &[Name], current_mode: Option<&str>) -> bool {
    // Non-modal element: always active.
    if in_modes.is_empty() {
        return true;
    }
    // No mode context supplied: treat as active.
    let mode = match current_mode {
        Some(m) => m,
        None => return true,
    };
    in_modes
        .iter()
        .any(|m| m.as_str().eq_ignore_ascii_case(mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use la_arena::Arena;
    use rustc_hash::FxHashMap;
    use spar_hir_def::instance::*;
    use spar_hir_def::item_tree::ComponentCategory;
    use spar_hir_def::name::Name;

    fn make_instance(soms: Vec<SystemOperationMode>) -> SystemInstance {
        let mut components = Arena::default();
        let root = components.alloc(ComponentInstance {
            name: Name::new("root"),
            category: ComponentCategory::System,
            type_name: Name::new("root"),
            impl_name: Some(Name::new("impl")),
            package: Name::new("Pkg"),
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
        SystemInstance {
            root,
            components,
            features: Arena::default(),
            connections: Arena::default(),
            flow_instances: Arena::default(),
            end_to_end_flows: Arena::default(),
            mode_instances: Arena::default(),
            mode_transition_instances: Arena::default(),
            diagnostics: Vec::new(),
            property_maps: FxHashMap::default(),
            semantic_connections: Vec::new(),
            system_operation_modes: soms,
        }
    }

    #[test]
    fn has_modes_false_when_empty() {
        let inst = make_instance(Vec::new());
        assert!(!has_modes(&inst));
    }

    #[test]
    fn has_modes_true_when_present() {
        let soms = vec![SystemOperationMode {
            name: "nominal".to_string(),
            mode_selections: Vec::new(),
        }];
        let inst = make_instance(soms);
        assert!(has_modes(&inst));
    }

    #[test]
    fn mode_names_returns_all_names() {
        let soms = vec![
            SystemOperationMode {
                name: "active_fast".to_string(),
                mode_selections: Vec::new(),
            },
            SystemOperationMode {
                name: "standby_slow".to_string(),
                mode_selections: Vec::new(),
            },
        ];
        let inst = make_instance(soms);
        let names = mode_names(&inst);
        assert_eq!(names, vec!["active_fast", "standby_slow"]);
    }

    #[test]
    fn mode_names_empty_when_no_soms() {
        let inst = make_instance(Vec::new());
        let names = mode_names(&inst);
        assert!(names.is_empty());
    }

    // ── is_active_in_mode tests ────────────────────────────────────

    #[test]
    fn no_mode_context_always_active() {
        let in_modes = vec![Name::new("fast")];
        assert!(is_active_in_mode(&in_modes, None));
    }

    #[test]
    fn empty_in_modes_always_active() {
        let in_modes: Vec<Name> = Vec::new();
        assert!(is_active_in_mode(&in_modes, Some("fast")));
        assert!(is_active_in_mode(&in_modes, None));
    }

    #[test]
    fn matching_mode_is_active() {
        let in_modes = vec![Name::new("fast"), Name::new("slow")];
        assert!(is_active_in_mode(&in_modes, Some("fast")));
        assert!(is_active_in_mode(&in_modes, Some("slow")));
        assert!(!is_active_in_mode(&in_modes, Some("standby")));
    }

    #[test]
    fn mode_matching_is_case_insensitive() {
        let in_modes = vec![Name::new("FastMode")];
        assert!(is_active_in_mode(&in_modes, Some("fastmode")));
        assert!(is_active_in_mode(&in_modes, Some("FASTMODE")));
        assert!(is_active_in_mode(&in_modes, Some("FastMode")));
    }
}
