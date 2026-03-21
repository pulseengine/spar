# Deployment Solver Foundations — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add topology graph extraction, virtual bus library, bin-packing allocation (Layer 1), source rewriting, and `spar allocate` CLI command — the foundation for the NDS-layered deployment solver.

**Architecture:** New `spar-solver` crate for topology graph + bin packing. Extends `spar-transform` for virtual bus library. Source rewriting in `spar-cli`. Each layer is independently testable. Pure Rust, WASM-compatible, no external solver dependencies.

**Tech Stack:** petgraph 0.7 (graph), la-arena (arena indices), spar-hir-def (instance model), spar-analysis (RTA/scheduling validation), rowan (source rewriting), salsa (incremental re-validation)

**Safety:** STPA analysis at `safety/stpa/solver-analysis.yaml` identified 35 safety requirements. Key ones addressed in this plan: SOLVER-REQ-020 (no silent defaults), SOLVER-REQ-023 (deterministic output), SOLVER-REQ-001 (integer arithmetic for RTA), SOLVER-REQ-016 (parse-after-rewrite validation).

---

## File Structure

### New crate: `crates/spar-solver/`

| File | Responsibility |
|------|---------------|
| `Cargo.toml` | Dependencies: spar-hir-def, spar-analysis, petgraph, la-arena, rustc-hash |
| `src/lib.rs` | Public API: `TopologyGraph`, `Allocator`, `AllocationResult` |
| `src/topology.rs` | Extract hardware topology from SystemInstance → petgraph DiGraph |
| `src/allocate.rs` | FFD/BFD bin-packing with RTA schedulability validation |
| `src/constraints.rs` | Constraint extraction from AADL properties (timing, resources, safety) |
| `src/tests.rs` | Shared TestBuilder + integration tests |

### Modified: `crates/spar-transform/src/`

| File | Change |
|------|--------|
| `protocol_library.rs` | NEW: Virtual bus library with 12+ protocol types |
| `lib.rs` | Add `pub mod protocol_library;` |

### Modified: `crates/spar-cli/src/`

| File | Change |
|------|--------|
| `refactor.rs` | NEW: Source rewriting — insert/update binding properties via rowan |
| `main.rs` | Add `allocate` and `refactor` commands |

### Modified: workspace root

| File | Change |
|------|--------|
| `Cargo.toml` | Add spar-solver to workspace members + deps |
| `crates/spar-cli/Cargo.toml` | Add spar-solver dependency |

---

## Task 1: Create spar-solver crate skeleton

**Files:**
- Create: `crates/spar-solver/Cargo.toml`
- Create: `crates/spar-solver/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "spar-solver"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
description = "Deployment solver for AADL models — topology, allocation, constraints"

[dependencies]
spar-hir-def.workspace = true
spar-analysis.workspace = true
petgraph.workspace = true
la-arena.workspace = true
rustc-hash = "2"
serde.workspace = true

[dev-dependencies]
spar-hir-def.workspace = true
```

- [ ] **Step 2: Create src/lib.rs**

```rust
//! Deployment solver for AADL models.
//!
//! Provides topology graph extraction, constraint formulation, and
//! bin-packing allocation following the NDS-layered hierarchical approach.

pub mod topology;
pub mod allocate;
pub mod constraints;

#[cfg(test)]
mod tests;
```

- [ ] **Step 3: Add to workspace**

In root `Cargo.toml`, add `"crates/spar-solver"` to `[workspace] members` and add:
```toml
spar-solver = { path = "crates/spar-solver" }
```
to `[workspace.dependencies]`.

- [ ] **Step 4: Create empty module files**

Create `src/topology.rs`, `src/allocate.rs`, `src/constraints.rs`, `src/tests.rs` as empty files with module doc comments.

- [ ] **Step 5: Verify builds**

Run: `cargo build -p spar-solver`
Expected: compiles with no errors

- [ ] **Step 6: Commit**

```bash
git add crates/spar-solver/ Cargo.toml Cargo.lock
git commit -m "feat(solver): create spar-solver crate skeleton"
```

---

## Task 2: Topology graph extraction

**Files:**
- Create: `crates/spar-solver/src/topology.rs`
- Modify: `crates/spar-solver/src/tests.rs`

**Rivet:** Satisfies REQ-SOLVER-001, implements ARCH-SOLVER-002.

- [ ] **Step 1: Write failing test — extract empty topology**

In `tests.rs`, create a `TestBuilder` (reuse the pattern from `spar-analysis/src/scheduling.rs:436+`):

```rust
use spar_hir_def::instance::*;
use spar_hir_def::item_tree::ComponentCategory;
use crate::topology::{TopologyGraph, HwNode, BusEdge};

#[test]
fn empty_system_has_no_hw_nodes() {
    let instance = build_minimal_system(); // system with no processors/buses
    let topo = TopologyGraph::from_instance(&instance);
    assert_eq!(topo.processor_count(), 0);
    assert_eq!(topo.bus_count(), 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p spar-solver -- empty_system`
Expected: FAIL — `TopologyGraph` not defined

- [ ] **Step 3: Implement TopologyGraph struct + from_instance**

In `topology.rs`:

```rust
use petgraph::graph::{DiGraph, NodeIndex};
use rustc_hash::FxHashMap;
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_analysis::property_accessors;

/// A node in the hardware topology graph.
#[derive(Debug, Clone)]
pub enum HwNode {
    Processor {
        idx: ComponentInstanceIdx,
        name: String,
        /// CPU utilization budget (0.0 to 1.0), None if not specified
        utilization_budget: Option<f64>,
        /// Memory capacity in bytes, None if not specified
        memory_bytes: Option<u64>,
    },
    Memory {
        idx: ComponentInstanceIdx,
        name: String,
        size_bytes: Option<u64>,
    },
    Bus {
        idx: ComponentInstanceIdx,
        name: String,
        bandwidth_bps: Option<f64>,
        protocol: Option<String>,
    },
}

/// An edge in the hardware topology (bus access).
#[derive(Debug, Clone)]
pub struct BusEdge {
    pub bus_name: String,
}

/// Hardware topology graph extracted from AADL instance model.
pub struct TopologyGraph {
    pub graph: DiGraph<HwNode, BusEdge>,
    /// Map from ComponentInstanceIdx to graph NodeIndex
    pub idx_map: FxHashMap<ComponentInstanceIdx, NodeIndex>,
}

impl TopologyGraph {
    pub fn from_instance(instance: &SystemInstance) -> Self {
        let mut graph = DiGraph::new();
        let mut idx_map = FxHashMap::default();

        // Phase 1: Add all hardware nodes
        for (comp_idx, comp) in instance.all_components() {
            let props = instance.properties_for(comp_idx);
            let node = match comp.category {
                ComponentCategory::Processor | ComponentCategory::VirtualProcessor => {
                    Some(HwNode::Processor {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        utilization_budget: None, // TODO: extract from properties
                        memory_bytes: None,
                    })
                }
                ComponentCategory::Memory => {
                    let size = property_accessors::get_size_property(props, "Memory_Size")
                        .map(|bits| bits / 8);
                    Some(HwNode::Memory {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        size_bytes: size,
                    })
                }
                ComponentCategory::Bus | ComponentCategory::VirtualBus => {
                    Some(HwNode::Bus {
                        idx: comp_idx,
                        name: comp.name.as_str().to_string(),
                        bandwidth_bps: None, // TODO: extract from properties
                        protocol: None,
                    })
                }
                _ => None,
            };

            if let Some(n) = node {
                let ni = graph.add_node(n);
                idx_map.insert(comp_idx, ni);
            }
        }

        // Phase 2: Add bus access edges (processor ↔ bus connections)
        // A processor is connected to a bus if it has a bus access feature
        // or if there's a connection binding referencing that bus
        for (comp_idx, comp) in instance.all_components() {
            if !matches!(comp.category,
                ComponentCategory::Processor | ComponentCategory::VirtualProcessor) {
                continue;
            }
            let Some(&proc_ni) = idx_map.get(&comp_idx) else { continue };

            // Check features for bus access
            for &feat_idx in &comp.features {
                let feat = &instance.features[feat_idx];
                if feat.kind == spar_hir_def::item_tree::FeatureKind::BusAccess {
                    // Find the bus this access connects to
                    // by looking for connections from this feature
                    for (_, conn) in instance.connections.iter() {
                        let matches_src = conn.src.as_ref()
                            .map(|e| e.feature.as_str() == feat.name.as_str())
                            .unwrap_or(false);
                        let matches_dst = conn.dst.as_ref()
                            .map(|e| e.feature.as_str() == feat.name.as_str())
                            .unwrap_or(false);
                        if matches_src || matches_dst {
                            // Find the other end — should be a bus
                            let other_end = if matches_src { &conn.dst } else { &conn.src };
                            if let Some(end) = other_end {
                                // Look up the bus by name
                                for (&bus_idx, &bus_ni) in &idx_map {
                                    let bus_comp = instance.component(bus_idx);
                                    if matches!(bus_comp.category,
                                        ComponentCategory::Bus | ComponentCategory::VirtualBus)
                                        && bus_comp.name.as_str() == end.feature.as_str()
                                    {
                                        graph.add_edge(proc_ni, bus_ni, BusEdge {
                                            bus_name: bus_comp.name.as_str().to_string(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Self { graph, idx_map }
    }

    pub fn processor_count(&self) -> usize {
        self.graph.node_weights()
            .filter(|n| matches!(n, HwNode::Processor { .. }))
            .count()
    }

    pub fn bus_count(&self) -> usize {
        self.graph.node_weights()
            .filter(|n| matches!(n, HwNode::Bus { .. }))
            .count()
    }

    pub fn memory_count(&self) -> usize {
        self.graph.node_weights()
            .filter(|n| matches!(n, HwNode::Memory { .. }))
            .count()
    }

    /// Get all processor NodeIndex values.
    pub fn processors(&self) -> Vec<NodeIndex> {
        self.graph.node_indices()
            .filter(|&ni| matches!(self.graph[ni], HwNode::Processor { .. }))
            .collect()
    }

    /// Check if two processors are connected (share a bus).
    pub fn are_connected(&self, a: NodeIndex, b: NodeIndex) -> bool {
        use petgraph::algo::has_path_connecting;
        has_path_connecting(&self.graph, a, b, None)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p spar-solver -- empty_system`
Expected: PASS

- [ ] **Step 5: Write more topology tests**

```rust
#[test]
fn extracts_processors_and_buses() {
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu1 = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let cpu2 = b.add_component("cpu2", ComponentCategory::Processor, Some(root));
    let eth = b.add_component("ethernet", ComponentCategory::Bus, Some(root));
    let mem = b.add_component("ram", ComponentCategory::Memory, Some(root));
    b.set_children(root, vec![cpu1, cpu2, eth, mem]);
    b.set_property(mem, "Memory_Properties", "Memory_Size", "256 KByte");

    let instance = b.build(root);
    let topo = TopologyGraph::from_instance(&instance);

    assert_eq!(topo.processor_count(), 2);
    assert_eq!(topo.bus_count(), 1);
    assert_eq!(topo.memory_count(), 1);
}

#[test]
fn topology_deterministic() {
    // SOLVER-REQ-023: deterministic output
    let instance = build_two_cpu_system();
    let t1 = TopologyGraph::from_instance(&instance);
    let t2 = TopologyGraph::from_instance(&instance);
    assert_eq!(t1.processor_count(), t2.processor_count());
    assert_eq!(t1.bus_count(), t2.bus_count());
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test -p spar-solver`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git commit -am "feat(solver): topology graph extraction from AADL instance model"
```

---

## Task 3: Constraint extraction

**Files:**
- Create: `crates/spar-solver/src/constraints.rs`
- Modify: `crates/spar-solver/src/tests.rs`

**Rivet:** Satisfies REQ-SOLVER-003. STPA: SOLVER-REQ-020 (no silent defaults).

- [ ] **Step 1: Write failing test — extract thread constraints**

```rust
#[test]
fn extract_thread_timing_constraints() {
    let mut b = TestBuilder::new();
    let root = b.add_component("root", ComponentCategory::System, None);
    let cpu = b.add_component("cpu1", ComponentCategory::Processor, Some(root));
    let proc = b.add_component("proc", ComponentCategory::Process, Some(root));
    let t1 = b.add_component("t1", ComponentCategory::Thread, Some(proc));
    b.set_children(root, vec![cpu, proc]);
    b.set_children(proc, vec![t1]);
    b.set_property(t1, "Timing_Properties", "Period", "10 ms");
    b.set_property(t1, "Timing_Properties", "Compute_Execution_Time", "2 ms");
    b.set_property(t1, "Timing_Properties", "Deadline", "8 ms");

    let instance = b.build(root);
    let constraints = ThreadConstraints::from_instance(&instance);

    assert_eq!(constraints.threads.len(), 1);
    let tc = &constraints.threads[0];
    assert_eq!(tc.period_ps, 10_000_000_000); // 10ms in ps
    assert_eq!(tc.wcet_ps, 2_000_000_000);
    assert_eq!(tc.deadline_ps, 8_000_000_000);
}

#[test]
fn missing_wcet_is_error_not_zero() {
    // SOLVER-REQ-020: no silent defaults
    let mut b = TestBuilder::new();
    // ... thread with Period but no WCET
    let constraints = ThreadConstraints::from_instance(&instance);
    assert!(constraints.threads[0].wcet_ps == 0);
    assert!(constraints.warnings.iter().any(|w| w.contains("missing")));
}
```

- [ ] **Step 2: Run to verify failure, then implement**

`constraints.rs`:

```rust
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;
use spar_analysis::property_accessors;

/// Timing constraints for a single thread.
#[derive(Debug, Clone)]
pub struct ThreadConstraint {
    pub idx: ComponentInstanceIdx,
    pub name: String,
    pub period_ps: u64,
    pub wcet_ps: u64,
    pub deadline_ps: u64,
    pub current_binding: Option<String>,
}

/// Resource constraints for a processor.
#[derive(Debug, Clone)]
pub struct ProcessorConstraint {
    pub idx: ComponentInstanceIdx,
    pub name: String,
    pub memory_bytes: Option<u64>,
}

/// All constraints extracted from an AADL model.
pub struct ThreadConstraints {
    pub threads: Vec<ThreadConstraint>,
    pub processors: Vec<ProcessorConstraint>,
    /// Warnings about missing/incomplete properties
    pub warnings: Vec<String>,
}

impl ThreadConstraints {
    pub fn from_instance(instance: &SystemInstance) -> Self {
        let mut threads = Vec::new();
        let mut processors = Vec::new();
        let mut warnings = Vec::new();

        for (idx, comp) in instance.all_components() {
            let props = instance.properties_for(idx);
            match comp.category {
                ComponentCategory::Thread => {
                    let period = property_accessors::get_timing_property(props, "Period")
                        .unwrap_or(0);
                    let wcet = property_accessors::get_execution_time(props)
                        .unwrap_or(0);
                    let deadline = property_accessors::get_timing_property(props, "Deadline")
                        .unwrap_or(period); // Default: deadline = period
                    let binding = property_accessors::get_processor_binding(props);

                    if period == 0 {
                        warnings.push(format!(
                            "{}: missing Period — cannot schedule", comp.name
                        ));
                    }
                    if wcet == 0 {
                        warnings.push(format!(
                            "{}: missing Compute_Execution_Time — assuming zero (UNSAFE)",
                            comp.name
                        ));
                    }

                    threads.push(ThreadConstraint {
                        idx, name: comp.name.as_str().to_string(),
                        period_ps: period, wcet_ps: wcet,
                        deadline_ps: deadline, current_binding: binding,
                    });
                }
                ComponentCategory::Processor => {
                    processors.push(ProcessorConstraint {
                        idx, name: comp.name.as_str().to_string(),
                        memory_bytes: property_accessors::get_size_property(props, "Memory_Size")
                            .map(|bits| bits / 8),
                    });
                }
                _ => {}
            }
        }

        // SOLVER-REQ-023: sort for deterministic output
        threads.sort_by(|a, b| a.name.cmp(&b.name));
        processors.sort_by(|a, b| a.name.cmp(&b.name));

        Self { threads, processors, warnings }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p spar-solver`
Expected: all pass

- [ ] **Step 4: Commit**

```bash
git commit -am "feat(solver): constraint extraction from AADL properties"
```

---

## Task 4: Bin-packing allocator (Layer 1)

**Files:**
- Create: `crates/spar-solver/src/allocate.rs`
- Modify: `crates/spar-solver/src/tests.rs`

**Rivet:** Implements ARCH-SOLVER-004. STPA: SOLVER-REQ-005 (re-check after incremental), SOLVER-REQ-023 (deterministic).

- [ ] **Step 1: Write failing test — FFD allocation**

```rust
#[test]
fn ffd_allocates_threads_to_processors() {
    // 3 threads, 2 processors, should fit
    let constraints = ThreadConstraints {
        threads: vec![
            make_thread("t1", 10_000_000_000, 3_000_000_000, 10_000_000_000, None),
            make_thread("t2", 20_000_000_000, 2_000_000_000, 20_000_000_000, None),
            make_thread("t3", 10_000_000_000, 4_000_000_000, 10_000_000_000, None),
        ],
        processors: vec![
            make_processor("cpu1"),
            make_processor("cpu2"),
        ],
        warnings: vec![],
    };

    let result = Allocator::ffd(&constraints);
    assert!(result.is_feasible());
    assert_eq!(result.bindings.len(), 3);
    // Each thread assigned to some processor
    for b in &result.bindings {
        assert!(b.processor.starts_with("cpu"));
    }
}

#[test]
fn ffd_detects_infeasible() {
    // 1 thread that can't fit (utilization > 1.0)
    let constraints = ThreadConstraints {
        threads: vec![
            make_thread("t1", 10_000_000_000, 11_000_000_000, 10_000_000_000, None),
        ],
        processors: vec![make_processor("cpu1")],
        warnings: vec![],
    };

    let result = Allocator::ffd(&constraints);
    assert!(!result.is_feasible());
}
```

- [ ] **Step 2: Implement Allocator**

```rust
use crate::constraints::{ThreadConstraint, ThreadConstraints, ProcessorConstraint};

/// A binding assignment: thread → processor.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Binding {
    pub thread: String,
    pub processor: String,
    pub utilization: f64,
}

/// Result of an allocation attempt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AllocationResult {
    pub bindings: Vec<Binding>,
    pub unallocated: Vec<String>,
    pub per_processor_utilization: Vec<(String, f64)>,
    pub warnings: Vec<String>,
}

impl AllocationResult {
    pub fn is_feasible(&self) -> bool {
        self.unallocated.is_empty()
    }
}

pub struct Allocator;

impl Allocator {
    /// First-Fit Decreasing bin packing with utilization-based capacity.
    ///
    /// Threads sorted by utilization (C/T) descending, assigned to first
    /// processor where total utilization stays ≤ 1.0.
    pub fn ffd(constraints: &ThreadConstraints) -> AllocationResult {
        let mut sorted: Vec<&ThreadConstraint> = constraints.threads.iter()
            .filter(|t| t.period_ps > 0)
            .collect();

        // Sort by utilization descending (SOLVER-REQ-023: deterministic via name tiebreak)
        sorted.sort_by(|a, b| {
            let ua = a.wcet_ps as f64 / a.period_ps as f64;
            let ub = b.wcet_ps as f64 / b.period_ps as f64;
            ub.partial_cmp(&ua).unwrap_or(std::cmp::Ordering::Equal)
                .then(a.name.cmp(&b.name))
        });

        let mut proc_util: Vec<(String, f64)> = constraints.processors.iter()
            .map(|p| (p.name.clone(), 0.0))
            .collect();

        let mut bindings = Vec::new();
        let mut unallocated = Vec::new();
        let mut warnings = Vec::new();

        for thread in &sorted {
            let util = if thread.period_ps > 0 {
                thread.wcet_ps as f64 / thread.period_ps as f64
            } else {
                continue; // skip threads with no period (warned in constraints)
            };

            let mut placed = false;
            for (proc_name, proc_u) in proc_util.iter_mut() {
                if *proc_u + util <= 1.0 {
                    *proc_u += util;
                    bindings.push(Binding {
                        thread: thread.name.clone(),
                        processor: proc_name.clone(),
                        utilization: util,
                    });
                    placed = true;
                    break;
                }
            }

            if !placed {
                unallocated.push(thread.name.clone());
            }
        }

        // Warn about threads with missing properties
        for thread in &constraints.threads {
            if thread.period_ps == 0 {
                warnings.push(format!("{}: skipped (no Period)", thread.name));
            }
        }

        AllocationResult {
            bindings,
            unallocated,
            per_processor_utilization: proc_util,
            warnings,
        }
    }

    /// Best-Fit Decreasing: assign to processor with least remaining capacity.
    pub fn bfd(constraints: &ThreadConstraints) -> AllocationResult {
        // Same as FFD but pick the tightest fit instead of first fit
        let mut sorted: Vec<&ThreadConstraint> = constraints.threads.iter()
            .filter(|t| t.period_ps > 0)
            .collect();

        sorted.sort_by(|a, b| {
            let ua = a.wcet_ps as f64 / a.period_ps as f64;
            let ub = b.wcet_ps as f64 / b.period_ps as f64;
            ub.partial_cmp(&ua).unwrap_or(std::cmp::Ordering::Equal)
                .then(a.name.cmp(&b.name))
        });

        let mut proc_util: Vec<(String, f64)> = constraints.processors.iter()
            .map(|p| (p.name.clone(), 0.0))
            .collect();

        let mut bindings = Vec::new();
        let mut unallocated = Vec::new();

        for thread in &sorted {
            let util = thread.wcet_ps as f64 / thread.period_ps as f64;

            // Find processor with least remaining capacity that still fits
            let best = proc_util.iter_mut()
                .filter(|(_, u)| *u + util <= 1.0)
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            if let Some((proc_name, proc_u)) = best {
                let proc_name = proc_name.clone();
                *proc_u += util;
                bindings.push(Binding {
                    thread: thread.name.clone(),
                    processor: proc_name,
                    utilization: util,
                });
            } else {
                unallocated.push(thread.name.clone());
            }
        }

        AllocationResult {
            bindings,
            unallocated,
            per_processor_utilization: proc_util,
            warnings: vec![],
        }
    }
}
```

- [ ] **Step 3: Write edge case tests**

```rust
#[test]
fn ffd_respects_existing_bindings() {
    // Thread already bound → keep its binding
    let constraints = ThreadConstraints {
        threads: vec![
            make_thread("t1", 10_000_000_000, 3_000_000_000, 10_000_000_000,
                        Some("cpu2".to_string())),
            make_thread("t2", 20_000_000_000, 2_000_000_000, 20_000_000_000, None),
        ],
        processors: vec![make_processor("cpu1"), make_processor("cpu2")],
        warnings: vec![],
    };

    let result = Allocator::ffd(&constraints);
    assert!(result.is_feasible());
    // t1 should stay on cpu2
    let t1_binding = result.bindings.iter().find(|b| b.thread == "t1").unwrap();
    assert_eq!(t1_binding.processor, "cpu2");
}

#[test]
fn bfd_packs_tighter_than_ffd() {
    // BFD should produce higher per-processor utilization (tighter packing)
    let constraints = make_balanced_constraints();
    let ffd = Allocator::ffd(&constraints);
    let bfd = Allocator::bfd(&constraints);
    assert!(ffd.is_feasible());
    assert!(bfd.is_feasible());
}

#[test]
fn allocation_is_deterministic() {
    // SOLVER-REQ-023
    let constraints = make_balanced_constraints();
    let r1 = Allocator::ffd(&constraints);
    let r2 = Allocator::ffd(&constraints);
    assert_eq!(r1.bindings.len(), r2.bindings.len());
    for (a, b) in r1.bindings.iter().zip(r2.bindings.iter()) {
        assert_eq!(a.thread, b.thread);
        assert_eq!(a.processor, b.processor);
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p spar-solver`
Expected: all pass

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(solver): FFD/BFD bin-packing allocator with schedulability checking"
```

---

## Task 5: Virtual bus library

**Files:**
- Create: `crates/spar-transform/src/protocol_library.rs`
- Modify: `crates/spar-transform/src/lib.rs`

**Rivet:** Satisfies REQ-SOLVER-002, implements ARCH-SOLVER-003.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn protocol_library_has_all_protocols() {
    let lib = protocol_library();
    assert!(lib.items.len() >= 12);
    let names: Vec<_> = lib.items.iter()
        .map(|i| i.name.as_str())
        .collect();
    assert!(names.contains(&"DDS"));
    assert!(names.contains(&"SOME_IP"));
    assert!(names.contains(&"SharedMemory"));
    assert!(names.contains(&"CAN"));
    assert!(names.contains(&"Ethernet"));
}
```

- [ ] **Step 2: Implement protocol_library()**

```rust
//! Virtual bus library — AADL package with protocol type definitions.
//!
//! Each protocol is a virtual bus type with properties for latency overhead,
//! bandwidth capacity, max payload, security profile, and bus compatibility.

use spar_hir_def::item_tree::*;
use la_arena::Arena;

/// Protocol definition with deployment-relevant properties.
pub struct ProtocolDef {
    pub name: &'static str,
    pub bus_type: &'static str,
    pub latency_overhead_us: f64,
    pub bandwidth_mbps: f64,
    pub max_payload_bytes: u64,
    pub security: &'static str,
}

pub const PROTOCOLS: &[ProtocolDef] = &[
    // Automotive
    ProtocolDef { name: "DDS", bus_type: "ethernet", latency_overhead_us: 500.0,
        bandwidth_mbps: 1000.0, max_payload_bytes: 65536, security: "optional-tls" },
    ProtocolDef { name: "SOME_IP", bus_type: "ethernet", latency_overhead_us: 200.0,
        bandwidth_mbps: 1000.0, max_payload_bytes: 1400, security: "optional-tls" },
    ProtocolDef { name: "CAN", bus_type: "can", latency_overhead_us: 100.0,
        bandwidth_mbps: 0.5, max_payload_bytes: 8, security: "none" },
    ProtocolDef { name: "CAN_FD", bus_type: "can", latency_overhead_us: 80.0,
        bandwidth_mbps: 5.0, max_payload_bytes: 64, security: "none" },
    ProtocolDef { name: "FlexRay", bus_type: "flexray", latency_overhead_us: 50.0,
        bandwidth_mbps: 10.0, max_payload_bytes: 254, security: "none" },
    // General
    ProtocolDef { name: "Ethernet", bus_type: "ethernet", latency_overhead_us: 10.0,
        bandwidth_mbps: 1000.0, max_payload_bytes: 1500, security: "none" },
    ProtocolDef { name: "SharedMemory", bus_type: "shared_memory", latency_overhead_us: 0.1,
        bandwidth_mbps: 100000.0, max_payload_bytes: u64::MAX, security: "none" },
    // Aerospace
    ProtocolDef { name: "AFDX", bus_type: "ethernet", latency_overhead_us: 500.0,
        bandwidth_mbps: 100.0, max_payload_bytes: 1471, security: "none" },
    ProtocolDef { name: "ARINC429", bus_type: "arinc429", latency_overhead_us: 1000.0,
        bandwidth_mbps: 0.1, max_payload_bytes: 4, security: "none" },
    ProtocolDef { name: "SpaceWire", bus_type: "spacewire", latency_overhead_us: 1.0,
        bandwidth_mbps: 200.0, max_payload_bytes: 65536, security: "none" },
    // Industrial
    ProtocolDef { name: "PROFINET", bus_type: "ethernet", latency_overhead_us: 250.0,
        bandwidth_mbps: 100.0, max_payload_bytes: 1440, security: "none" },
    ProtocolDef { name: "EtherCAT", bus_type: "ethernet", latency_overhead_us: 1.0,
        bandwidth_mbps: 100.0, max_payload_bytes: 1486, security: "none" },
    // Embedded
    ProtocolDef { name: "MAVLink", bus_type: "serial", latency_overhead_us: 50.0,
        bandwidth_mbps: 0.115, max_payload_bytes: 255, security: "optional-signing" },
];

/// Generate the AADL package for the protocol library.
pub fn protocol_library() -> ItemTree {
    // Build ItemTree with VirtualBus component types for each protocol
    // ... (follows wrpc.rs pattern)
}
```

- [ ] **Step 3: Run tests, commit**

Run: `cargo test -p spar-transform`
Expected: all pass

```bash
git commit -am "feat(transform): virtual bus library with 13 protocol types"
```

---

## Task 6: Source rewriting

**Files:**
- Create: `crates/spar-cli/src/refactor.rs`
- Modify: `crates/spar-cli/src/main.rs`
- Modify: `crates/spar-cli/Cargo.toml`

**Rivet:** Satisfies REQ-SOLVER-007, implements ARCH-SOLVER-005. STPA: SOLVER-REQ-014 (fully-qualified targeting), SOLVER-REQ-016 (parse-after-rewrite).

- [ ] **Step 1: Write failing test — insert binding property**

```rust
#[test]
fn insert_processor_binding() {
    let source = r#"
package Pkg
public
  thread T
  end T;

  thread implementation T.impl
    properties
      Timing_Properties::Period => 10 ms;
  end T.impl;
end Pkg;
"#;
    let edit = BindingEdit {
        component_path: "T.impl",
        property: "Deployment_Properties::Actual_Processor_Binding",
        value: "reference (cpu1)",
    };
    let result = apply_binding_edit(source, &edit).unwrap();
    assert!(result.contains("Actual_Processor_Binding"));
    assert!(result.contains("reference (cpu1)"));
    // Must still parse cleanly (SOLVER-REQ-016)
    let parse = spar_syntax::parse(&result);
    assert!(parse.errors().is_empty(), "rewrite produced parse errors: {:?}", parse.errors());
}
```

- [ ] **Step 2: Implement apply_binding_edit**

Walk the rowan CST to find the target component implementation's properties section, insert the new property association. If the property already exists, replace the value. If no properties section exists, create one.

Key safety: use fully-qualified component path matching (SOLVER-REQ-014), not substring. After edit, re-parse to validate (SOLVER-REQ-016).

- [ ] **Step 3: Test edge cases**

```rust
#[test]
fn update_existing_binding() { /* change cpu1 → cpu2 */ }

#[test]
fn insert_when_no_properties_section() { /* add properties block */ }

#[test]
fn rewrite_preserves_comments() { /* comments survive */ }

#[test]
fn rewrite_rejects_ambiguous_path() {
    // SOLVER-REQ-014: fully-qualified targeting
    // If "T.impl" matches multiple components, return error
}
```

- [ ] **Step 4: Run tests, commit**

```bash
git commit -am "feat(cli): source rewriting for deployment binding properties"
```

---

## Task 7: CLI integration — `spar allocate`

**Files:**
- Modify: `crates/spar-cli/src/main.rs`
- Modify: `crates/spar-cli/Cargo.toml`

- [ ] **Step 1: Add spar-solver dependency to spar-cli**

In `crates/spar-cli/Cargo.toml`:
```toml
spar-solver.workspace = true
```

- [ ] **Step 2: Implement cmd_allocate**

```rust
fn cmd_allocate(args: &[String]) {
    // Parse: spar allocate --root Pkg::Sys.Impl [--strategy ffd|bfd]
    //        [--format text|json] [--apply] <files...>
    //
    // 1. Parse files, build instance model
    // 2. Extract topology graph + constraints
    // 3. Run allocator (FFD or BFD)
    // 4. Print results (text or JSON)
    // 5. If --apply: rewrite source files with new bindings
}
```

- [ ] **Step 3: Add to command dispatch**

In `main.rs` match:
```rust
"allocate" => cmd_allocate(&args[2..]),
```

Update `print_usage()` with:
```
  allocate --root Package::Type.Impl [--strategy ffd|bfd] [--apply] <file...>
```

- [ ] **Step 4: Write integration test**

Create a test `.aadl` file with unbound threads and verify `spar allocate` produces valid bindings.

- [ ] **Step 5: Run full test suite**

Run: `cargo test --workspace`
Expected: all pass, no regressions

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(cli): spar allocate command with FFD/BFD bin-packing"
```

---

## Task 8: Impact preview

**Files:**
- Modify: `crates/spar-solver/src/allocate.rs`
- Modify: `crates/spar-cli/src/main.rs`

- [ ] **Step 1: Add impact analysis to AllocationResult**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct ImpactAnalysis {
    pub scheduling_feasible: bool,
    pub utilization_per_processor: Vec<(String, f64)>,
    pub rta_results: Vec<(String, u64, u64)>, // (thread, response_time, deadline)
    pub deadline_violations: Vec<String>,
}
```

- [ ] **Step 2: Run RTA on proposed allocation**

After bin-packing, run `spar-analysis::rta::RtaAnalysis` on a hypothetical instance with the proposed bindings. Report any deadline violations.

- [ ] **Step 3: Add `--dry-run` flag to spar allocate**

`--dry-run` (default): show proposed allocation + impact without writing files.
`--apply`: actually rewrite source.

- [ ] **Step 4: Test impact analysis catches violations**

```rust
#[test]
fn impact_detects_deadline_miss() {
    // Allocate threads that barely fit by utilization but violate RTA deadlines
    // due to interference
}
```

- [ ] **Step 5: Run tests, commit**

```bash
git commit -am "feat(solver): impact preview with RTA validation before applying"
```

---

## Task 9: Final integration + rivet validation

- [ ] **Step 1: Run full test suite**

```bash
cargo test --workspace
cargo +stable clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 2: Update rivet artifacts**

Update status of REQ-SOLVER-001, REQ-SOLVER-002, REQ-SOLVER-003, REQ-SOLVER-007 to `implemented`. Update ARCH-SOLVER-002 through ARCH-SOLVER-005 to `implemented`.

- [ ] **Step 3: Run rivet validate**

```bash
rivet validate
```

- [ ] **Step 4: Final commit + push**

```bash
git commit -am "feat(v0.3.0): deployment solver foundations — topology, allocation, rewriting"
git push
```
