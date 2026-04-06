//! Standard predefined AADL property sets (AS5506 Appendix A).
//!
//! These property sets are built-in to every AADL model and do not
//! require explicit `with` imports. They are registered automatically
//! in the [`GlobalScope`](crate::resolver::GlobalScope) so that
//! property references like `Timing_Properties::Period` resolve
//! without parsing any AADL property set files.

/// Metadata about a single standard property definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardProperty {
    /// The property name (e.g., `"Period"`).
    pub name: &'static str,
    /// The property set this belongs to (e.g., `"Timing_Properties"`).
    pub property_set: &'static str,
    /// A human-readable description of the property's type.
    pub type_description: &'static str,
}

/// All standard predefined property set names.
pub const STANDARD_PROPERTY_SET_NAMES: &[&str] = &[
    "Timing_Properties",
    "Communication_Properties",
    "Memory_Properties",
    "Deployment_Properties",
    "Thread_Properties",
    "Programming_Properties",
    "Modeling_Properties",
    "AADL_Project",
];

// ── Timing_Properties ───────────────────────────────────────────────

const TIMING_PROPERTIES: &[(&str, &str)] = &[
    ("Period", "Time"),
    ("Deadline", "Time"),
    ("Compute_Execution_Time", "Time_Range"),
    ("Clock_Period", "Time"),
    (
        "Reference_Time",
        "list of reference (applies to processor, device)",
    ),
    ("Startup_Execution_Time", "Time_Range"),
    ("Clock_Jitter", "Time"),
    ("First_Dispatch_Time", "Time"),
    ("Dispatch_Jitter", "Time"),
    ("Dispatch_Offset", "Time"),
    ("Execution_Time", "Time_Range"),
    ("Process_Swap_Execution_Time", "Time_Range"),
];

// ── Communication_Properties ────────────────────────────────────────

const COMMUNICATION_PROPERTIES: &[(&str, &str)] = &[
    (
        "Fan_Out_Policy",
        "enumeration (Broadcast, RoundRobin, Selective)",
    ),
    ("Queue_Size", "aadlinteger"),
    (
        "Queue_Processing_Protocol",
        "enumeration (FIFO, LIFO, Priority)",
    ),
    (
        "Overflow_Handling_Protocol",
        "enumeration (DropOldest, DropNewest, Error)",
    ),
    ("Transmission_Type", "enumeration (push, pull)"),
    ("Input_Rate", "Rate_Spec"),
    ("Output_Rate", "Rate_Spec"),
    ("Data_Rate", "aadlinteger units Data_Rate_Units"),
    (
        "Connection_Pattern",
        "list of list of Supported_Connection_Patterns",
    ),
    ("Connection_Set", "list of Connection_Pair"),
    ("Latency", "Time_Range"),
    ("Actual_Latency", "Time_Range"),
    ("Required_Connection", "list of reference (port)"),
];

// ── Memory_Properties ───────────────────────────────────────────────

const MEMORY_PROPERTIES: &[(&str, &str)] = &[
    ("Data_Size", "Size"),
    ("Code_Size", "Size"),
    ("Stack_Size", "Size"),
    ("Heap_Size", "Size"),
    ("Byte_Count", "aadlinteger"),
    ("Word_Size", "Size"),
    ("Word_Space", "aadlinteger 1..2**32"),
    ("Base_Address", "aadlinteger"),
    ("Source_Code_Size", "Size"),
    ("Source_Data_Size", "Size"),
    ("Source_Stack_Size", "Size"),
    ("Source_Heap_Size", "Size"),
    ("Runtime_Protection", "aadlboolean"),
    ("Read_Only", "aadlboolean"),
];

// ── Deployment_Properties ───────────────────────────────────────────

const DEPLOYMENT_PROPERTIES: &[(&str, &str)] = &[
    (
        "Allowed_Processor_Binding",
        "list of reference (processor, virtual processor, device, system)",
    ),
    (
        "Actual_Processor_Binding",
        "list of reference (processor, virtual processor)",
    ),
    (
        "Allowed_Memory_Binding",
        "list of reference (memory, system, processor)",
    ),
    (
        "Actual_Memory_Binding",
        "list of reference (memory, system, processor)",
    ),
    (
        "Allowed_Connection_Binding",
        "list of reference (bus, virtual bus, processor, virtual processor, device, memory, system)",
    ),
    (
        "Actual_Connection_Binding",
        "list of reference (bus, virtual bus, processor, virtual processor, device, memory, system)",
    ),
    (
        "Allowed_Dispatch_Protocol",
        "list of Supported_Dispatch_Protocols",
    ),
    ("Actual_Subprogram_Call", "reference (subprogram)"),
    ("Actual_Subprogram_Call_Binding", "list of reference"),
    ("Preemptive_Scheduler", "aadlboolean"),
    (
        "Scheduling_Protocol",
        "list of Supported_Scheduling_Protocols",
    ),
    (
        "Not_Collocated",
        "record (Targets: list of reference; Location: classifier)",
    ),
    ("Priority_Map", "list of Priority_Mapping"),
    ("Priority_Range", "range of aadlinteger"),
];

// ── Thread_Properties ───────────────────────────────────────────────

const THREAD_PROPERTIES: &[(&str, &str)] = &[
    (
        "Dispatch_Protocol",
        "enumeration (Periodic, Sporadic, Aperiodic, Timed, Hybrid, Background)",
    ),
    ("Dispatch_Trigger", "list of reference (port)"),
    ("Priority", "aadlinteger"),
    ("Criticality", "aadlinteger"),
    (
        "POSIX_Scheduling_Policy",
        "enumeration (SCHED_FIFO, SCHED_RR, SCHED_OTHERS)",
    ),
    (
        "Active_Thread_Handling_Protocol",
        "Supported_Active_Thread_Handling_Protocols",
    ),
    (
        "Active_Thread_Queue_Handling_Protocol",
        "enumeration (flush, hold)",
    ),
];

// ── Programming_Properties ──────────────────────────────────────────

const PROGRAMMING_PROPERTIES: &[(&str, &str)] = &[
    ("Source_Language", "list of Supported_Source_Languages"),
    ("Source_Text", "list of aadlstring"),
    ("Source_Name", "aadlstring"),
    ("Type_Source_Name", "aadlstring"),
    ("Hardware_Description_Source_Text", "list of aadlstring"),
    (
        "Hardware_Source_Language",
        "Supported_Hardware_Source_Languages",
    ),
    ("Device_Driver", "classifier (abstract implementation)"),
    (
        "Initialize_Entrypoint",
        "classifier (subprogram classifier)",
    ),
    (
        "Initialize_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Initialize_Entrypoint_Source_Text", "aadlstring"),
    ("Compute_Entrypoint", "classifier (subprogram classifier)"),
    (
        "Compute_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Compute_Entrypoint_Source_Text", "aadlstring"),
    ("Activate_Entrypoint", "classifier (subprogram classifier)"),
    (
        "Activate_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Activate_Entrypoint_Source_Text", "aadlstring"),
    (
        "Deactivate_Entrypoint",
        "classifier (subprogram classifier)",
    ),
    (
        "Deactivate_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Deactivate_Entrypoint_Source_Text", "aadlstring"),
    ("Finalize_Entrypoint", "classifier (subprogram classifier)"),
    (
        "Finalize_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Finalize_Entrypoint_Source_Text", "aadlstring"),
    ("Recover_Entrypoint", "classifier (subprogram classifier)"),
    (
        "Recover_Entrypoint_Call_Sequence",
        "classifier (subprogram classifier)",
    ),
    ("Recover_Entrypoint_Source_Text", "aadlstring"),
];

// ── Modeling_Properties ─────────────────────────────────────────────

const MODELING_PROPERTIES: &[(&str, &str)] = &[
    (
        "Classifier_Matching_Rule",
        "enumeration (Classifier_Match, Equivalence, Subset, Conversion, Complement)",
    ),
    (
        "Classifier_Substitution_Rule",
        "enumeration (Classifier_Match, Type_Extension, Signature_Match)",
    ),
    (
        "Prototype_Substitution_Rule",
        "enumeration (Classifier_Match, Type_Extension, Signature_Match)",
    ),
    ("Implemented_As", "classifier (abstract implementation)"),
];

// ── AADL_Project ────────────────────────────────────────────────────

const AADL_PROJECT: &[(&str, &str)] = &[
    (
        "Supported_Dispatch_Protocols",
        "list of Supported_Dispatch_Protocols",
    ),
    (
        "Supported_Scheduling_Protocols",
        "list of Supported_Scheduling_Protocols",
    ),
    (
        "Supported_Source_Languages",
        "list of Supported_Source_Languages",
    ),
    (
        "Supported_Hardware_Source_Languages",
        "list of Supported_Hardware_Source_Languages",
    ),
    ("Max_Thread_Compute_Execution_Time", "Time"),
    ("Max_Urgency", "aadlinteger"),
    ("Max_Byte_Count", "aadlinteger"),
    ("Max_Word_Space", "aadlinteger"),
    ("Max_Memory_Size", "Size"),
    ("Max_Queue_Size", "aadlinteger"),
    ("Data_Volume", "aadlinteger units Data_Volume_Units"),
    (
        "Supported_Connection_Patterns",
        "enumeration (One_To_One, All_To_All, One_To_All, All_To_One, Next, Previous, Cyclic_Next, Cyclic_Previous)",
    ),
    (
        "Supported_Active_Thread_Handling_Protocols",
        "enumeration (abort, complete_one_flush_queue, complete_one_transfer_queue, complete_one_stop, complete_all, stop)",
    ),
];

/// Helper: collect properties from a table into the result vector.
fn collect_properties(
    table: &[(&'static str, &'static str)],
    property_set: &'static str,
    result: &mut Vec<StandardProperty>,
) {
    for &(name, ty) in table {
        result.push(StandardProperty {
            name,
            property_set,
            type_description: ty,
        });
    }
}

/// Return all standard properties across all predefined property sets.
pub fn all_standard_properties() -> Vec<StandardProperty> {
    let mut result = Vec::new();

    collect_properties(TIMING_PROPERTIES, "Timing_Properties", &mut result);
    collect_properties(
        COMMUNICATION_PROPERTIES,
        "Communication_Properties",
        &mut result,
    );
    collect_properties(MEMORY_PROPERTIES, "Memory_Properties", &mut result);
    collect_properties(DEPLOYMENT_PROPERTIES, "Deployment_Properties", &mut result);
    collect_properties(THREAD_PROPERTIES, "Thread_Properties", &mut result);
    collect_properties(
        PROGRAMMING_PROPERTIES,
        "Programming_Properties",
        &mut result,
    );
    collect_properties(MODELING_PROPERTIES, "Modeling_Properties", &mut result);
    collect_properties(AADL_PROJECT, "AADL_Project", &mut result);

    result
}

/// Check if a property set name is a standard predefined set.
///
/// Comparison is case-insensitive per the AADL spec.
pub fn is_standard_property_set(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    STANDARD_PROPERTY_SET_NAMES
        .iter()
        .any(|s| s.to_ascii_lowercase() == lower)
}

/// Map a lowercased property set name to its table of properties.
fn lookup_table(set_lower: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match set_lower {
        "timing_properties" => Some(TIMING_PROPERTIES),
        "communication_properties" => Some(COMMUNICATION_PROPERTIES),
        "memory_properties" => Some(MEMORY_PROPERTIES),
        "deployment_properties" => Some(DEPLOYMENT_PROPERTIES),
        "thread_properties" => Some(THREAD_PROPERTIES),
        "programming_properties" => Some(PROGRAMMING_PROPERTIES),
        "modeling_properties" => Some(MODELING_PROPERTIES),
        "aadl_project" => Some(AADL_PROJECT),
        _ => None,
    }
}

/// Get all property names in a standard property set.
///
/// Returns an empty vec if `set_name` is not a standard set.
/// Comparison is case-insensitive.
pub fn standard_properties_in_set(set_name: &str) -> Vec<&'static str> {
    let lower = set_name.to_ascii_lowercase();
    match lookup_table(&lower) {
        Some(table) => table.iter().map(|&(name, _)| name).collect(),
        None => Vec::new(),
    }
}

/// Get the type description for a specific standard property.
///
/// Returns `None` if the property set or property name is not found.
/// Comparison is case-insensitive.
pub fn standard_property_type(set_name: &str, property_name: &str) -> Option<&'static str> {
    let set_lower = set_name.to_ascii_lowercase();
    let prop_lower = property_name.to_ascii_lowercase();

    lookup_table(&set_lower)?
        .iter()
        .find(|&&(name, _)| name.to_ascii_lowercase() == prop_lower)
        .map(|&(_, ty)| ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_standard_property_set() {
        assert!(is_standard_property_set("Timing_Properties"));
        assert!(is_standard_property_set("Communication_Properties"));
        assert!(is_standard_property_set("Memory_Properties"));
        assert!(is_standard_property_set("Deployment_Properties"));
        assert!(is_standard_property_set("Thread_Properties"));
        assert!(is_standard_property_set("Programming_Properties"));
        assert!(is_standard_property_set("Modeling_Properties"));
        assert!(is_standard_property_set("AADL_Project"));

        // Case-insensitive
        assert!(is_standard_property_set("timing_properties"));
        assert!(is_standard_property_set("TIMING_PROPERTIES"));
        assert!(is_standard_property_set("Timing_PROPERTIES"));
        assert!(is_standard_property_set("aadl_project"));
        assert!(is_standard_property_set("AADL_PROJECT"));

        // Not standard
        assert!(!is_standard_property_set("Custom_Properties"));
        assert!(!is_standard_property_set(""));
        assert!(!is_standard_property_set("Timing"));
    }

    #[test]
    fn test_standard_properties_in_timing() {
        let props = standard_properties_in_set("Timing_Properties");
        assert_eq!(props.len(), 12);
        assert!(props.contains(&"Period"));
        assert!(props.contains(&"Deadline"));
        assert!(props.contains(&"Compute_Execution_Time"));
        assert!(props.contains(&"Clock_Period"));
        assert!(props.contains(&"Reference_Time"));
        assert!(props.contains(&"Startup_Execution_Time"));
        assert!(props.contains(&"Clock_Jitter"));
        assert!(props.contains(&"First_Dispatch_Time"));
        assert!(props.contains(&"Dispatch_Jitter"));
        assert!(props.contains(&"Dispatch_Offset"));
        assert!(props.contains(&"Execution_Time"));
        assert!(props.contains(&"Process_Swap_Execution_Time"));
    }

    #[test]
    fn test_standard_properties_in_communication() {
        let props = standard_properties_in_set("Communication_Properties");
        assert_eq!(props.len(), 13);
        assert!(props.contains(&"Fan_Out_Policy"));
        assert!(props.contains(&"Queue_Size"));
        assert!(props.contains(&"Queue_Processing_Protocol"));
        assert!(props.contains(&"Overflow_Handling_Protocol"));
        assert!(props.contains(&"Transmission_Type"));
        assert!(props.contains(&"Input_Rate"));
        assert!(props.contains(&"Output_Rate"));
        assert!(props.contains(&"Data_Rate"));
        assert!(props.contains(&"Connection_Pattern"));
        assert!(props.contains(&"Connection_Set"));
        assert!(props.contains(&"Latency"));
        assert!(props.contains(&"Actual_Latency"));
        assert!(props.contains(&"Required_Connection"));
    }

    #[test]
    fn test_standard_properties_in_memory() {
        let props = standard_properties_in_set("Memory_Properties");
        assert_eq!(props.len(), 14);
        assert!(props.contains(&"Data_Size"));
        assert!(props.contains(&"Code_Size"));
        assert!(props.contains(&"Stack_Size"));
        assert!(props.contains(&"Heap_Size"));
        assert!(props.contains(&"Byte_Count"));
        assert!(props.contains(&"Word_Size"));
        assert!(props.contains(&"Word_Space"));
        assert!(props.contains(&"Base_Address"));
        assert!(props.contains(&"Source_Code_Size"));
        assert!(props.contains(&"Source_Data_Size"));
        assert!(props.contains(&"Source_Stack_Size"));
        assert!(props.contains(&"Source_Heap_Size"));
        assert!(props.contains(&"Runtime_Protection"));
        assert!(props.contains(&"Read_Only"));
    }

    #[test]
    fn test_standard_properties_in_deployment() {
        let props = standard_properties_in_set("Deployment_Properties");
        assert_eq!(props.len(), 14);
        assert!(props.contains(&"Allowed_Processor_Binding"));
        assert!(props.contains(&"Actual_Processor_Binding"));
        assert!(props.contains(&"Allowed_Memory_Binding"));
        assert!(props.contains(&"Actual_Memory_Binding"));
        assert!(props.contains(&"Allowed_Connection_Binding"));
        assert!(props.contains(&"Actual_Connection_Binding"));
        assert!(props.contains(&"Allowed_Dispatch_Protocol"));
        assert!(props.contains(&"Actual_Subprogram_Call"));
        assert!(props.contains(&"Actual_Subprogram_Call_Binding"));
        assert!(props.contains(&"Preemptive_Scheduler"));
        assert!(props.contains(&"Scheduling_Protocol"));
        assert!(props.contains(&"Not_Collocated"));
        assert!(props.contains(&"Priority_Map"));
        assert!(props.contains(&"Priority_Range"));
    }

    #[test]
    fn test_standard_properties_in_thread() {
        let props = standard_properties_in_set("Thread_Properties");
        assert_eq!(props.len(), 7);
        assert!(props.contains(&"Dispatch_Protocol"));
        assert!(props.contains(&"Dispatch_Trigger"));
        assert!(props.contains(&"Priority"));
        assert!(props.contains(&"Criticality"));
        assert!(props.contains(&"POSIX_Scheduling_Policy"));
        assert!(props.contains(&"Active_Thread_Handling_Protocol"));
        assert!(props.contains(&"Active_Thread_Queue_Handling_Protocol"));
    }

    #[test]
    fn test_standard_properties_in_programming() {
        let props = standard_properties_in_set("Programming_Properties");
        assert_eq!(props.len(), 25);
        assert!(props.contains(&"Source_Language"));
        assert!(props.contains(&"Source_Text"));
        assert!(props.contains(&"Source_Name"));
        assert!(props.contains(&"Type_Source_Name"));
        assert!(props.contains(&"Hardware_Description_Source_Text"));
        assert!(props.contains(&"Hardware_Source_Language"));
        assert!(props.contains(&"Device_Driver"));
        assert!(props.contains(&"Initialize_Entrypoint"));
        assert!(props.contains(&"Initialize_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Initialize_Entrypoint_Source_Text"));
        assert!(props.contains(&"Compute_Entrypoint"));
        assert!(props.contains(&"Compute_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Compute_Entrypoint_Source_Text"));
        assert!(props.contains(&"Activate_Entrypoint"));
        assert!(props.contains(&"Activate_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Activate_Entrypoint_Source_Text"));
        assert!(props.contains(&"Deactivate_Entrypoint"));
        assert!(props.contains(&"Deactivate_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Deactivate_Entrypoint_Source_Text"));
        assert!(props.contains(&"Finalize_Entrypoint"));
        assert!(props.contains(&"Finalize_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Finalize_Entrypoint_Source_Text"));
        assert!(props.contains(&"Recover_Entrypoint"));
        assert!(props.contains(&"Recover_Entrypoint_Call_Sequence"));
        assert!(props.contains(&"Recover_Entrypoint_Source_Text"));
    }

    #[test]
    fn test_standard_properties_in_modeling() {
        let props = standard_properties_in_set("Modeling_Properties");
        assert_eq!(props.len(), 4);
        assert!(props.contains(&"Classifier_Matching_Rule"));
        assert!(props.contains(&"Classifier_Substitution_Rule"));
        assert!(props.contains(&"Prototype_Substitution_Rule"));
        assert!(props.contains(&"Implemented_As"));
    }

    #[test]
    fn test_standard_properties_in_aadl_project() {
        let props = standard_properties_in_set("AADL_Project");
        assert_eq!(props.len(), 13);
        assert!(props.contains(&"Supported_Dispatch_Protocols"));
        assert!(props.contains(&"Supported_Scheduling_Protocols"));
        assert!(props.contains(&"Supported_Source_Languages"));
        assert!(props.contains(&"Supported_Hardware_Source_Languages"));
        assert!(props.contains(&"Max_Thread_Compute_Execution_Time"));
        assert!(props.contains(&"Max_Urgency"));
        assert!(props.contains(&"Max_Byte_Count"));
        assert!(props.contains(&"Max_Word_Space"));
        assert!(props.contains(&"Max_Memory_Size"));
        assert!(props.contains(&"Max_Queue_Size"));
        assert!(props.contains(&"Data_Volume"));
        assert!(props.contains(&"Supported_Connection_Patterns"));
        assert!(props.contains(&"Supported_Active_Thread_Handling_Protocols"));
    }

    #[test]
    fn test_standard_properties_unknown_set() {
        let props = standard_properties_in_set("Nonexistent_Properties");
        assert!(props.is_empty());
    }

    #[test]
    fn test_standard_properties_case_insensitive() {
        let props = standard_properties_in_set("timing_properties");
        assert_eq!(props.len(), 12);
        assert!(props.contains(&"Period"));
    }

    #[test]
    fn test_all_standard_properties_total_count() {
        let all = all_standard_properties();
        // 12 + 13 + 14 + 14 + 7 + 25 + 4 + 13 = 102
        assert_eq!(all.len(), 102);
    }

    #[test]
    fn test_all_standard_properties_have_correct_sets() {
        let all = all_standard_properties();
        for prop in &all {
            assert!(
                is_standard_property_set(prop.property_set),
                "property {} claims to be in set {} which is not standard",
                prop.name,
                prop.property_set
            );
        }
    }

    #[test]
    fn test_standard_property_type_lookup() {
        assert_eq!(
            standard_property_type("Timing_Properties", "Period"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("Timing_Properties", "Compute_Execution_Time"),
            Some("Time_Range")
        );
        assert_eq!(
            standard_property_type("Memory_Properties", "Data_Size"),
            Some("Size")
        );
        assert_eq!(
            standard_property_type("Thread_Properties", "Priority"),
            Some("aadlinteger")
        );

        // New property sets
        assert_eq!(
            standard_property_type("Programming_Properties", "Source_Language"),
            Some("list of Supported_Source_Languages")
        );
        assert_eq!(
            standard_property_type("Programming_Properties", "Compute_Entrypoint"),
            Some("classifier (subprogram classifier)")
        );
        assert_eq!(
            standard_property_type("Modeling_Properties", "Classifier_Matching_Rule"),
            Some("enumeration (Classifier_Match, Equivalence, Subset, Conversion, Complement)")
        );
        assert_eq!(
            standard_property_type("AADL_Project", "Max_Memory_Size"),
            Some("Size")
        );

        // New properties in existing sets
        assert_eq!(
            standard_property_type("Timing_Properties", "Clock_Jitter"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("Communication_Properties", "Latency"),
            Some("Time_Range")
        );
        assert_eq!(
            standard_property_type("Memory_Properties", "Base_Address"),
            Some("aadlinteger")
        );
        assert_eq!(
            standard_property_type("Deployment_Properties", "Preemptive_Scheduler"),
            Some("aadlboolean")
        );

        // Case-insensitive
        assert_eq!(
            standard_property_type("timing_properties", "period"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("programming_properties", "source_language"),
            Some("list of Supported_Source_Languages")
        );
        assert_eq!(
            standard_property_type("aadl_project", "max_urgency"),
            Some("aadlinteger")
        );

        // Not found
        assert_eq!(
            standard_property_type("Timing_Properties", "Nonexistent"),
            None
        );
        assert_eq!(standard_property_type("Nonexistent", "Period"), None);
    }

    #[test]
    fn test_standard_properties_resolved_via_global_scope() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        // Build a GlobalScope with no item trees — standard properties
        // should still be registered automatically.
        let scope = GlobalScope::from_trees(vec![]);

        // Resolve Timing_Properties::Period
        let result = scope.resolve_property(&Name::new("Timing_Properties"), &Name::new("Period"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef, got {:?}",
            result
        );

        // Resolve Deployment_Properties::Actual_Processor_Binding
        let result = scope.resolve_property(
            &Name::new("Deployment_Properties"),
            &Name::new("Actual_Processor_Binding"),
        );
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef, got {:?}",
            result
        );

        // Resolve Thread_Properties::Priority
        let result =
            scope.resolve_property(&Name::new("Thread_Properties"), &Name::new("Priority"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef, got {:?}",
            result
        );

        // Resolve new property sets
        let result = scope.resolve_property(
            &Name::new("Programming_Properties"),
            &Name::new("Source_Language"),
        );
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for Programming_Properties::Source_Language, got {:?}",
            result
        );

        let result = scope.resolve_property(
            &Name::new("Modeling_Properties"),
            &Name::new("Classifier_Matching_Rule"),
        );
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for Modeling_Properties::Classifier_Matching_Rule, got {:?}",
            result
        );

        let result =
            scope.resolve_property(&Name::new("AADL_Project"), &Name::new("Max_Memory_Size"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for AADL_Project::Max_Memory_Size, got {:?}",
            result
        );

        // Case-insensitive resolution
        let result = scope.resolve_property(&Name::new("timing_properties"), &Name::new("period"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected case-insensitive match, got {:?}",
            result
        );

        let result = scope.resolve_property(&Name::new("aadl_project"), &Name::new("max_urgency"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected case-insensitive match for AADL_Project, got {:?}",
            result
        );

        // Unknown property in a standard set should be Unresolved
        let result =
            scope.resolve_property(&Name::new("Timing_Properties"), &Name::new("Nonexistent"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved, got {:?}",
            result
        );

        // Unknown property set should be Unresolved
        let result = scope.resolve_property(&Name::new("Custom_Properties"), &Name::new("Foo"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved, got {:?}",
            result
        );
    }
}
