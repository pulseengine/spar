# Rendering Quality & CI Upgrade Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade etch rendering engine with port-aware layout, orthogonal edge routing, and interactive HTML output; upgrade spar CI to match rivet's quality gates.

**Architecture:** etch (in sdlc/rivet repo) is the shared layout+rendering crate. spar-render bridges AADL instance models to etch. Changes flow: etch data model -> etch layout -> etch SVG/HTML rendering -> spar-render bridge -> spar-cli. CI is a parallel track.

**Tech Stack:** Rust, petgraph, etch crate, embedded JavaScript (no deps), Playwright for visual testing, GitHub Actions.

**Spec:** `docs/plans/2026-03-17-rendering-quality-design.md`

**Repos:**
- etch: `/Volumes/Home/git/sdlc/etch/` (branch off `feat/compound-layout`)
- spar: `/Volumes/Home/git/pulseengine/spar/` (branch off `main`)

**Cross-project workflow:** Tasks 1-8 modify etch in the sdlc/rivet repo. After Task 8, push the etch branch, create a PR, merge it. Then update spar's etch git rev in Task 9. Tasks 10-13 are spar-only.

**Port ID scope:** Port IDs are unique within a node. Edge `source_port`/`target_port` are resolved relative to the source/target node (no global uniqueness needed). spar-render generates IDs as `{feature_name}` (unique within a component).

---

## Chunk 1: Layout Determinism + Port Data Model

### Task 1: Audit and fix HashMap non-determinism in etch layout

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/layout.rs`

This task addresses RENDER-REQ-004.

- [ ] **Step 1: Identify order-sensitive HashMap usages**

In `layout.rs`, these HashMaps affect output order:
- `infos: HashMap<NodeIndex, NodeInfo>` -- iterated to detect compound graphs and build children maps
- `ranks: HashMap<NodeIndex, usize>` -- iterated to build rank lists
- `children_of: HashMap<NodeIndex, Vec<NodeIndex>>` -- iterated for container layout
- `idx_to_id: HashMap<NodeIndex, String>` -- used for edge routing lookup (order-independent)

`NodeIndex` is a `u32` wrapper with `Ord`, so `BTreeMap` works. Most iteration is already sorted after collection, but `children_of` and `container_depths` iterate without sorting.

- [ ] **Step 2: Write determinism test (flat layout)**

```rust
#[test]
fn layout_is_deterministic() {
    let mut g = Graph::new();
    let a = g.add_node("A");
    let b = g.add_node("B");
    let c = g.add_node("C");
    let d = g.add_node("D");
    let e = g.add_node("E");
    g.add_edge(a, b, "ab");
    g.add_edge(a, c, "ac");
    g.add_edge(b, d, "bd");
    g.add_edge(c, d, "cd");
    g.add_edge(d, e, "de");

    let opts = LayoutOptions::default();
    let first = layout(&g, &simple_node_info, &simple_edge_info, &opts);

    for _ in 0..10 {
        let result = layout(&g, &simple_node_info, &simple_edge_info, &opts);
        assert_eq!(first.nodes.len(), result.nodes.len());
        for (a, b) in first.nodes.iter().zip(result.nodes.iter()) {
            assert_eq!(a.id, b.id);
            assert!((a.x - b.x).abs() < 0.001, "x mismatch for {}", a.id);
            assert!((a.y - b.y).abs() < 0.001, "y mismatch for {}", a.id);
        }
    }
}
```

- [ ] **Step 3: Run test, check if it already passes**

Run: `cargo test -p etch layout_is_deterministic`

- [ ] **Step 4: Write compound layout determinism test**

```rust
#[test]
fn compound_layout_is_deterministic() {
    let mut g = Graph::new();
    let _s = g.add_node("S");
    let a = g.add_node("A");
    let b = g.add_node("B");
    let c = g.add_node("C");
    g.add_edge(a, b, "ab");
    g.add_edge(b, c, "bc");

    let node_info = |_idx: NodeIndex, n: &&str| NodeInfo {
        id: n.to_string(), label: n.to_string(), node_type: "default".into(),
        sublabel: None,
        parent: if *n != "S" { Some("S".into()) } else { None },
    };

    let first = layout(&g, &node_info, &simple_edge_info, &LayoutOptions::default());

    for _ in 0..10 {
        let result = layout(&g, &node_info, &simple_edge_info, &LayoutOptions::default());
        for (a, b) in first.nodes.iter().zip(result.nodes.iter()) {
            assert_eq!(a.id, b.id);
            assert!((a.x - b.x).abs() < 0.001, "x mismatch for {}", a.id);
            assert!((a.y - b.y).abs() < 0.001, "y mismatch for {}", a.id);
        }
    }
}
```

- [ ] **Step 5: Fix any non-determinism found, run all etch tests**

Run: `cargo test -p etch`

- [ ] **Step 6: Commit**

```bash
git add etch/src/layout.rs
git commit -m "feat(etch): add layout determinism tests (RENDER-REQ-004)"
```

---

### Task 2: Add port data model to etch

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/layout.rs` -- add PortInfo, PortSide, PortDirection, PortType; extend NodeInfo, EdgeInfo, LayoutNode, LayoutEdge
- Modify: `/Volumes/Home/git/sdlc/etch/src/lib.rs` -- update doctest, re-export new types
- Modify: `/Volumes/Home/git/sdlc/etch/src/svg.rs` -- update test helpers (add `ports: vec![]`, `source_port: None, target_port: None`)
- Modify: `/Volumes/Home/git/sdlc/rivet-cli/src/serve.rs` -- add `ports: vec![]` to NodeInfo constructors (3 call sites around lines 3204, 3405, 6502), add `source_port: None, target_port: None` to EdgeInfo constructors

- [ ] **Step 1: Write test for port-aware NodeInfo construction** (in layout.rs tests)
- [ ] **Step 2: Run test to verify it fails**
- [ ] **Step 3: Add port types (PortSide, PortDirection, PortType, PortInfo enums/struct) and extend NodeInfo, EdgeInfo, LayoutNode, LayoutEdge**

Key: `NodeInfo::ports` is `Vec<PortInfo>` (empty = backward compat). `EdgeInfo` gains `source_port: Option<String>` and `target_port: Option<String>`.

- [ ] **Step 4: Update ALL existing constructors across the codebase**

Files to update:
- `etch/src/layout.rs` tests: `simple_node_info` (add `ports: vec![]`), `simple_edge_info` (add `source_port: None, target_port: None`), all compound test closures
- `etch/src/svg.rs` tests: `build_test_layout` closure and `svg_compound_container_rendering` closure
- `etch/src/lib.rs` doctest: add `ports: vec![]` and `source_port: None, target_port: None`
- `rivet-cli/src/serve.rs`: 3 NodeInfo constructors + EdgeInfo constructors

- [ ] **Step 5: Pass port info through layout engine** -- `LayoutNode::ports` = empty vec initially, `LayoutEdge` copies port refs from EdgeInfo
- [ ] **Step 6: Run all tests** (`cargo test -p etch` and `cargo check -p rivet-cli`)
- [ ] **Step 7: Update lib.rs re-exports**
- [ ] **Step 8: Commit**

```bash
git add etch/src/layout.rs etch/src/lib.rs etch/src/svg.rs rivet-cli/src/serve.rs
git commit -m "feat(etch): add port data model (PortInfo, PortSide, PortType) — RENDER-REQ-002"
```

---

### Task 3: Port positioning and edge-to-port snapping

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/layout.rs`

- [ ] **Step 1: Write test for port position computation** (`ports_positioned_on_node_sides`)
- [ ] **Step 2: Run test, verify it fails**
- [ ] **Step 3: Implement `position_ports()` function**

Resolves `PortSide::Auto` based on direction: `In` -> Left, `Out` -> Right, `InOut` -> Right.

- [ ] **Step 4: Adjust node height for port count**

Port height uses resolved sides (after Auto resolution), not raw side values:

```rust
fn resolved_side_counts(ports: &[PortInfo]) -> (usize, usize) {
    let mut left = 0usize;
    let mut right = 0usize;
    for p in ports {
        match p.side {
            PortSide::Left => left += 1,
            PortSide::Right => right += 1,
            PortSide::Auto => match p.direction {
                PortDirection::In => left += 1,
                PortDirection::Out | PortDirection::InOut => right += 1,
            },
            _ => {} // Top/Bottom don't affect height
        }
    }
    (left, right)
}
```

- [ ] **Step 5: Write test for edge snapping to ports** (`edge_connects_to_ports`)
- [ ] **Step 6: Implement edge-to-port snapping in `compute_waypoints()`** -- use port coordinates when `source_port`/`target_port` are set
- [ ] **Step 7: Write test for container port routing** (`container_port_bridges_boundary`) -- edge from outside container to port on container border, then internal routing to child
- [ ] **Step 8: Run all tests, commit**

```bash
git commit -m "feat(etch): port positioning and edge-to-port snapping — RENDER-REQ-002"
```

---

### Task 4: Port rendering in SVG

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/svg.rs`

- [ ] **Step 1: Write test for port SVG output** (`svg_renders_ports`)
- [ ] **Step 2: Run test, verify it fails**
- [ ] **Step 3: Implement port rendering in `write_nodes()`** -- circle (r=3), direction triangle, label text, CSS classes per port type
- [ ] **Step 4: Run tests, commit**

```bash
git commit -m "feat(etch): SVG port rendering with type colors — RENDER-REQ-002"
```

---

## Chunk 2: Orthogonal Edge Routing

### Task 5: Add EdgeRouting enum and config

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/layout.rs`

- [ ] **Step 1: Add `EdgeRouting` enum (`Orthogonal`, `CubicBezier`) and new `LayoutOptions` fields**
- [ ] **Step 2: Update Default impl; `Orthogonal` falls back to `CubicBezier` temporarily**
- [ ] **Step 3: Run tests, commit**

```bash
git commit -m "feat(etch): add EdgeRouting enum and config — RENDER-REQ-001"
```

---

### Task 6: Implement orthogonal edge router

**Files:**
- Create: `/Volumes/Home/git/sdlc/etch/src/ortho.rs`
- Modify: `/Volumes/Home/git/sdlc/etch/src/layout.rs`
- Modify: `/Volumes/Home/git/sdlc/etch/src/lib.rs`

- [ ] **Step 1: Write test for basic orthogonal routing** (`orthogonal_route_simple_chain`) -- assert all segments are axis-aligned
- [ ] **Step 2: Run test, verify it fails**
- [ ] **Step 3: Implement visibility-graph orthogonal router in `ortho.rs`**

Exports: `pub fn route_orthogonal(nodes: &[LayoutNode], source: (f64, f64), target: (f64, f64), options: &LayoutOptions) -> Vec<(f64, f64)>`

- [ ] **Step 4: Wire into `layout.rs` `route_edges()`**
- [ ] **Step 5: Write obstacle avoidance test** (`orthogonal_route_avoids_obstacles`)
- [ ] **Step 6: Write self-loop test** (`orthogonal_self_loop`) -- edge from a port back to another port on same node, routes as rectangular loop
- [ ] **Step 7: Write multi-edge test** (`orthogonal_multi_edges_separated`) -- two edges between same pair nudged apart
- [ ] **Step 8: Run all tests, commit**

```bash
git add etch/src/ortho.rs etch/src/layout.rs etch/src/lib.rs
git commit -m "feat(etch): orthogonal edge routing — RENDER-REQ-001"
```

---

### Task 7: Update SVG renderer for orthogonal edges

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/svg.rs`

- [ ] **Step 1: Write test for orthogonal SVG path** (`svg_orthogonal_edges_use_line_commands`) -- verify path uses `L` not `C` commands
- [ ] **Step 2: Run test, verify it fails**
- [ ] **Step 3: Update `build_bezier_path()` to detect axis-aligned segments and emit `L` commands**
- [ ] **Step 4: Run tests, commit**

```bash
git commit -m "feat(etch): SVG polyline rendering for orthogonal edges"
```

---

## Chunk 3: Interactive HTML

### Task 8: Create etch::html module (Phase 3a: pan/zoom/selection)

**Files:**
- Create: `/Volumes/Home/git/sdlc/etch/src/html.rs`
- Create: `/Volumes/Home/git/sdlc/etch/src/html_interactivity.js`
- Modify: `/Volumes/Home/git/sdlc/etch/src/lib.rs`

- [ ] **Step 1: Write test for HTML output structure** (`html_contains_svg_and_script`)
- [ ] **Step 2: Run test, verify it fails**
- [ ] **Step 3: Implement `HtmlOptions` struct and `render_html()` function**

Wraps SVG in self-contained HTML with embedded JS via `include_str!("html_interactivity.js")`.

- [ ] **Step 4: Create `html_interactivity.js`** -- pan (mousedown+mousemove), zoom (wheel, viewBox scaling), selection (click `.node`, toggle `.selected`, emit CustomEvent), group highlight (click `.node.container`)
- [ ] **Step 5: Run tests, commit**

```bash
git add etch/src/html.rs etch/src/html_interactivity.js etch/src/lib.rs
git commit -m "feat(etch): interactive HTML wrapper with pan/zoom/selection — RENDER-REQ-003, RENDER-REQ-005"
```

---

### Task 8b: Enhanced navigation (Phase 3b: minimap/search/semantic zoom/legend)

**Files:**
- Modify: `/Volumes/Home/git/sdlc/etch/src/html.rs`
- Modify: `/Volumes/Home/git/sdlc/etch/src/html_interactivity.js`

- [ ] **Step 1: Write test for minimap presence** (`html_has_minimap`)
- [ ] **Step 2: Write test for search** (`html_has_search`)
- [ ] **Step 3: Implement minimap** -- cloned small SVG in corner div, viewport rectangle, click-to-navigate
- [ ] **Step 4: Implement search** -- Ctrl+F floating input, filter nodes by label/id, pan to match
- [ ] **Step 5: Implement semantic zoom** -- at zoom < 50% add `.zoom-low` class to SVG (CSS hides `.sublabel`, `.port text`), at zoom < 25% add `.zoom-overview` (CSS hides `.edge text`)
- [ ] **Step 6: Implement legend** -- auto-generated from distinct `node_type` values, collapsible panel
- [ ] **Step 7: Run tests, commit**

```bash
git commit -m "feat(etch): minimap, search, semantic zoom, legend — RENDER-REQ-003, RENDER-REQ-006"
```

---

### Task 8c: Push etch changes, create PR, merge

- [ ] **Step 1: Run full etch test suite** (`cargo test -p etch`)
- [ ] **Step 2: Run clippy and fmt** (`rustup run nightly cargo fmt --all`, `rustup run stable cargo clippy --workspace --all-targets -- -D warnings`)
- [ ] **Step 3: Push branch, create PR in rivet repo**
- [ ] **Step 4: Wait for CI, merge**
- [ ] **Step 5: Note the merge commit SHA for spar's etch dependency**

---

## Chunk 4: spar Integration + CI + Artifacts

### Task 9: Wire spar-render and spar-cli to ports + HTML

**Files:**
- Modify: `/Volumes/Home/git/pulseengine/spar/Cargo.toml` -- update etch rev
- Modify: `/Volumes/Home/git/pulseengine/spar/crates/spar-render/src/lib.rs`
- Modify: `/Volumes/Home/git/pulseengine/spar/crates/spar-cli/src/main.rs`

- [ ] **Step 1: Update etch git rev in workspace Cargo.toml**
- [ ] **Step 2: Add `feature_to_port()` mapping in spar-render**

Maps `FeatureInstance` to `PortInfo` using FeatureKind-to-PortType table from spec. Port ID = `feature.name.to_string()` (unique within component).

- [ ] **Step 3: Update `build_graph()` to populate ports on NodeInfo and port refs on EdgeInfo**
- [ ] **Step 4: Add `render_instance_html()` function**
- [ ] **Step 5: Add `--format html|svg` to CLI render command**
- [ ] **Step 6: Write tests for feature-to-port mapping**
- [ ] **Step 7: Run all spar tests, commit**

```bash
git commit -m "feat(spar-render): port-aware rendering with HTML output — RENDER-REQ-002, RENDER-REQ-003"
```

---

### Task 10: Upgrade spar CI workflow

**Files:**
- Modify: `/Volumes/Home/git/pulseengine/spar/.github/workflows/ci.yml`

- [ ] **Step 1: Add Miri job** (`cargo miri test -p spar-hir-def --lib`)
- [ ] **Step 2: Add proptest job** (`PROPTEST_CASES=1000 cargo test`)
- [ ] **Step 3: Add mutation testing job** (`cargo mutants -p spar-analysis --timeout 120 --jobs 4`, `continue-on-error: true`)
- [ ] **Step 4: Add MSRV job** (pin to minimum Rust version, `cargo check --all`)
- [ ] **Step 5: Add cargo-vet job** (supply chain verification)
- [ ] **Step 6: Commit, push via PR**

```bash
git commit -m "feat(ci): add Miri, proptest, mutation testing, MSRV, cargo-vet"
```

---

### Task 11: Create rendering STPA artifacts

**Files:**
- Create: `/Volumes/Home/git/pulseengine/spar/safety/stpa/rendering-analysis.yaml`
- Modify: `/Volumes/Home/git/pulseengine/spar/artifacts/architecture.yaml`
- Modify: `/Volumes/Home/git/pulseengine/spar/artifacts/verification.yaml`

- [ ] **Step 1: Create rendering-analysis.yaml** -- losses L-R1..4, hazards H-R1..5, requirements RENDER-REQ-001..006
- [ ] **Step 2: Add ARCH decisions** -- ARCH-R1 (port layout), ARCH-R2 (orthogonal routing), ARCH-R3 (interactive HTML), ARCH-R4 (GLSP separation)
- [ ] **Step 3: Add VAL records** linking tests to RENDER-REQ-*
- [ ] **Step 4: Run `rivet validate`, commit**

```bash
git commit -m "docs(stpa): rendering quality STPA analysis and architecture decisions"
```

---

### Task 12: Playwright visual regression tests

**Files:**
- Create: `/Volumes/Home/git/pulseengine/spar/tests/playwright/rendering.spec.ts`

- [ ] **Step 1: Set up Playwright config**
- [ ] **Step 2: Write tests:**
  - Render AADL model to HTML via spar CLI
  - Verify ports visible (`.port` elements present)
  - Verify edges orthogonal (SVG path uses `L` commands)
  - Verify pan/zoom (scroll, check viewBox changes)
  - Verify selection (click node, check `.selected` class)
  - Verify semantic zoom (zoom out, check `.zoom-low` class)
  - Verify minimap (check `#minimap` element)
  - Verify search (Ctrl+F, type query, check node highlighted)
- [ ] **Step 3: Run Playwright tests, commit**

```bash
git commit -m "test: Playwright visual regression tests — RENDER-REQ-001..006"
```
