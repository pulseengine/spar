//! Modal property evaluation helper (STPA-REQ-017).
//!
//! Provides utilities for checking whether a system instance has
//! System Operation Modes (SOMs) and retrieving mode names. Analyses
//! use these helpers to note when they used default (non-modal) property
//! values despite SOMs being defined.

use spar_hir_def::instance::SystemInstance;

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
}
