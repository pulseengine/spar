//! Constraint extraction from AADL instance model properties.
//!
//! Walks all component instances and extracts:
//! - **Thread constraints**: Period, WCET, Deadline, Priority, processor binding
//! - **Processor constraints**: Memory_Size
//!
//! Safety requirements:
//! - SOLVER-REQ-020: Missing Compute_Execution_Time on a periodic thread
//!   produces an explicit warning (never silently defaults).
//! - SOLVER-REQ-023: Output is sorted by component name for determinism.

use serde::Serialize;

use spar_analysis::property_accessors::{
    get_execution_time, get_processor_binding, get_size_property, get_timing_property,
};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

/// Timing and binding constraints for a single thread instance.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadConstraint {
    /// Arena index of this thread in the instance model.
    #[serde(skip)]
    pub idx: ComponentInstanceIdx,
    /// Dot-separated path name of the thread instance.
    pub name: String,
    /// Period in picoseconds (0 = missing).
    pub period_ps: u64,
    /// Worst-case execution time in picoseconds (0 = missing).
    pub wcet_ps: u64,
    /// Deadline in picoseconds (defaults to period when absent).
    pub deadline_ps: u64,
    /// Existing processor binding, if any.
    pub current_binding: Option<String>,
    /// Thread priority (from `Deployment_Properties::Priority`).
    pub priority: Option<u64>,
}

/// Resource constraints for a single processor instance.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessorConstraint {
    /// Arena index of this processor in the instance model.
    #[serde(skip)]
    pub idx: ComponentInstanceIdx,
    /// Dot-separated path name of the processor instance.
    pub name: String,
    /// Memory capacity in bytes (from `Memory_Size` property).
    pub memory_bytes: Option<u64>,
}

/// Extracted constraints from an entire AADL instance model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelConstraints {
    /// Thread timing/binding constraints, sorted by name (SOLVER-REQ-023).
    pub threads: Vec<ThreadConstraint>,
    /// Processor resource constraints, sorted by name (SOLVER-REQ-023).
    pub processors: Vec<ProcessorConstraint>,
    /// Warnings about missing or incomplete properties.
    pub warnings: Vec<String>,
}

impl ModelConstraints {
    /// Extract constraints from a fully instantiated AADL system.
    ///
    /// Walks every component instance:
    /// - Threads yield [`ThreadConstraint`] entries.
    /// - Processors yield [`ProcessorConstraint`] entries.
    ///
    /// Warnings are emitted for missing critical properties per STPA
    /// safety requirements.
    pub fn from_instance(instance: &SystemInstance) -> Self {
        let mut threads = Vec::new();
        let mut processors = Vec::new();
        let mut warnings = Vec::new();

        for (idx, comp) in instance.all_components() {
            let props = instance.properties_for(idx);
            let name = component_path(instance, idx);

            match comp.category {
                ComponentCategory::Thread => {
                    let period_ps = get_timing_property(props, "Period").unwrap_or(0);
                    let wcet_ps = get_execution_time(props).unwrap_or(0);
                    let deadline_ps = get_timing_property(props, "Deadline").unwrap_or(period_ps);
                    let current_binding = get_processor_binding(props);
                    let priority = get_priority(props);

                    // SOLVER-REQ-020: warn on missing WCET for periodic threads
                    if period_ps > 0 && wcet_ps == 0 {
                        warnings.push(format!(
                            "thread '{}': missing Compute_Execution_Time \
                             — assuming zero (UNSAFE)",
                            name,
                        ));
                    }

                    // Warn on threads with no Period at all
                    if period_ps == 0 {
                        warnings.push(format!(
                            "thread '{}': missing Period — cannot compute \
                             scheduling constraints",
                            name,
                        ));
                    }

                    threads.push(ThreadConstraint {
                        idx,
                        name,
                        period_ps,
                        wcet_ps,
                        deadline_ps,
                        current_binding,
                        priority,
                    });
                }
                ComponentCategory::Processor => {
                    // Memory_Size is returned in bits; convert to bytes.
                    let memory_bits = get_size_property(props, "Memory_Size");
                    let memory_bytes = memory_bits.map(|b| b / 8);

                    processors.push(ProcessorConstraint {
                        idx,
                        name,
                        memory_bytes,
                    });
                }
                _ => {}
            }
        }

        // SOLVER-REQ-023: deterministic output ordering
        threads.sort_by(|a, b| a.name.cmp(&b.name));
        processors.sort_by(|a, b| a.name.cmp(&b.name));
        warnings.sort();

        ModelConstraints {
            threads,
            processors,
            warnings,
        }
    }
}

/// Build a dot-separated path for a component instance by walking parents.
fn component_path(instance: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    let mut segments = Vec::new();
    let mut current = Some(idx);
    while let Some(ci) = current {
        let comp = instance.component(ci);
        segments.push(comp.name.as_str().to_string());
        current = comp.parent;
    }
    segments.reverse();
    segments.join(".")
}

/// Read the `Deployment_Properties::Priority` value.
fn get_priority(props: &spar_hir_def::properties::PropertyMap) -> Option<u64> {
    let raw = props
        .get("Deployment_Properties", "Priority")
        .or_else(|| props.get("", "Priority"))?;
    raw.trim().parse::<u64>().ok()
}
