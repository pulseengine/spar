# Spar + Rivet Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make AADL models first-class rivet lifecycle artifacts via serde serialization, JSON CLI output, an AADL schema, and a rivet adapter.

**Architecture:** Add `Serialize`/`Deserialize` to all spar-hir public types and spar-hir-def enums. Create a serializable `InstanceTree` projection that flattens the arena-based `SystemInstance` into a JSON-friendly tree. Add `--format json` to the spar CLI. On the rivet side, create an `aadl.yaml` schema and an `AadlAdapter` that shells out to `spar analyze --format json` and converts the output into rivet `Artifact`s.

**Tech Stack:** Rust, serde/serde_json, spar-hir, spar-hir-def, rivet-core

---

## Repo Map

- **spar** = `/Volumes/Home/git/pulseengine/spar`
- **rivet** = `/Volumes/Home/git/sdlc`

---

### Task 1: Add serde derives to spar-hir-def enums

**Files:**
- Modify: `spar/crates/spar-hir-def/Cargo.toml`
- Modify: `spar/crates/spar-hir-def/src/item_tree/mod.rs`

**Step 1: Add serde dependency to spar-hir-def**

In `spar/crates/spar-hir-def/Cargo.toml`, add:

```toml
serde = { version = "1", features = ["derive"] }
```

to the `[dependencies]` section.

Also add `serde` to workspace deps in `spar/Cargo.toml`:

```toml
serde = { version = "1", features = ["derive"] }
```

Then change the spar-hir-def dep to `serde.workspace = true`.

**Step 2: Derive Serialize on public enums in item_tree/mod.rs**

Add `use serde::Serialize;` at the top of `spar/crates/spar-hir-def/src/item_tree/mod.rs`.

Add `Serialize` to the derive list of these types:

- `ComponentCategory` (line 71): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `Direction` (line 200): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `FeatureKind` (line 218): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `AccessKind` (line 251): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `ConnectionKind` (line 297): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `FlowKind` (line 332): `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]`
- `PropertyExpr` (line 501): `#[derive(Debug, Clone, PartialEq, Eq, Serialize)]`
- `PropertyTypeDef` (line 536): `#[derive(Debug, Clone, PartialEq, Eq, Serialize)]`

Note: `PropertyExpr` contains `Name` (from `crate::name`) and `ClassifierRef`. These need `Serialize` too. Check `crate::name` — `Name` wraps `SmolStr`. We need to add `Serialize` to `Name` and `ClassifierRef` in `spar/crates/spar-hir-def/src/name.rs`. If `Name` is a newtype over `SmolStr`, implement `Serialize` manually or add the derive. `SmolStr` has `serde` feature — add `smol_str = { version = "0.3", features = ["serde"] }` to spar-hir-def Cargo.toml. Then derive `Serialize` on `Name`. Similarly for `ClassifierRef`.

**Step 3: Build and verify**

Run: `cargo build -p spar-hir-def`
Expected: compiles cleanly.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat(spar-hir-def): add serde Serialize to public enums and types

Adds Serialize derives to ComponentCategory, Direction, FeatureKind,
AccessKind, ConnectionKind, FlowKind, PropertyExpr, PropertyTypeDef,
Name, and ClassifierRef. Foundation for JSON serialization."
```

---

### Task 2: Add serde derives to spar-hir public types

**Files:**
- Modify: `spar/crates/spar-hir/Cargo.toml`
- Modify: `spar/crates/spar-hir/src/lib.rs`
- Test: `spar/crates/spar-hir/src/lib.rs` (inline tests)

**Step 1: Add serde + serde_json dependencies**

In `spar/crates/spar-hir/Cargo.toml`:

```toml
[dependencies]
# ... existing deps ...
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

Use workspace deps if already defined, otherwise add to workspace.

**Step 2: Write failing test**

Add to the `tests` module in `spar/crates/spar-hir/src/lib.rs`:

```rust
#[test]
fn serde_round_trip_packages() {
    let db = make_db(
        r#"
        package Nav
        public
          system GPS
            features
              pos_out: out data port;
          end GPS;
        end Nav;
        "#,
    );
    let pkgs = db.packages();
    let json = serde_json::to_string_pretty(&pkgs).expect("serialize");
    assert!(json.contains("GPS"));
    assert!(json.contains("pos_out"));
    let round: Vec<Package> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(round.len(), 1);
    assert_eq!(round[0].name, "Nav");
    assert_eq!(round[0].component_types[0].name, "GPS");
}
```

Run: `cargo test -p spar-hir serde_round_trip_packages`
Expected: FAIL — `Serialize` not derived on `Package`.

**Step 3: Add Serialize + Deserialize derives to all spar-hir types**

Add `use serde::{Serialize, Deserialize};` at the top of `lib.rs`.

Add `Serialize, Deserialize` to every public struct/enum derive:

- `Package` (line 177): `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`
- `ComponentType` (line 188): same
- `ComponentImpl` (line 203): same
- `FeatureGroupType` (line 225): same
- `Feature` (line 237): same
- `Subcomponent` (line 251): same
- `Connection` (line 263): same
- `FlowSpec` (line 281): same
- `EndToEndFlow` (line 295): same
- `Mode` (line 307): same
- `ModeTransition` (line 316): same
- `PropertyAssociation` (line 327): same
- `Classifier` (line 346): same

Also need `Deserialize` on the re-exported hir-def enums. Go back to `item_tree/mod.rs` and add `Deserialize` alongside `Serialize` on: `ComponentCategory`, `Direction`, `FeatureKind`, `AccessKind`, `ConnectionKind`, `FlowKind`, `PropertyExpr`.

Add `serde = { version = "1", features = ["derive"] }` to `spar-hir-def` deps if not already, and `Deserialize` to `Name` and `ClassifierRef`.

**Step 4: Run test**

Run: `cargo test -p spar-hir serde_round_trip_packages`
Expected: PASS

**Step 5: Add more serde tests**

```rust
#[test]
fn serde_round_trip_classifier() {
    let db = make_db(
        r#"
        package Sys
        public
          system Top end Top;
          system implementation Top.Impl
            subcomponents
              cpu: processor;
          end Top.Impl;
        end Sys;
        "#,
    );
    let cls = db.find_classifier("Sys::Top.Impl").unwrap();
    let json = serde_json::to_string(&cls).unwrap();
    assert!(json.contains("Top.Impl"));
    let round: Classifier = serde_json::from_str(&json).unwrap();
    assert_eq!(round, cls);
}

#[test]
fn serde_property_expressions() {
    let db = make_db(
        r#"
        package Props
        public
          thread Worker
            properties
              Dispatch_Protocol => Periodic;
              Period => 10 ms;
          end Worker;
        end Props;
        "#,
    );
    let pkgs = db.packages();
    let json = serde_json::to_string_pretty(&pkgs).unwrap();
    assert!(json.contains("Dispatch_Protocol"));
    assert!(json.contains("10"));
    let round: Vec<Package> = serde_json::from_str(&json).unwrap();
    assert_eq!(round[0].component_types[0].properties.len(),
               pkgs[0].component_types[0].properties.len());
}
```

Run: `cargo test -p spar-hir`
Expected: all tests PASS.

**Step 6: Commit**

```bash
git add -A
git commit -m "feat(spar-hir): add serde Serialize/Deserialize to all public types

Enables JSON serialization of Package, ComponentType, ComponentImpl,
Feature, Subcomponent, Connection, FlowSpec, EndToEndFlow, Mode,
ModeTransition, PropertyAssociation, and Classifier. Includes round-trip
tests."
```

---

### Task 3: Create serializable instance tree projection

**Files:**
- Modify: `spar/crates/spar-hir/src/lib.rs`

The `Instance` wraps `SystemInstance` which uses arena indices. We need a serializable tree projection.

**Step 1: Write failing test**

Add to tests in `spar/crates/spar-hir/src/lib.rs`:

```rust
#[test]
fn serde_instance_tree() {
    let db = make_db(
        r#"
        package IMA
        public
          system Platform
            features
              eth: in out data port;
          end Platform;

          processor CPU end CPU;

          system implementation Platform.Dual
            subcomponents
              cpu1: processor CPU;
              cpu2: processor CPU;
          end Platform.Dual;
        end IMA;
        "#,
    );
    let inst = db.instantiate("IMA::Platform.Dual").unwrap();
    let tree = inst.to_serializable();
    let json = serde_json::to_string_pretty(&tree).expect("serialize instance");
    assert!(json.contains("Platform"));
    assert!(json.contains("cpu1"));
    assert!(json.contains("cpu2"));

    // Should be a tree: root with 2 children
    assert_eq!(tree.children.len(), 2);
    assert_eq!(tree.name, "Platform");
}
```

Run: `cargo test -p spar-hir serde_instance_tree`
Expected: FAIL — `to_serializable` method doesn't exist.

**Step 2: Define InstanceNode and implement to_serializable**

Add above the `Instance` impl block in `lib.rs`:

```rust
/// A serializable tree representation of an AADL instance model.
///
/// Flattens the arena-based `SystemInstance` into a JSON-friendly tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceNode {
    pub name: String,
    pub category: ComponentCategory,
    pub package: String,
    pub type_name: String,
    pub impl_name: Option<String>,
    pub features: Vec<InstanceFeature>,
    pub connections: Vec<InstanceConnection>,
    pub children: Vec<InstanceNode>,
    pub diagnostics: Vec<String>,
}

/// A feature in the serializable instance tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceFeature {
    pub name: String,
    pub kind: FeatureKind,
    pub direction: Option<Direction>,
}

/// A connection in the serializable instance tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceConnection {
    pub name: String,
    pub kind: ConnectionKind,
    pub is_bidirectional: bool,
    pub source: Option<String>,
    pub destination: Option<String>,
}
```

Add method to `Instance`:

```rust
impl Instance {
    // ... existing methods ...

    /// Convert the instance model to a serializable tree.
    pub fn to_serializable(&self) -> InstanceNode {
        self.build_node(self.inner.root)
    }

    fn build_node(&self, idx: spar_hir_def::instance::ComponentInstanceIdx) -> InstanceNode {
        let comp = self.inner.component(idx);

        let features = comp.features.iter().map(|&fi| {
            let f = &self.inner.features[fi];
            InstanceFeature {
                name: f.name.as_str().to_string(),
                kind: f.kind,
                direction: f.direction,
            }
        }).collect();

        let connections = comp.connections.iter().map(|&ci| {
            let c = &self.inner.connections[ci];
            InstanceConnection {
                name: c.name.as_str().to_string(),
                kind: c.kind,
                is_bidirectional: c.is_bidirectional,
                source: c.src.as_ref().map(|e| {
                    match &e.subcomponent {
                        Some(sub) => format!("{}.{}", sub, e.feature),
                        None => e.feature.as_str().to_string(),
                    }
                }),
                destination: c.dst.as_ref().map(|e| {
                    match &e.subcomponent {
                        Some(sub) => format!("{}.{}", sub, e.feature),
                        None => e.feature.as_str().to_string(),
                    }
                }),
            }
        }).collect();

        let children = comp.children.iter()
            .map(|&child_idx| self.build_node(child_idx))
            .collect();

        InstanceNode {
            name: comp.name.as_str().to_string(),
            category: comp.category,
            package: comp.package.as_str().to_string(),
            type_name: comp.type_name.as_str().to_string(),
            impl_name: comp.impl_name.as_ref().map(|n| n.as_str().to_string()),
            features,
            connections,
            children,
            diagnostics: vec![],
        }
    }
}
```

**Step 3: Run test**

Run: `cargo test -p spar-hir serde_instance_tree`
Expected: PASS

**Step 4: Commit**

```bash
git add -A
git commit -m "feat(spar-hir): add serializable InstanceNode tree projection

Adds InstanceNode, InstanceFeature, InstanceConnection types with serde
derives. Instance::to_serializable() flattens the arena-based
SystemInstance into a JSON-friendly tree."
```

---

### Task 4: Create JSON output types for CLI

**Files:**
- Modify: `spar/crates/spar-analysis/Cargo.toml`
- Modify: `spar/crates/spar-analysis/src/lib.rs`

The CLI JSON output also needs to serialize analysis diagnostics. Add serde to spar-analysis.

**Step 1: Add serde to spar-analysis**

In `spar/crates/spar-analysis/Cargo.toml`, add:

```toml
serde = { version = "1", features = ["derive"] }
```

**Step 2: Add Serialize to AnalysisDiagnostic and Severity**

In `spar/crates/spar-analysis/src/lib.rs`:

```rust
use serde::Serialize;
```

Change derives:
- `AnalysisDiagnostic` (line 44): add `Serialize`
- `Severity` (line 54): add `Serialize`

**Step 3: Build**

Run: `cargo build -p spar-analysis`
Expected: compiles.

**Step 4: Commit**

```bash
git add -A
git commit -m "feat(spar-analysis): add serde Serialize to AnalysisDiagnostic and Severity"
```

---

### Task 5: Add --format json to spar CLI

**Files:**
- Modify: `spar/crates/spar-cli/Cargo.toml`
- Modify: `spar/crates/spar-cli/src/main.rs`

**Step 1: Add spar-hir dependency to CLI**

The CLI currently uses spar-hir-def directly. Add spar-hir for the public API:

In `spar/crates/spar-cli/Cargo.toml`, add:

```toml
spar-hir.workspace = true
```

Verify `serde` and `serde_json` are already there (they are: `serde_json = "1"` exists).

**Step 2: Define JSON output structs**

Add near the top of `main.rs`, after imports:

```rust
use serde::Serialize;

/// Top-level JSON output for `spar analyze --format json`.
#[derive(Serialize)]
struct JsonOutput {
    root: String,
    packages: Vec<spar_hir::Package>,
    instance: Option<spar_hir::InstanceNode>,
    diagnostics: Vec<spar_analysis::AnalysisDiagnostic>,
}
```

**Step 3: Add --format flag to cmd_analyze**

Modify `cmd_analyze` to accept `--format text|json`:

In the argument parsing loop inside `cmd_analyze`, add:

```rust
"--format" => {
    i += 1;
    if i < args.len() {
        format = Some(args[i].clone());
    } else {
        eprintln!("--format requires a value (text|json)");
        process::exit(1);
    }
}
```

Add `let mut format = None;` at the top alongside `let mut root = None;`.

After running analyses, add the JSON output path:

```rust
if format.as_deref() == Some("json") {
    let hir_db = spar_hir::Database::from_aadl(
        &files.iter().map(|f| (f.clone(), read_file(f))).collect::<Vec<_>>()
    );
    let instance_tree = hir_db.instantiate(&root).map(|i| i.to_serializable());
    let output = JsonOutput {
        root: root.clone(),
        packages: hir_db.packages(),
        instance: instance_tree,
        diagnostics: diagnostics.clone(),
    };
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
} else {
    // existing text output...
}
```

**Step 4: Test manually**

Run: `cargo run -p spar -- analyze --root FlightControl::Controller.Basic --format json test-data/vehicle.aadl`

Expected: JSON output with `packages`, `instance`, and `diagnostics` fields. If vehicle.aadl doesn't have that root, use whatever root exists.

Run: `cargo run -p spar -- analyze --root FlightControl::Controller.Basic test-data/vehicle.aadl`

Expected: existing text output unchanged.

**Step 5: Also add --format json to cmd_items**

Modify `cmd_items` similarly:

```rust
fn cmd_items(args: &[String]) {
    let mut format = None;
    let mut file_args = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                }
            }
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                process::exit(1);
            }
            s => file_args.push(s.to_string()),
        }
        i += 1;
    }
    let files = if file_args.is_empty() {
        eprintln!("Missing file argument(s)");
        process::exit(1);
    } else {
        file_args
    };

    if format.as_deref() == Some("json") {
        let sources: Vec<_> = files.iter().map(|f| (f.clone(), read_file(f))).collect();
        let hir_db = spar_hir::Database::from_aadl(&sources);
        let pkgs = hir_db.packages();
        println!("{}", serde_json::to_string_pretty(&pkgs).unwrap());
    } else {
        // existing text output (keep current code)
        let db = spar_hir_def::HirDefDatabase::default();
        for file_path in &files {
            let source = read_file(file_path);
            let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
            let tree = spar_hir_def::file_item_tree(&db, sf);
            println!("=== {} ===", file_path);
            for (_idx, pkg) in tree.packages.iter() {
                println!("  package {}", pkg.name);
                println!("    with: {:?}", pkg.with_clauses.iter().map(|n| n.as_str()).collect::<Vec<_>>());
                print_items("    public", &pkg.public_items, &tree);
                print_items("    private", &pkg.private_items, &tree);
            }
            for (_idx, ps) in tree.property_sets.iter() {
                println!("  property set {}", ps.name);
                for d in &ps.property_defs {
                    println!("    property {}", d.name);
                }
                for c in &ps.property_constants {
                    println!("    constant {}", c.name);
                }
            }
        }
    }
}
```

**Step 6: Update usage text**

Update `print_usage` to show the new format flag:

```
eprintln!("  items    [--format text|json] <file...>");
eprintln!("  analyze  --root Package::Type.Impl [--format text|json] <file...>");
```

**Step 7: Commit**

```bash
git add -A
git commit -m "feat(spar-cli): add --format json output to analyze and items commands

JSON output includes packages, instance tree, and analysis diagnostics.
Enables Layer 1 integration with rivet via CLI piping."
```

---

### Task 6: Fix units conversion factor parsing (#13)

**Files:**
- Modify: `spar/crates/spar-hir-def/src/item_tree/lower.rs:633-691`
- Test: add test in existing test suite

**Step 1: Write failing test**

Add a test that parses a units type with conversion factors. Find existing test infrastructure — likely in `spar/crates/spar-hir-def/src/item_tree/lower.rs` tests or `spar/crates/spar-hir/src/lib.rs` tests.

Use the spar-hir facade for the test:

```rust
#[test]
fn units_type_conversion_factors() {
    let db = make_db(
        r#"
        property set Time_Props is
          Time_Units: type units (ps, ns => ps * 1000, us => ns * 1000, ms => us * 1000, sec => ms * 1000, min => sec * 60, hr => min * 60);
        end Time_Props;
        "#,
    );
    // The property set should parse without errors.
    // We can verify via items that the property set exists.
    let pkgs = db.packages();
    // Property sets are at the tree level, not package level in the hir facade.
    // Just verify no panic during parsing.
}
```

This is a parsing correctness test. The bug is in how the conversion factor index tracks. Looking at `lower.rs:633-691`, the issue is that when parsing `ns => ps * 1000`, after consuming the factor on line 668-669, `idx` is incremented, and then `continue` skips the normal `idx += 1` on line 688. But when there's no `*` sign (just `base_name` with no factor), the code falls through to line 678 which pushes with factor "1" but doesn't `continue`, so `idx` gets double-incremented.

**Step 2: Fix the bug**

In the `else` branch at line 674-679, add `continue;` after the push:

```rust
} else {
    units.push((
        unit_name,
        Some((base_name, "1".to_string())),
    ));
    continue; // skip normal idx increment
}
```

**Step 3: Run test**

Run: `cargo test -p spar-hir units_type`
Expected: PASS

Run: `cargo test` (full suite)
Expected: all PASS.

**Step 4: Commit**

```bash
git add -A
git commit -m "fix(spar-hir-def): fix units conversion factor parsing (#13)

Add missing continue after base-unit-only conversion factor to prevent
double-incrementing the token index."
```

---

### Task 7: Create AADL schema for rivet

**Files:**
- Create: `rivet/schemas/aadl.yaml`

**Step 1: Write the schema**

Create `rivet/schemas/aadl.yaml`:

```yaml
# AADL Architecture schema for rivet
#
# Maps AADL components, analysis results, and flows into the rivet
# artifact model. Bridges ASPICE SYS.3/SWE.2 architecture levels
# with formal AADL models analyzed by spar.
#
# V-model mapping:
#   stakeholder-req -> system-req -> system-arch-component
#                                          | allocated-from
#                                    aadl-component
#                                          | verifies
#                                    aadl-analysis-result

schema:
  name: aadl
  version: "0.1.0"
  namespace: "http://pulseengine.dev/ns/aadl#"
  extends: [common]
  description: >
    AADL architecture model artifact types for spar integration.

artifact-types:

  - name: aadl-component
    description: AADL component type or implementation imported from spar
    fields:
      - name: category
        type: string
        required: true
        allowed-values:
          - system
          - process
          - thread
          - thread-group
          - processor
          - virtual-processor
          - memory
          - bus
          - virtual-bus
          - device
          - subprogram
          - subprogram-group
          - data
          - abstract
      - name: aadl-package
        type: string
        required: true
        description: AADL package containing this component
      - name: classifier-kind
        type: string
        required: false
        allowed-values: [type, implementation, feature-group-type]
      - name: features
        type: structured
        required: false
        description: Port/access/feature group declarations
      - name: properties
        type: structured
        required: false
        description: AADL property associations
      - name: aadl-file
        type: string
        required: false
        description: Source .aadl file path
    link-fields:
      - name: allocated-from
        link-type: allocated-from
        target-types: [system-req, sw-req, system-arch-component]
        required: false
        cardinality: zero-or-many

  - name: aadl-analysis-result
    description: Output of a spar analysis pass
    fields:
      - name: analysis-name
        type: string
        required: true
        description: Name of the analysis (e.g., connectivity, scheduling, latency)
      - name: severity
        type: string
        required: true
        allowed-values: [error, warning, info]
      - name: component-path
        type: string
        required: false
        description: Hierarchical path to the component (e.g., root/subsystem/cpu)
      - name: details
        type: text
        required: false
    link-fields:
      - name: analyzes
        link-type: verifies
        target-types: [aadl-component]
        required: false
        cardinality: zero-or-many

  - name: aadl-flow
    description: End-to-end flow with latency bounds
    fields:
      - name: flow-kind
        type: string
        required: true
        allowed-values: [source, sink, path, end-to-end]
      - name: latency-best-ms
        type: number
        required: false
      - name: latency-worst-ms
        type: number
        required: false
      - name: segments
        type: structured
        required: false
    link-fields:
      - name: part-of
        link-type: allocated-from
        target-types: [aadl-component]
        required: false
        cardinality: zero-or-many

# AADL-specific link types
link-types:
  - name: modeled-by
    inverse: models
    description: An architecture component is modeled by an AADL component
    source-types: [system-arch-component, sw-arch-component]
    target-types: [aadl-component]

# Traceability rules
traceability-rules:
  - name: aadl-component-has-allocation
    description: Every AADL component should trace to a requirement or architecture element
    source-type: aadl-component
    required-backlink: allocated-from
    from-types: [system-req, sw-req, system-arch-component]
    severity: warning

  - name: safety-critical-has-analysis
    description: Safety-critical AADL components should have analysis results
    source-type: aadl-component
    required-backlink: verifies
    from-types: [aadl-analysis-result]
    severity: warning
```

**Step 2: Validate schema loads**

Run from rivet directory:

```bash
cd /Volumes/Home/git/sdlc && cargo test -- schema
```

Expected: existing schema tests pass. The new schema file doesn't break anything since it's only loaded on demand.

**Step 3: Write a schema loading test**

Add a test in `rivet/rivet-core/tests/integration.rs` (or wherever schema tests live):

```rust
#[test]
fn aadl_schema_loads() {
    let schemas_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("schemas");
    let common = rivet_core::schema::Schema::load_file(&schemas_dir.join("common.yaml")).unwrap();
    let aadl = rivet_core::schema::Schema::load_file(&schemas_dir.join("aadl.yaml")).unwrap();
    let merged = rivet_core::schema::Schema::merge(&[common, aadl]);
    assert!(merged.artifact_type("aadl-component").is_some());
    assert!(merged.artifact_type("aadl-analysis-result").is_some());
    assert!(merged.artifact_type("aadl-flow").is_some());
    assert!(merged.link_type("modeled-by").is_some());
}
```

Run: `cd /Volumes/Home/git/sdlc && cargo test aadl_schema_loads`
Expected: PASS

**Step 4: Commit**

```bash
cd /Volumes/Home/git/sdlc
git add schemas/aadl.yaml rivet-core/tests/integration.rs
git commit -m "feat(schema): add AADL artifact type schema

Defines aadl-component, aadl-analysis-result, and aadl-flow artifact
types with traceability rules linking to ASPICE requirement types.
Enables spar analysis results to flow into rivet traceability."
```

---

### Task 8: Create AADL adapter for rivet (CLI mode / Layer 1)

**Files:**
- Create: `rivet/rivet-core/src/formats/aadl.rs`
- Modify: `rivet/rivet-core/src/formats/mod.rs`
- Modify: `rivet/rivet-core/src/lib.rs` (wire into `load_artifacts`)

**Step 1: Write failing test**

Add to `rivet/rivet-core/tests/integration.rs`:

```rust
#[test]
fn aadl_adapter_parses_spar_json() {
    use rivet_core::adapter::{Adapter, AdapterSource, AdapterConfig};
    use rivet_core::formats::aadl::AadlAdapter;

    // Simulated spar JSON output
    let json = r#"{
        "root": "Pkg::Sys.Impl",
        "packages": [
            {
                "name": "Pkg",
                "with_clauses": [],
                "component_types": [
                    {
                        "name": "Sys",
                        "category": "System",
                        "extends": null,
                        "features": [],
                        "flows": [],
                        "modes": [],
                        "mode_transitions": [],
                        "properties": []
                    }
                ],
                "component_impls": [
                    {
                        "name": "Sys.Impl",
                        "category": "System",
                        "type_name": "Sys",
                        "impl_name": "Impl",
                        "extends": null,
                        "subcomponents": [],
                        "connections": [],
                        "flows": [],
                        "e2e_flows": [],
                        "modes": [],
                        "mode_transitions": [],
                        "properties": []
                    }
                ],
                "feature_group_types": []
            }
        ],
        "instance": {
            "name": "Sys",
            "category": "System",
            "package": "Pkg",
            "type_name": "Sys",
            "impl_name": "Impl",
            "features": [],
            "connections": [],
            "children": [],
            "diagnostics": []
        },
        "diagnostics": []
    }"#;

    let adapter = AadlAdapter::new();
    let source = AdapterSource::Bytes(json.as_bytes().to_vec());
    let config = AdapterConfig::default();
    let artifacts = adapter.import(&source, &config).unwrap();

    // Should produce artifacts for: Sys type + Sys.Impl impl
    assert!(artifacts.len() >= 2);
    assert!(artifacts.iter().any(|a| a.artifact_type == "aadl-component" && a.id == "AADL-Pkg-Sys"));
    assert!(artifacts.iter().any(|a| a.artifact_type == "aadl-component" && a.id == "AADL-Pkg-Sys.Impl"));
}
```

Run: `cd /Volumes/Home/git/sdlc && cargo test aadl_adapter_parses`
Expected: FAIL — module doesn't exist.

**Step 2: Create the adapter**

Create `rivet/rivet-core/src/formats/aadl.rs`:

```rust
//! AADL adapter for rivet.
//!
//! Imports AADL components and analysis results from spar's JSON output.
//!
//! Two modes:
//! - **Bytes mode**: parse pre-computed spar JSON (from CLI piping or tests)
//! - **Directory mode**: find .aadl files, call `spar analyze --format json`, parse output

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::adapter::{Adapter, AdapterConfig, AdapterSource};
use crate::error::Error;
use crate::model::{Artifact, Link};

pub struct AadlAdapter {
    supported: Vec<String>,
}

impl AadlAdapter {
    pub fn new() -> Self {
        Self {
            supported: vec![
                "aadl-component".to_string(),
                "aadl-analysis-result".to_string(),
                "aadl-flow".to_string(),
            ],
        }
    }
}

impl Default for AadlAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Adapter for AadlAdapter {
    fn id(&self) -> &str {
        "aadl"
    }

    fn name(&self) -> &str {
        "AADL (spar)"
    }

    fn supported_types(&self) -> &[String] {
        &self.supported
    }

    fn import(
        &self,
        source: &AdapterSource,
        config: &AdapterConfig,
    ) -> Result<Vec<Artifact>, Error> {
        match source {
            AdapterSource::Bytes(bytes) => {
                let content = std::str::from_utf8(bytes)
                    .map_err(|e| Error::Adapter(format!("invalid UTF-8: {}", e)))?;
                parse_spar_json(content)
            }
            AdapterSource::Directory(dir) => import_from_directory(dir, config),
            AdapterSource::Path(path) => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| Error::Io(format!("{}: {}", path.display(), e)))?;
                parse_spar_json(&content)
            }
        }
    }

    fn export(&self, _artifacts: &[Artifact], _config: &AdapterConfig) -> Result<Vec<u8>, Error> {
        Err(Error::Adapter("AADL export not supported (use spar directly)".to_string()))
    }
}

/// Parse spar's JSON output into rivet artifacts.
fn parse_spar_json(json: &str) -> Result<Vec<Artifact>, Error> {
    let output: SparOutput = serde_json::from_str(json)
        .map_err(|e| Error::Adapter(format!("invalid spar JSON: {}", e)))?;

    let mut artifacts = Vec::new();

    // Convert packages -> component artifacts
    for pkg in &output.packages {
        for ct in &pkg.component_types {
            artifacts.push(component_type_to_artifact(&pkg.name, ct));
        }
        for ci in &pkg.component_impls {
            artifacts.push(component_impl_to_artifact(&pkg.name, ci));
        }
    }

    // Convert diagnostics -> analysis result artifacts
    for (i, diag) in output.diagnostics.iter().enumerate() {
        artifacts.push(diagnostic_to_artifact(i, diag));
    }

    Ok(artifacts)
}

fn component_type_to_artifact(pkg_name: &str, ct: &SparComponentType) -> Artifact {
    let id = format!("AADL-{}-{}", pkg_name, ct.name);
    let mut fields = BTreeMap::new();
    fields.insert("category".to_string(), serde_yaml::Value::String(ct.category.clone()));
    fields.insert("aadl-package".to_string(), serde_yaml::Value::String(pkg_name.to_string()));
    fields.insert("classifier-kind".to_string(), serde_yaml::Value::String("type".to_string()));

    Artifact {
        id,
        artifact_type: "aadl-component".to_string(),
        title: format!("{} {} {}", ct.category.to_lowercase(), pkg_name, ct.name),
        description: None,
        status: Some("imported".to_string()),
        tags: vec!["aadl".to_string()],
        links: vec![],
        fields,
        source_file: None,
    }
}

fn component_impl_to_artifact(pkg_name: &str, ci: &SparComponentImpl) -> Artifact {
    let id = format!("AADL-{}-{}", pkg_name, ci.name);
    let mut fields = BTreeMap::new();
    fields.insert("category".to_string(), serde_yaml::Value::String(ci.category.clone()));
    fields.insert("aadl-package".to_string(), serde_yaml::Value::String(pkg_name.to_string()));
    fields.insert("classifier-kind".to_string(), serde_yaml::Value::String("implementation".to_string()));

    Artifact {
        id,
        artifact_type: "aadl-component".to_string(),
        title: format!("{} implementation {} ({})", ci.category.to_lowercase(), ci.name, pkg_name),
        description: None,
        status: Some("imported".to_string()),
        tags: vec!["aadl".to_string()],
        links: vec![],
        fields,
        source_file: None,
    }
}

fn diagnostic_to_artifact(index: usize, diag: &SparDiagnostic) -> Artifact {
    let id = format!("AADL-DIAG-{:04}", index + 1);
    let mut fields = BTreeMap::new();
    fields.insert("analysis-name".to_string(), serde_yaml::Value::String(diag.analysis.clone()));
    fields.insert("severity".to_string(), serde_yaml::Value::String(diag.severity.clone()));
    fields.insert("component-path".to_string(), serde_yaml::Value::String(diag.path.join("/")));
    fields.insert("details".to_string(), serde_yaml::Value::String(diag.message.clone()));

    // Link to the component at the diagnostic path
    let component_path = &diag.path;
    let links = if component_path.len() >= 2 {
        // Try to construct the AADL component ID from the path
        vec![Link {
            link_type: "verifies".to_string(),
            target: format!("AADL-{}", component_path.join("-")),
        }]
    } else {
        vec![]
    };

    Artifact {
        id,
        artifact_type: "aadl-analysis-result".to_string(),
        title: format!("[{}] {}", diag.severity, diag.message),
        description: Some(diag.message.clone()),
        status: None,
        tags: vec!["aadl".to_string(), diag.analysis.clone()],
        links,
        fields,
        source_file: None,
    }
}

/// Call spar CLI and parse JSON output.
fn import_from_directory(dir: &Path, config: &AdapterConfig) -> Result<Vec<Artifact>, Error> {
    let root = config.get("root").ok_or_else(|| {
        Error::Adapter("AADL adapter requires 'root' config (e.g., Package::Type.Impl)".to_string())
    })?;

    // Find .aadl files
    let aadl_files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Io(format!("{}: {}", dir.display(), e)))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "aadl"))
        .map(|e| e.path())
        .collect();

    if aadl_files.is_empty() {
        return Ok(vec![]);
    }

    // Call spar analyze --format json
    let mut cmd = Command::new("spar");
    cmd.arg("analyze")
        .arg("--root")
        .arg(root)
        .arg("--format")
        .arg("json");
    for f in &aadl_files {
        cmd.arg(f);
    }

    let output = cmd.output().map_err(|e| {
        Error::Adapter(format!("failed to run spar: {} (is spar in PATH?)", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Adapter(format!("spar failed: {}", stderr)));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| Error::Adapter(format!("invalid spar output: {}", e)))?;

    parse_spar_json(&stdout)
}

// ── Spar JSON types (mirrors spar-hir output) ────────────────────

#[derive(serde::Deserialize)]
struct SparOutput {
    #[allow(dead_code)]
    root: String,
    packages: Vec<SparPackage>,
    #[allow(dead_code)]
    instance: Option<serde_json::Value>,
    diagnostics: Vec<SparDiagnostic>,
}

#[derive(serde::Deserialize)]
struct SparPackage {
    name: String,
    component_types: Vec<SparComponentType>,
    component_impls: Vec<SparComponentImpl>,
}

#[derive(serde::Deserialize)]
struct SparComponentType {
    name: String,
    category: String,
}

#[derive(serde::Deserialize)]
struct SparComponentImpl {
    name: String,
    category: String,
}

#[derive(serde::Deserialize)]
struct SparDiagnostic {
    severity: String,
    message: String,
    path: Vec<String>,
    analysis: String,
}
```

**Step 3: Wire into module and load_artifacts**

Update `rivet/rivet-core/src/formats/mod.rs`:

```rust
pub mod aadl;
pub mod generic;
pub mod stpa;
```

Update `rivet/rivet-core/src/lib.rs` — add the `"aadl"` match arm in `load_artifacts`:

```rust
"aadl" => {
    let adapter = formats::aadl::AadlAdapter::new();
    adapter::Adapter::import(&adapter, &source_input, &adapter_config)
}
```

**Step 4: Run test**

Run: `cd /Volumes/Home/git/sdlc && cargo test aadl_adapter_parses`
Expected: PASS

**Step 5: Run full test suite**

Run: `cd /Volumes/Home/git/sdlc && cargo test`
Expected: all PASS.

**Step 6: Commit**

```bash
cd /Volumes/Home/git/sdlc
git add rivet-core/src/formats/aadl.rs rivet-core/src/formats/mod.rs rivet-core/src/lib.rs rivet-core/tests/integration.rs
git commit -m "feat(adapter): add AADL adapter for spar integration (Layer 1)

AadlAdapter imports AADL components and analysis results from spar's
JSON output. Supports both direct JSON parsing (Bytes mode) and CLI
mode (calls 'spar analyze --format json'). Produces aadl-component
and aadl-analysis-result artifacts for rivet traceability."
```

---

### Task 9: End-to-end integration test

**Files:**
- Create: `rivet/examples/aadl/rivet.yaml`
- Create: `rivet/examples/aadl/artifacts/requirements.yaml`
- Create: `rivet/examples/aadl/aadl/flight-control.aadl`

**Step 1: Create example project**

`rivet/examples/aadl/rivet.yaml`:

```yaml
project:
  name: aadl-integration-example
  version: "0.1.0"
  schemas:
    - common
    - aspice
    - aadl

sources:
  - path: artifacts
    format: generic-yaml
```

`rivet/examples/aadl/artifacts/requirements.yaml`:

```yaml
artifacts:
  - id: SYSREQ-001
    type: system-req
    title: Flight controller shall process sensor data within 50ms
    status: approved
    fields:
      req-type: performance
      priority: must
    links:
      - type: derives-from
        target: STAKE-001

  - id: STAKE-001
    type: stakeholder-req
    title: System shall respond to pilot inputs in real-time
    status: approved

  - id: AADL-FlightControl-Controller
    type: aadl-component
    title: system FlightControl Controller
    status: imported
    tags: [aadl]
    fields:
      category: system
      aadl-package: FlightControl
      classifier-kind: type
    links:
      - type: allocated-from
        target: SYSREQ-001
```

`rivet/examples/aadl/aadl/flight-control.aadl`:

```
package FlightControl
public
  system Controller
    features
      sensor_in: in data port;
      actuator_out: out data port;
  end Controller;

  process NavProcess
    features
      inp: in data port;
      outp: out data port;
  end NavProcess;

  system implementation Controller.Basic
    subcomponents
      nav: process NavProcess;
    connections
      c1: port sensor_in -> nav.inp;
      c2: port nav.outp -> actuator_out;
  end Controller.Basic;
end FlightControl;
```

**Step 2: Validate with rivet**

Run: `cd /Volumes/Home/git/sdlc && cargo run -- validate --project examples/aadl/rivet.yaml`

Expected: validation passes (or produces only warnings for missing traceability links, not errors).

**Step 3: Test coverage report**

Run: `cd /Volumes/Home/git/sdlc && cargo run -- coverage --project examples/aadl/rivet.yaml`

Expected: shows coverage of AADL components with requirement traceability.

**Step 4: Commit**

```bash
cd /Volumes/Home/git/sdlc
git add examples/aadl/
git commit -m "feat(examples): add AADL integration example project

Demonstrates spar+rivet integration with AADL components linked to
ASPICE requirements. Shows the Layer 1 integration pattern."
```

---

## Summary

| Task | Repo | What | Issue |
|------|------|------|-------|
| 1 | spar | Serde derives on hir-def enums | #15 |
| 2 | spar | Serde derives on hir public types | #15 |
| 3 | spar | Serializable InstanceNode projection | #15 |
| 4 | spar | Serde on AnalysisDiagnostic | #15 |
| 5 | spar | `--format json` CLI flag | #15 |
| 6 | spar | Fix units parsing | #13 |
| 7 | rivet | AADL schema (aadl.yaml) | integration |
| 8 | rivet | AADL adapter (Layer 1) | integration |
| 9 | rivet | End-to-end example | integration |

Tasks 1-6 are in spar, tasks 7-9 are in rivet. Tasks 1-4 are sequential (each builds on the last). Tasks 6-9 can begin after task 5 completes.
