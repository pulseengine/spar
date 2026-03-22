# v0.4.0 Implementation Plan — Code Generation + SysML v2 Parser

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two parallel tracks — (A) `spar-codegen` generates WIT + Rust + verification from AADL, (B) `spar-sysml2` parses SysML v2 textual notation and lowers to AADL.

**Architecture:** Track A uses the existing `spar-transform` WIT infrastructure + instance model property accessors. Track B follows the `spar-parser` pattern (rowan CST, recursive descent). Both are independent crates that integrate via CLI commands.

**Tech Stack:** Rust, rowan 0.16, petgraph 0.7, spar-hir-def (instance model), spar-transform (WIT), spar-analysis (property accessors)

**Spec:** `docs/plans/2026-03-22-v040-codegen-sysml2-design.md`

---

## Track A: Code Generation (`spar-codegen`)

### File Structure

| File | Responsibility |
|------|---------------|
| `crates/spar-codegen/Cargo.toml` | Dependencies: spar-hir-def, spar-transform, spar-analysis |
| `crates/spar-codegen/src/lib.rs` | Public API: `generate()` → `GeneratedWorkspace` |
| `crates/spar-codegen/src/wit_gen.rs` | AADL process → WIT world file |
| `crates/spar-codegen/src/rust_gen.rs` | AADL process → Rust crate skeleton |
| `crates/spar-codegen/src/config_gen.rs` | AADL properties → `#[aadl(...)]` config.rs |
| `crates/spar-codegen/src/test_gen.rs` | Generate contract + timing tests |
| `crates/spar-codegen/src/proof_gen.rs` | Generate Lean4 proofs + Kani harnesses |
| `crates/spar-codegen/src/doc_gen.rs` | Generate rivet design docs with frontmatter |
| `crates/spar-codegen/src/workspace_gen.rs` | Generate Cargo.toml + BUILD.bazel |
| `crates/spar-cli/src/main.rs` | Add `codegen` command |

### Task A1: Create spar-codegen crate skeleton

**Files:**
- Create: `crates/spar-codegen/Cargo.toml`
- Create: `crates/spar-codegen/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add member + dep)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "spar-codegen"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Code generation from AADL models — WIT, Rust, verification"

[dependencies]
spar-hir-def.workspace = true
spar-analysis.workspace = true
spar-transform.workspace = true
serde.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Create lib.rs with public API**

```rust
//! Code generation from AADL models.
//!
//! Generates WIT interfaces, Rust crate skeletons, verification tests,
//! formal proofs, and rivet documentation from AADL system instances.

pub mod wit_gen;
pub mod rust_gen;
pub mod config_gen;
pub mod test_gen;
pub mod proof_gen;
pub mod doc_gen;
pub mod workspace_gen;

use spar_hir_def::instance::SystemInstance;

/// A generated file with its relative path and content.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

/// Complete generated workspace output.
#[derive(Debug)]
pub struct GeneratedWorkspace {
    pub files: Vec<GeneratedFile>,
}

/// Options for code generation.
#[derive(Debug, Clone)]
pub struct CodegenOptions {
    pub root_classifier: String,
    pub output_dir: String,
    pub generate_wit: bool,
    pub generate_rust: bool,
    pub generate_bazel: bool,
    pub generate_tests: bool,
    pub generate_proofs: bool,
    pub generate_rivet_docs: bool,
}

impl Default for CodegenOptions {
    fn default() -> Self {
        Self {
            root_classifier: String::new(),
            output_dir: "generated".into(),
            generate_wit: true,
            generate_rust: true,
            generate_bazel: true,
            generate_tests: true,
            generate_proofs: true,
            generate_rivet_docs: true,
        }
    }
}

/// Generate code from an AADL system instance.
pub fn generate(
    instance: &SystemInstance,
    options: &CodegenOptions,
) -> GeneratedWorkspace {
    let mut files = Vec::new();

    // Phase 1: WIT interfaces
    if options.generate_wit {
        files.extend(wit_gen::generate_wit_files(instance));
    }

    // Phase 2: Rust crate skeletons
    if options.generate_rust {
        files.extend(rust_gen::generate_rust_crates(instance));
    }

    // Phase 3: AADL config attributes
    files.extend(config_gen::generate_config_files(instance));

    // Phase 4: Verification tests
    if options.generate_tests {
        files.extend(test_gen::generate_test_files(instance));
    }

    // Phase 5: Formal proofs
    if options.generate_proofs {
        files.extend(proof_gen::generate_proof_files(instance));
    }

    // Phase 6: rivet documentation
    if options.generate_rivet_docs {
        files.extend(doc_gen::generate_doc_files(instance, options));
    }

    // Phase 7: Workspace files
    files.extend(workspace_gen::generate_workspace_files(
        instance, options,
    ));

    GeneratedWorkspace { files }
}
```

- [ ] **Step 3: Create empty module files with doc comments**

- [ ] **Step 4: Add to workspace, verify builds**

Run: `cargo build -p spar-codegen`

- [ ] **Step 5: Commit**

```
feat(codegen): create spar-codegen crate skeleton
```

---

### Task A2: WIT generation from AADL processes

**Files:**
- Create: `crates/spar-codegen/src/wit_gen.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn process_generates_wit_world() {
    let instance = build_test_system(); // system with 1 process, 2 threads, data ports
    let files = generate_wit_files(&instance);
    assert!(!files.is_empty());
    let wit = &files[0];
    assert!(wit.path.ends_with(".wit"));
    assert!(wit.content.contains("world"));
    assert!(wit.content.contains("export"));
}
```

- [ ] **Step 2: Implement WIT generation**

Walk the instance model. For each process component:
- Create a WIT `world` named after the process
- For each thread child: create a WIT `export` interface
- For each data port feature: create a WIT `stream<T>` or `future<T>`
- For each data type classifier: create a WIT `record`

Use the existing `spar_transform::wit::WitTransform::from_aadl()` pattern for type mapping.

- [ ] **Step 3: Write more tests** — multi-process, event ports, access features

- [ ] **Step 4: Commit**

```
feat(codegen): WIT interface generation from AADL processes
```

---

### Task A3: Rust crate skeleton generation

**Files:**
- Create: `crates/spar-codegen/src/rust_gen.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn process_generates_rust_crate() {
    let instance = build_test_system();
    let files = generate_rust_crates(&instance);
    let lib = files.iter().find(|f| f.path.ends_with("lib.rs")).unwrap();
    assert!(lib.content.contains("pub trait"));
    assert!(lib.content.contains("async fn"));
}
```

- [ ] **Step 2: Implement Rust skeleton generation**

For each process:
- `lib.rs`: trait with async fn per thread, typed port parameters
- `ports.rs`: `DataPort<T, Dir>`, `EventDataPort<T, Dir>` structs
- `types.rs`: data type structs from AADL classifiers

- [ ] **Step 3: Commit**

```
feat(codegen): Rust crate skeleton generation from AADL
```

---

### Task A4: AADL config attribute generation

**Files:**
- Create: `crates/spar-codegen/src/config_gen.rs`

- [ ] **Step 1: Implement config.rs generation**

For each thread: extract Period, Deadline, WCET, Processor_Binding, Dispatch via `spar_analysis::property_accessors` and emit `pub const` declarations.

- [ ] **Step 2: Commit**

```
feat(codegen): #[aadl] config attribute generation
```

---

### Task A5: Test generation (contract + timing)

**Files:**
- Create: `crates/spar-codegen/src/test_gen.rs`

- [ ] **Step 1: Generate contract_test.rs**

Per thread: port type assertions, direction assertions, connection assertions.

- [ ] **Step 2: Generate timing_test.rs**

Per thread with WCET: execution time measurement against bound.

- [ ] **Step 3: Commit**

```
feat(codegen): contract + timing test generation
```

---

### Task A6: Formal proof generation (Lean4 + Kani)

**Files:**
- Create: `crates/spar-codegen/src/proof_gen.rs`

- [ ] **Step 1: Generate scheduling.lean**

Per processor: collect bound threads, emit `compute_response_time` theorem with concrete values.

- [ ] **Step 2: Generate kani_harness.rs**

Per thread: `#[kani::proof]` with symbolic WCET bounded by declared max.

- [ ] **Step 3: Commit**

```
feat(codegen): Lean4 proof + Kani harness generation
```

---

### Task A7: rivet document generation

**Files:**
- Create: `crates/spar-codegen/src/doc_gen.rs`

- [ ] **Step 1: Generate design docs with frontmatter**

Per process: markdown with YAML frontmatter (id, type, links), property table, port table, connection table, verification table.

- [ ] **Step 2: Generate verification.yaml**

Per process: verification-verdict records for each verification layer.

- [ ] **Step 3: Commit**

```
feat(codegen): rivet design doc + verification artifact generation
```

---

### Task A8: Workspace generation (Cargo + Bazel)

**Files:**
- Create: `crates/spar-codegen/src/workspace_gen.rs`

- [ ] **Step 1: Generate workspace Cargo.toml**

Member list from processes, workspace deps.

- [ ] **Step 2: Generate per-crate Cargo.toml**

Dependencies: wit-bindgen (optional), spar-verify-macros (for #[aadl]).

- [ ] **Step 3: Generate BUILD.bazel files**

rules_wasm_component, rules_lean, rules_kani targets.

- [ ] **Step 4: Commit**

```
feat(codegen): Cargo workspace + Bazel BUILD generation
```

---

### Task A9: CLI integration (`spar codegen`)

**Files:**
- Modify: `crates/spar-cli/src/main.rs`
- Modify: `crates/spar-cli/Cargo.toml`

- [ ] **Step 1: Add codegen command**

```rust
"codegen" => cmd_codegen(&args[2..]),
```

- [ ] **Step 2: Implement cmd_codegen**

Parse args (--root, --output, --format, --verify, --rivet, --dry-run), build instance, call `spar_codegen::generate()`, write files to disk.

- [ ] **Step 3: Commit**

```
feat(cli): spar codegen command
```

---

## Track B: SysML v2 Parser (`spar-sysml2`)

### File Structure

| File | Responsibility |
|------|---------------|
| `crates/spar-sysml2/Cargo.toml` | Dependencies: rowan, spar-hir-def |
| `crates/spar-sysml2/src/lib.rs` | Public API: parse, lower, extract |
| `crates/spar-sysml2/src/syntax_kind.rs` | SysML2SyntaxKind enum |
| `crates/spar-sysml2/src/lexer.rs` | KerML tokenizer |
| `crates/spar-sysml2/src/parser.rs` | Recursive descent parser |
| `crates/spar-sysml2/src/grammar/mod.rs` | Top-level grammar |
| `crates/spar-sysml2/src/grammar/packages.rs` | package, import |
| `crates/spar-sysml2/src/grammar/parts.rs` | part def, part, port, connection |
| `crates/spar-sysml2/src/grammar/requirements.rs` | requirement def, satisfy, verify |
| `crates/spar-sysml2/src/grammar/constraints.rs` | constraint def |
| `crates/spar-sysml2/src/grammar/actions.rs` | action def, state def |
| `crates/spar-sysml2/src/grammar/expressions.rs` | Feature expressions, operators |
| `crates/spar-sysml2/src/lower.rs` | SysML v2 → AADL ItemTree |
| `crates/spar-sysml2/src/extract.rs` | Requirements → rivet YAML |
| `crates/spar-cli/src/main.rs` | Add `sysml2` subcommand |

### Task B1: Create spar-sysml2 crate skeleton + SyntaxKind

**Files:**
- Create: `crates/spar-sysml2/Cargo.toml`
- Create: `crates/spar-sysml2/src/lib.rs`
- Create: `crates/spar-sysml2/src/syntax_kind.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "spar-sysml2"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "SysML v2 (KerML) parser — rowan-based lossless CST"

[dependencies]
rowan.workspace = true
spar-hir-def.workspace = true
serde.workspace = true
serde_json.workspace = true
```

- [ ] **Step 2: Define SysML2SyntaxKind enum**

Follow `spar-parser/src/syntax_kind.rs` pattern. Key variants:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[repr(u16)]
pub enum SysML2SyntaxKind {
    // Tokens
    WHITESPACE = 0, COMMENT, IDENT, STRING_LIT, INT_LIT, REAL_LIT,
    // Punctuation
    SEMICOLON, COLON, COLON_COLON, DOT, COMMA, EQ, FAT_ARROW,
    L_BRACE, R_BRACE, L_BRACKET, R_BRACKET, L_PAREN, R_PAREN,
    SPECIALIZES, // :>
    TILDE,       // ~
    DOT_DOT,     // ..
    // Keywords — definitions
    PACKAGE_KW, IMPORT_KW, PART_KW, DEF_KW, PORT_KW, CONNECTION_KW,
    ACTION_KW, STATE_KW, REQUIREMENT_KW, CONSTRAINT_KW, INTERFACE_KW,
    ATTRIBUTE_KW, ITEM_KW, ALLOCATION_KW, ANALYSIS_KW, CASE_KW,
    CALCULATION_KW, VIEW_KW, VIEWPOINT_KW, RENDERING_KW,
    // Keywords — usage/relationships
    IN_KW, OUT_KW, INOUT_KW, REF_KW, ABSTRACT_KW,
    SATISFY_KW, VERIFY_KW, REFINE_KW, ALLOCATE_KW,
    CONNECT_KW, BIND_KW, FLOW_KW, SUCCESSION_KW,
    PERFORM_KW, EXHIBIT_KW, TRANSITION_KW, ACCEPT_KW,
    REDEFINES_KW, SUBSETS_KW, ABOUT_KW,
    // Keywords — visibility
    PUBLIC_KW, PRIVATE_KW, PROTECTED_KW,
    // Keywords — misc
    FIRST_KW, THEN_KW, IF_KW, ELSE_KW, WHILE_KW,
    TRUE_KW, FALSE_KW, NULL_KW,
    // Error/EOF
    ERROR, EOF,
    // Composite nodes
    SOURCE_FILE, PACKAGE, IMPORT_DECL, NAMESPACE_BODY,
    PART_DEF, PART_USAGE, PORT_DEF, PORT_USAGE,
    CONNECTION_DEF, CONNECTION_USAGE, INTERFACE_DEF,
    ACTION_DEF, ACTION_USAGE, STATE_DEF, STATE_USAGE,
    REQUIREMENT_DEF, REQUIREMENT_USAGE, CONSTRAINT_DEF,
    ATTRIBUTE_DEF, ATTRIBUTE_USAGE, ITEM_DEF, ITEM_USAGE,
    ALLOCATION_DEF, ALLOCATION_USAGE,
    FEATURE_DECL, FEATURE_VALUE, MULTIPLICITY,
    SPECIALIZATION, REDEFINITION, SUBSETTING,
    SATISFY_REQ, VERIFY_REQ, REFINE_REQ,
    TYPE_REF, QUALIFIED_NAME, NAME,
    BODY, EXPRESSION, BINARY_EXPR, LITERAL,
    __LAST,
}
```

- [ ] **Step 3: Implement rowan Language trait**

Same pattern as `spar-parser/src/syntax_kind.rs`.

- [ ] **Step 4: Add to workspace, verify builds**

- [ ] **Step 5: Commit**

```
feat(sysml2): create spar-sysml2 crate with SyntaxKind enum
```

---

### Task B2: KerML lexer

**Files:**
- Create: `crates/spar-sysml2/src/lexer.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn lex_part_def() {
    let tokens = lex("part def Vehicle;");
    assert_eq!(tokens[0].0, SysML2SyntaxKind::PART_KW);
    assert_eq!(tokens[1].0, SysML2SyntaxKind::WHITESPACE);
    assert_eq!(tokens[2].0, SysML2SyntaxKind::DEF_KW);
    assert_eq!(tokens[3].0, SysML2SyntaxKind::WHITESPACE);
    assert_eq!(tokens[4].0, SysML2SyntaxKind::IDENT);
    assert_eq!(tokens[4].1, "Vehicle");
    assert_eq!(tokens[5].0, SysML2SyntaxKind::SEMICOLON);
}
```

- [ ] **Step 2: Implement cursor-based lexer**

Follow `spar-parser/src/lexer.rs` pattern. SysML v2 is case-sensitive (unlike AADL). Handle: `//` line comments, `/* */` block comments, string literals, numeric literals, all keywords, `:>` specialization operator, `..` range.

- [ ] **Step 3: Write keyword + operator tests**

- [ ] **Step 4: Commit**

```
feat(sysml2): KerML lexer with all SysML v2 keywords
```

---

### Task B3: Recursive descent parser — packages + parts

**Files:**
- Create: `crates/spar-sysml2/src/parser.rs`
- Create: `crates/spar-sysml2/src/grammar/mod.rs`
- Create: `crates/spar-sysml2/src/grammar/packages.rs`
- Create: `crates/spar-sysml2/src/grammar/parts.rs`

- [ ] **Step 1: Implement parser infrastructure**

Follow `spar-parser/src/parser.rs` — marker-based builder with `start()`, `complete()`, rowan GreenNodeBuilder.

- [ ] **Step 2: Parse packages + imports**

```sysml
package Vehicle {
    import ScalarValues::*;

    part def ECU;
}
```

- [ ] **Step 3: Parse part def + part usage**

```sysml
part def ECU {
    attribute weight : Real;
    port sensorIn : ~SensorPort;
    port commandOut : CommandPort;
}

part ecu : ECU;
```

- [ ] **Step 4: Parse port def + connection def**

```sysml
port def SensorPort {
    out item sensorData : SensorReading;
}

connection def SensorLink {
    end source : SensorPort;
    end target : ~SensorPort;
}
```

- [ ] **Step 5: Write parser tests with assert on tree structure**

- [ ] **Step 6: Commit**

```
feat(sysml2): parser for packages, parts, ports, connections
```

---

### Task B4: Parser — requirements + constraints

**Files:**
- Create: `crates/spar-sysml2/src/grammar/requirements.rs`
- Create: `crates/spar-sysml2/src/grammar/constraints.rs`

- [ ] **Step 1: Parse requirement def + satisfy + verify**

```sysml
requirement def LatencyReq {
    doc /* Sensor-to-actuator latency must be < 20ms */
    attribute maxLatency : Real = 20.0;
}

requirement sensorLatency : LatencyReq {
    subject sensor : SensorSubsystem;
    satisfy by ecu.controller;
    verify by latencyTest;
}
```

- [ ] **Step 2: Parse constraint def**

```sysml
constraint def TimingBudget {
    attribute totalLatency : Real;
    attribute bound : Real;
    totalLatency <= bound;
}
```

- [ ] **Step 3: Tests**

- [ ] **Step 4: Commit**

```
feat(sysml2): parser for requirements, constraints, satisfy/verify
```

---

### Task B5: Parser — actions + states + expressions

**Files:**
- Create: `crates/spar-sysml2/src/grammar/actions.rs`
- Create: `crates/spar-sysml2/src/grammar/expressions.rs`

- [ ] **Step 1: Parse action def + state def**

- [ ] **Step 2: Parse expressions (feature values, operators)**

- [ ] **Step 3: Commit**

```
feat(sysml2): parser for actions, states, expressions
```

---

### Task B6: SysML v2 → AADL lowering

**Files:**
- Create: `crates/spar-sysml2/src/lower.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn part_def_lowers_to_system_type() {
    let source = "package Pkg { part def Vehicle; }";
    let cst = parse(source);
    let item_tree = lower_to_aadl(&cst);
    assert_eq!(item_tree.component_types().count(), 1);
}
```

- [ ] **Step 2: Implement lowering per SEI mapping**

Walk the SysML v2 CST. Map:
- `part def` (hardware stereotype) → AADL system type
- `part def` (software stereotype) → AADL process/thread type
- `port def` → AADL data port / event data port
- `connection def` → AADL connection
- `constraint def` (timing) → AADL timing properties
- `allocate` → AADL binding properties

- [ ] **Step 3: Commit**

```
feat(sysml2): SysML v2 → AADL lowering (SEI mapping)
```

---

### Task B7: Requirements extraction to rivet

**Files:**
- Create: `crates/spar-sysml2/src/extract.rs`

- [ ] **Step 1: Extract requirement def → rivet YAML**

Walk CST, find all `requirement def` nodes, emit rivet requirement artifacts with `satisfy` → `satisfies` and `verify` → `verifies` link mapping.

- [ ] **Step 2: Commit**

```
feat(sysml2): requirements extraction to rivet YAML
```

---

### Task B8: CLI integration (`spar sysml2`)

**Files:**
- Modify: `crates/spar-cli/src/main.rs`
- Modify: `crates/spar-cli/Cargo.toml`

- [ ] **Step 1: Add sysml2 subcommand**

```rust
"sysml2" => cmd_sysml2(&args[2..]),
```

With sub-subcommands: `parse`, `lower`, `extract`, `analyze`.

- [ ] **Step 2: Commit**

```
feat(cli): spar sysml2 command (parse, lower, extract)
```

---

## Track C: Exact Solver (parallel with A + B)

### Task C1: good_lp MILP integration

**Files:**
- Modify: `crates/spar-solver/Cargo.toml` — add `good_lp` dep
- Create: `crates/spar-solver/src/milp.rs`

- [ ] **Step 1: Formulate deployment as MILP**

Binary variables: `bind[thread][processor]`. Constraints: capacity, schedulability, anti-colocation. Objective: minimize utilization variance.

- [ ] **Step 2: Solve with HiGHS backend**

- [ ] **Step 3: Return optimality certificate (dual bound)**

- [ ] **Step 4: Commit**

```
feat(solver): MILP deployment optimization with optimality certificates
```

---

### Task C2: NSGA-II Pareto front (pure Rust)

**Files:**
- Create: `crates/spar-solver/src/nsga2.rs`

- [ ] **Step 1: Implement NSGA-II**

Pure Rust, no deps, WASM-compatible. Multi-objective: minimize latency, minimize cost, maximize safety margins.

- [ ] **Step 2: Return Pareto front**

- [ ] **Step 3: Commit**

```
feat(solver): NSGA-II multi-objective Pareto front computation
```

---

## Integration Phase

### Task I1: Full roundtrip test

- [ ] **Step 1: Create test SysML v2 model**
- [ ] **Step 2: Extract requirements → rivet**
- [ ] **Step 3: Lower → AADL**
- [ ] **Step 4: Analyze AADL**
- [ ] **Step 5: Generate code**
- [ ] **Step 6: Verify traceability chain in rivet**
- [ ] **Step 7: Commit**

```
test: full roundtrip SysML v2 → AADL → Rust → verification → rivet
```
