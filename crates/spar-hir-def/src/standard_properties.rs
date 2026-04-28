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
///
/// Includes the AS5506 Appendix A predeclared sets plus three non-standard
/// spar-defined sets (`Spar_Timing`, `Spar_Trace`, `Spar_Network`) that
/// support IRQ-aware RTA (Track A, v0.7.0), closed-loop trace verification
/// (v0.8.0 precursor), and TSN/Ethernet WCTT analysis (Track D, v0.8.0).
/// The spar-defined sets are treated like predefined sets so they resolve
/// without explicit `with` imports.
pub const STANDARD_PROPERTY_SET_NAMES: &[&str] = &[
    "Timing_Properties",
    "Communication_Properties",
    "Memory_Properties",
    "Deployment_Properties",
    "Thread_Properties",
    "Programming_Properties",
    "Modeling_Properties",
    "AADL_Project",
    "Spar_Timing",
    "Spar_Trace",
    "Spar_Network",
    "Spar_Migration",
    "Spar_Power",
    "Spar_TSN",
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
    // AS5506D §5.4.4: concurrency control protocol used for shared
    // resources accessed by this thread. `Priority_Inheritance_Protocol`
    // and `Priority_Ceiling_Protocol` enable PIP/PCP blocking analysis
    // in the v0.7.1 hierarchical RTA; `Stop_For_Lock` and `None` opt out.
    (
        "Locking_Protocol",
        "enumeration (Priority_Ceiling_Protocol, Priority_Inheritance_Protocol, Stop_For_Lock, None)",
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

// ── Spar_Timing ─────────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// for IRQ-aware RTA (Track A, v0.7.0). Models the interrupt layer so
// hierarchical RTA can reason about ISR priority, the ISR body's own
// BCET..WCET, the hardware-dispatch latency bound, and the top-half /
// bottom-half deferred-work split.

const SPAR_TIMING: &[(&str, &str)] = &[
    // Priority at which the ISR executes (higher than any task).
    // Drives hierarchical RTA layer ordering. Applies to thread
    // (handler thread) and virtual processor (ISR layer).
    ("ISR_Priority", "aadlinteger"),
    // BCET..WCET of the ISR body itself, separately from the handler
    // thread. Applies to thread and device.
    ("ISR_Execution_Time", "Time_Range"),
    // Platform-given upper bound on IRQ-assertion → ISR-entry
    // (hardware + kernel dispatch). Drives the "from wire to handler"
    // latency chain. Applies to processor and device.
    ("Interrupt_Latency_Bound", "Time"),
    // Reference to the thread that handles deferred ISR work (classic
    // top-half / bottom-half split). Applies to thread and device.
    ("Bottom_Half_Server", "reference (thread)"),
    // User-supplied bound on how long a higher-priority task can be
    // blocked by lower-priority tasks holding shared resources, under
    // the configured Thread_Properties::Locking_Protocol (PIP/PCP).
    // Drives the B_i term in the v0.7.1 hierarchical RTA recurrence.
    // Applies to thread.
    ("Critical_Section_Blocking", "Time"),
];

// ── Spar_Trace ──────────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// for closed-loop trace verification (v0.8.0 precursor). Annotates
// components whose entry/exit codegen should emit a runtime trace
// event and carries design-time predictions against which the observed
// traces are compared by `spar verify-trace`.

const SPAR_TRACE: &[(&str, &str)] = &[
    // Flags a component whose entry/exit codegen should emit a trace
    // event (Zephyr CTF k_*-primitive style in v0.8.0).
    ("Probe_Point", "aadlboolean"),
    // Design-time best-case prediction, separate from
    // Compute_Execution_Time because these are predictions for runtime
    // comparison, not the declared WCET the scheduler uses.
    ("Expected_BCET", "Time"),
    // Design-time worst-case prediction (runtime-comparison only).
    ("Expected_WCET", "Time"),
    // Design-time mean/expected prediction (runtime-comparison only).
    ("Expected_Mean", "Time"),
];

// ── Spar_Network ────────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// for TSN/Ethernet WCTT analysis (Track D, v0.8.0). Provides the AADL
// vocabulary for switch modeling under the Option C decision in
// research PR #152: a switched bus is modeled as
// `bus implementation` carrying a `Switch_Type` discriminator.
//
// Phase 1 (this milestone) covers FIFO + Priority networks. Phase 2's
// TSN-specific properties land in a separate `Spar_TSN::*` set.
//
// See `docs/designs/track-d-tsn-wctt-research.md` §5.1.

const SPAR_NETWORK: &[(&str, &str)] = &[
    // Discriminator for the bus's forwarding discipline. `FIFO` and
    // `Priority` cover Phase 1 (classical Ethernet, CAN, FlexRay
    // priority-based). `TSN` is reserved for Phase 2's scheduled-traffic
    // service curves; analysis passes treat it as opaque until the
    // Spar_TSN property set lands.
    ("Switch_Type", "enumeration (FIFO, Priority, TSN)"),
    // Per-port queue capacity in frames. Bounds the burst that can
    // accumulate at a switch egress before drops; an input to the
    // backlog bound used by the WCTT analysis.
    ("Queue_Depth", "aadlinteger"),
    // Store-and-forward latency: per-hop best-case .. worst-case
    // contribution from switch fabric and MAC processing. Modeled as
    // Time_Range so analyses can preserve BCET..WCET separation.
    ("Forwarding_Latency", "Time_Range"),
    // Egress link bandwidth per port. Note: AADL's `Data_Rate` is a
    // unit-typed integer (`aadlinteger units Data_Rate_Units`) in
    // Communication_Properties; the type description here is the
    // human-readable form. Proper unit-aware parsing of `Data_Rate`
    // is deferred to the WCTT analysis pass (Track D commit 4).
    ("Output_Rate", "Data_Rate"),
    // Per-bus end-to-end WCTT budget. When set on a switched bus, the
    // Track D commit 4 `wctt.rs` analysis pass compares each predicted
    // per-stream end-to-end traversal-time bound against this budget
    // and emits a `WcttExceedsBudget` Error when the prediction
    // exceeds it. Modeled as Time so it lowers to picoseconds via the
    // existing AADL time-unit machinery.
    ("WCTT_Budget", "Time"),
];

// ── Spar_Migration ──────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// for the v0.8.0 Track E "frozen-platform / mobile-application" split
// and the hypothetical-rebinding oracle. Provides the AADL vocabulary
// for declaring which items are platform (immutable for hypothetical
// rebinding) vs. which are application (eligible for movement). Per
// the design research in `docs/designs/track-e-migration-research.md`
// §6.1.

const SPAR_MIGRATION: &[(&str, &str)] = &[
    // Marks the item as platform — its binding/properties cannot be
    // altered by hypothetical-rebinding queries. Default false. Applies
    // to every component category (process, processor, memory, bus,
    // device, system, thread, …).
    ("Frozen", "aadlboolean"),
    // Marks the item as application — eligible for hypothetical
    // rebinding. Default false. Mutually inconsistent with Frozen=true;
    // when both are set Frozen wins (defensive; see is_frozen helper).
    ("Mobile", "aadlboolean"),
    // Enumerates the set of valid rebinding targets. Empty list = no
    // restriction beyond the platform's frozen set.
    //
    // TODO(v0.9.0): broaden to "list of reference (component)" once the
    // AADL type table supports the generic-component reference form.
    // Today we register as `list of reference (thread)` — the most
    // common case for hypothetical rebinding (threads are what get
    // rebound). Per §6.1 of the migration research; the property set
    // surface is permissive at the registration level since the
    // declarative type only gates parser-level type checking, and
    // analyses read the references untyped via PropertyExpr.
    ("Allowed_Targets", "list of reference (thread)"),
    // Human-readable reason for `Frozen => true`. For audit trails.
    ("Pinned_Reason", "aadlstring"),
];

// ── Spar_Power ──────────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// by the v0.8.0 Track E commit 5/8 multi-objective enumeration ranker
// (`spar moves enumerate --objective total-power`). Sits alongside the
// existing `SEI::PowerBudget` / `Physical_Properties::Power_Budget`
// vocabulary read by `spar-analysis::weight_power` so users with neither
// of those upstream sets in scope can still annotate per-component
// power consumption for design-space ranking.
//
// Modeled as `Time` so the value lowers to picoseconds via the existing
// AADL time-unit machinery — semantically the value carries milliwatts
// and is read with [`crate::power::read_power_budget_mw`], but using
// `Time` for the declarative type lets us reuse the time-aware lowerer
// without inventing a new "Power" base type for v0.8.0. The `mw`
// suffix is documented in the helper, not enforced at the
// property-set level.

const SPAR_POWER: &[(&str, &str)] = &[
    // Per-component power-budget annotation in milliwatts. Read by the
    // multi-objective ranker so users can score candidate bindings on
    // total power. Optional — when absent the candidate's power
    // contribution to the score is zero.
    ("Power_Budget", "Time"),
];

// ── Spar_TSN ────────────────────────────────────────────────────────
//
// Non-standard property set defined by spar itself (not AS5506); used
// for Time-Sensitive Networking (TSN) WCTT analysis (Track D Phase 2,
// v0.8.1+). Provides the AADL vocabulary for TSN-shaped service
// curves: TAS gate-control schedules (802.1Qbv), CBS credit-pool
// classes (802.1Qav), and frame preemption (802.1Qbu). Sits alongside
// `Spar_Network::*` (Phase 1, switch-discriminator + classical
// FIFO/Priority disciplines).
//
// v0.8.1 commit 1 ships the property surface only; subsequent commits
// in the v0.8.1 series add the analysis math (TAS gate-window service
// curves, CBS credit accounting, frame-preemption hooks). See
// `docs/designs/track-d-tsn-wctt-research.md` §5.1 (property design)
// and §5.2 (switch modeling).

const SPAR_TSN: &[(&str, &str)] = &[
    // Per-stream identifier required by TAS gate-control lists and
    // by stream reservation (802.1Qcc). Applies to port and connection.
    ("Stream_ID", "aadlinteger 0..2**32-1"),
    // 802.1Q priority class (0-7). Drives queue selection at switches
    // and gate-mask matching in the TAS gate-control list.
    // Applies to port and connection.
    ("Class_of_Service", "aadlinteger 0..7"),
    // Time-aware-shaper gate schedule — 802.1Qbv gate-control list.
    // For v0.8.1 commit 1 the value is parsed only as an opaque string
    // blob (the structured form lands in v0.8.1 commit 2 once the TAS
    // service-curve math is wired up).
    //
    // Future v0.9.x: structured form via Gate_Window record.
    //
    // Applies to bus.
    ("Gate_Control_List", "aadlstring"),
    // Maximum frame size in bytes (typed as `aadlinteger units
    // Size_Units` so it lowers to bytes via the existing AADL
    // size-unit machinery). Frame-preemption (802.1Qbu) and
    // serialization-time terms read this. Applies to port and
    // connection.
    ("Max_Frame_Size", "aadlinteger units Size_Units"),
    // Whether frames in this class can be pre-empted by Express
    // traffic (802.1Qbu). Applies to port and connection.
    ("Frame_Preemption", "aadlboolean"),
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
    collect_properties(SPAR_TIMING, "Spar_Timing", &mut result);
    collect_properties(SPAR_TRACE, "Spar_Trace", &mut result);
    collect_properties(SPAR_NETWORK, "Spar_Network", &mut result);
    collect_properties(SPAR_MIGRATION, "Spar_Migration", &mut result);
    collect_properties(SPAR_POWER, "Spar_Power", &mut result);
    collect_properties(SPAR_TSN, "Spar_TSN", &mut result);

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
        "spar_timing" => Some(SPAR_TIMING),
        "spar_trace" => Some(SPAR_TRACE),
        "spar_network" => Some(SPAR_NETWORK),
        "spar_migration" => Some(SPAR_MIGRATION),
        "spar_power" => Some(SPAR_POWER),
        "spar_tsn" => Some(SPAR_TSN),
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
        assert!(is_standard_property_set("Spar_Timing"));
        assert!(is_standard_property_set("Spar_Trace"));
        assert!(is_standard_property_set("Spar_Network"));
        assert!(is_standard_property_set("Spar_Migration"));
        assert!(is_standard_property_set("Spar_Power"));
        assert!(is_standard_property_set("Spar_TSN"));

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
        assert_eq!(props.len(), 8);
        assert!(props.contains(&"Dispatch_Protocol"));
        assert!(props.contains(&"Dispatch_Trigger"));
        assert!(props.contains(&"Priority"));
        assert!(props.contains(&"Criticality"));
        assert!(props.contains(&"POSIX_Scheduling_Policy"));
        assert!(props.contains(&"Active_Thread_Handling_Protocol"));
        assert!(props.contains(&"Active_Thread_Queue_Handling_Protocol"));
        assert!(props.contains(&"Locking_Protocol"));
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
    fn test_standard_properties_in_spar_timing() {
        // Spar_Timing is a known property set.
        assert!(is_standard_property_set("Spar_Timing"));

        let props = standard_properties_in_set("Spar_Timing");
        assert_eq!(props.len(), 5);
        assert!(props.contains(&"ISR_Priority"));
        assert!(props.contains(&"ISR_Execution_Time"));
        assert!(props.contains(&"Interrupt_Latency_Bound"));
        assert!(props.contains(&"Bottom_Half_Server"));
        assert!(props.contains(&"Critical_Section_Blocking"));

        // Each property resolves to its expected type.
        assert_eq!(
            standard_property_type("Spar_Timing", "ISR_Priority"),
            Some("aadlinteger")
        );
        assert_eq!(
            standard_property_type("Spar_Timing", "ISR_Execution_Time"),
            Some("Time_Range")
        );
        assert_eq!(
            standard_property_type("Spar_Timing", "Interrupt_Latency_Bound"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("Spar_Timing", "Bottom_Half_Server"),
            Some("reference (thread)")
        );
        assert_eq!(
            standard_property_type("Spar_Timing", "Critical_Section_Blocking"),
            Some("Time")
        );

        // Deliberately-wrong name returns None.
        assert_eq!(standard_property_type("Spar_Timing", "Nonexistent"), None);

        // Case-insensitive.
        assert_eq!(
            standard_property_type("spar_timing", "isr_priority"),
            Some("aadlinteger")
        );
    }

    #[test]
    fn test_standard_properties_in_spar_trace() {
        // Spar_Trace is a known property set.
        assert!(is_standard_property_set("Spar_Trace"));

        let props = standard_properties_in_set("Spar_Trace");
        assert_eq!(props.len(), 4);
        assert!(props.contains(&"Probe_Point"));
        assert!(props.contains(&"Expected_BCET"));
        assert!(props.contains(&"Expected_WCET"));
        assert!(props.contains(&"Expected_Mean"));

        // Each property resolves to its expected type.
        assert_eq!(
            standard_property_type("Spar_Trace", "Probe_Point"),
            Some("aadlboolean")
        );
        assert_eq!(
            standard_property_type("Spar_Trace", "Expected_BCET"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("Spar_Trace", "Expected_WCET"),
            Some("Time")
        );
        assert_eq!(
            standard_property_type("Spar_Trace", "Expected_Mean"),
            Some("Time")
        );

        // Deliberately-wrong name returns None.
        assert_eq!(standard_property_type("Spar_Trace", "Nonexistent"), None);

        // Case-insensitive.
        assert_eq!(
            standard_property_type("spar_trace", "probe_point"),
            Some("aadlboolean")
        );
    }

    #[test]
    fn test_standard_properties_in_spar_network() {
        // Spar_Network is a known property set.
        assert!(is_standard_property_set("Spar_Network"));

        let props = standard_properties_in_set("Spar_Network");
        assert_eq!(props.len(), 5);
        assert!(props.contains(&"Switch_Type"));
        assert!(props.contains(&"Queue_Depth"));
        assert!(props.contains(&"Forwarding_Latency"));
        assert!(props.contains(&"Output_Rate"));
        assert!(props.contains(&"WCTT_Budget"));

        // Each property resolves to its expected type.
        assert_eq!(
            standard_property_type("Spar_Network", "Switch_Type"),
            Some("enumeration (FIFO, Priority, TSN)")
        );
        assert_eq!(
            standard_property_type("Spar_Network", "Queue_Depth"),
            Some("aadlinteger")
        );
        assert_eq!(
            standard_property_type("Spar_Network", "Forwarding_Latency"),
            Some("Time_Range")
        );
        assert_eq!(
            standard_property_type("Spar_Network", "Output_Rate"),
            Some("Data_Rate")
        );
        assert_eq!(
            standard_property_type("Spar_Network", "WCTT_Budget"),
            Some("Time")
        );

        // Case-insensitive.
        assert_eq!(
            standard_property_type("spar_network", "switch_type"),
            Some("enumeration (FIFO, Priority, TSN)")
        );
    }

    #[test]
    fn test_spar_network_property_set_resolved_via_global_scope() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        let scope = GlobalScope::from_trees(vec![]);

        // Spar_Network::Switch_Type is resolvable without explicit `with`.
        let result = scope.resolve_property(&Name::new("Spar_Network"), &Name::new("Switch_Type"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for Spar_Network::Switch_Type, got {:?}",
            result
        );
    }

    #[test]
    fn test_spar_network_unknown_property_returns_none() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        // Lookup-table layer: unknown property in a known spar set is None.
        assert_eq!(standard_property_type("Spar_Network", "Nonexistent"), None);

        // Resolver layer: unknown property in a known spar set is Unresolved.
        let scope = GlobalScope::from_trees(vec![]);
        let result = scope.resolve_property(&Name::new("Spar_Network"), &Name::new("Nonexistent"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved for Spar_Network::Nonexistent, got {:?}",
            result
        );
    }

    #[test]
    fn test_spar_property_sets_resolved_via_global_scope() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        let scope = GlobalScope::from_trees(vec![]);

        // Spar_Timing::ISR_Priority is resolvable without explicit `with`.
        let result = scope.resolve_property(&Name::new("Spar_Timing"), &Name::new("ISR_Priority"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for Spar_Timing::ISR_Priority, got {:?}",
            result
        );

        // Spar_Trace::Probe_Point is resolvable without explicit `with`.
        let result = scope.resolve_property(&Name::new("Spar_Trace"), &Name::new("Probe_Point"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected PropertyDef for Spar_Trace::Probe_Point, got {:?}",
            result
        );

        // Deliberately-wrong name inside a known spar set is Unresolved.
        let result = scope.resolve_property(&Name::new("Spar_Timing"), &Name::new("Nonexistent"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved for Spar_Timing::Nonexistent, got {:?}",
            result
        );
    }

    #[test]
    fn test_standard_properties_in_spar_migration() {
        // Spar_Migration is a known property set.
        assert!(is_standard_property_set("Spar_Migration"));

        let props = standard_properties_in_set("Spar_Migration");
        assert_eq!(props.len(), 4);
        assert!(props.contains(&"Frozen"));
        assert!(props.contains(&"Mobile"));
        assert!(props.contains(&"Allowed_Targets"));
        assert!(props.contains(&"Pinned_Reason"));

        // Each property resolves to its expected type.
        assert_eq!(
            standard_property_type("Spar_Migration", "Frozen"),
            Some("aadlboolean")
        );
        assert_eq!(
            standard_property_type("Spar_Migration", "Mobile"),
            Some("aadlboolean")
        );
        assert_eq!(
            standard_property_type("Spar_Migration", "Allowed_Targets"),
            Some("list of reference (thread)")
        );
        assert_eq!(
            standard_property_type("Spar_Migration", "Pinned_Reason"),
            Some("aadlstring")
        );

        // Case-insensitive.
        assert_eq!(
            standard_property_type("spar_migration", "frozen"),
            Some("aadlboolean")
        );
        assert_eq!(
            standard_property_type("SPAR_MIGRATION", "PINNED_REASON"),
            Some("aadlstring")
        );
    }

    #[test]
    fn test_spar_migration_unknown_property_returns_none() {
        // Unknown property within a known spar_migration set.
        assert_eq!(
            standard_property_type("Spar_Migration", "Nonexistent"),
            None
        );
        assert_eq!(
            standard_property_type("Spar_Migration", "Migration_Cost"),
            None,
            "Migration_Cost is documented in §6.1 but deferred past commit 1; \
             must not resolve in the foundation property set"
        );
    }

    #[test]
    fn test_spar_migration_property_set_resolved_via_global_scope() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        let scope = GlobalScope::from_trees(vec![]);

        // Each Spar_Migration property is resolvable without explicit `with`.
        for prop_name in ["Frozen", "Mobile", "Allowed_Targets", "Pinned_Reason"] {
            let result =
                scope.resolve_property(&Name::new("Spar_Migration"), &Name::new(prop_name));
            assert!(
                matches!(result, ResolvedProperty::PropertyDef { .. }),
                "expected PropertyDef for Spar_Migration::{}, got {:?}",
                prop_name,
                result
            );
        }

        // Deliberately-wrong name inside a known spar set is Unresolved.
        let result =
            scope.resolve_property(&Name::new("Spar_Migration"), &Name::new("Nonexistent"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved for Spar_Migration::Nonexistent, got {:?}",
            result
        );

        // Case-insensitive resolution
        let result = scope.resolve_property(&Name::new("spar_migration"), &Name::new("frozen"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected case-insensitive match for Spar_Migration::Frozen, got {:?}",
            result
        );
    }

    #[test]
    fn test_standard_properties_in_spar_tsn() {
        // Spar_TSN is a known property set.
        assert!(is_standard_property_set("Spar_TSN"));

        let props = standard_properties_in_set("Spar_TSN");
        assert_eq!(props.len(), 5);
        assert!(props.contains(&"Stream_ID"));
        assert!(props.contains(&"Class_of_Service"));
        assert!(props.contains(&"Gate_Control_List"));
        assert!(props.contains(&"Max_Frame_Size"));
        assert!(props.contains(&"Frame_Preemption"));

        // Each property resolves to its expected type.
        assert_eq!(
            standard_property_type("Spar_TSN", "Stream_ID"),
            Some("aadlinteger 0..2**32-1")
        );
        assert_eq!(
            standard_property_type("Spar_TSN", "Class_of_Service"),
            Some("aadlinteger 0..7")
        );
        assert_eq!(
            standard_property_type("Spar_TSN", "Gate_Control_List"),
            Some("aadlstring")
        );
        assert_eq!(
            standard_property_type("Spar_TSN", "Max_Frame_Size"),
            Some("aadlinteger units Size_Units")
        );
        assert_eq!(
            standard_property_type("Spar_TSN", "Frame_Preemption"),
            Some("aadlboolean")
        );

        // Deliberately-wrong name returns None.
        assert_eq!(standard_property_type("Spar_TSN", "Nonexistent"), None);

        // Case-insensitive.
        assert_eq!(
            standard_property_type("spar_tsn", "stream_id"),
            Some("aadlinteger 0..2**32-1")
        );
        assert_eq!(
            standard_property_type("SPAR_TSN", "FRAME_PREEMPTION"),
            Some("aadlboolean")
        );
    }

    #[test]
    fn test_spar_tsn_property_set_resolved_via_global_scope() {
        use crate::name::Name;
        use crate::resolver::{GlobalScope, ResolvedProperty};

        let scope = GlobalScope::from_trees(vec![]);

        // Each Spar_TSN property is resolvable without explicit `with`.
        for prop_name in [
            "Stream_ID",
            "Class_of_Service",
            "Gate_Control_List",
            "Max_Frame_Size",
            "Frame_Preemption",
        ] {
            let result = scope.resolve_property(&Name::new("Spar_TSN"), &Name::new(prop_name));
            assert!(
                matches!(result, ResolvedProperty::PropertyDef { .. }),
                "expected PropertyDef for Spar_TSN::{}, got {:?}",
                prop_name,
                result
            );
        }

        // Deliberately-wrong name inside a known spar set is Unresolved.
        let result = scope.resolve_property(&Name::new("Spar_TSN"), &Name::new("Nonexistent"));
        assert!(
            matches!(result, ResolvedProperty::Unresolved),
            "expected Unresolved for Spar_TSN::Nonexistent, got {:?}",
            result
        );

        // Case-insensitive resolution.
        let result = scope.resolve_property(&Name::new("spar_tsn"), &Name::new("stream_id"));
        assert!(
            matches!(result, ResolvedProperty::PropertyDef { .. }),
            "expected case-insensitive match for Spar_TSN::Stream_ID, got {:?}",
            result
        );
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
        // 12 + 13 + 14 + 14 + 8 + 25 + 4 + 13 + 5 + 4 + 5 + 4 + 1 + 5 = 127
        // (Timing + Communication + Memory + Deployment + Thread + Programming
        //  + Modeling + AADL_Project + Spar_Timing + Spar_Trace + Spar_Network
        //  + Spar_Migration + Spar_Power + Spar_TSN)
        // Thread_Properties: +1 for Locking_Protocol (v0.7.1 PIP/PCP).
        // Spar_Timing: +1 for Critical_Section_Blocking (v0.7.1 PIP/PCP).
        // Spar_Network: +1 for WCTT_Budget (Track D commit 4).
        // Spar_Power: +1 for Power_Budget (Track E commit 5/8 ranker).
        // Spar_TSN: +5 for Stream_ID, Class_of_Service, Gate_Control_List,
        //   Max_Frame_Size, Frame_Preemption (Track D Phase 2 v0.8.1 c1).
        assert_eq!(all.len(), 127);
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
