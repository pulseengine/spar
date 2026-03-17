# Rendering Quality & CI Upgrade Design

**Date:** 2026-03-17
**Status:** Approved
**Scope:** etch crate (rendering engine), spar-render (AADL bridge), spar-cli, spar CI

## Problem

The etch rendering engine produces functional but subpar architecture diagrams compared to professional tools (ELK, yFiles, OSATE). Three critical gaps make diagrams unsuitable for engineering review of large AADL models:

1. **No port awareness** -- edges connect to node centers, hiding which features a connection uses
2. **No orthogonal routing** -- bezier curves become spaghetti in dense graphs
3. **No interactivity** -- static SVGs are unusable for models with 50+ components

Additionally, spar's CI quality gates lag behind rivet's (missing Miri, proptest, mutation testing, fuzz testing, MSRV, cargo-vet).

## STPA Loss Analysis

STPA IDs use L-R/H-R/RENDER-REQ prefix to distinguish from the main spar STPA (L-1/H-1/STPA-REQ). These are tracked in a separate artifact file `safety/stpa/rendering-analysis.yaml`.

### Losses

| ID | Loss |
|----|------|
| L-R1 | Engineer misreads architecture (wrong connection, missed component) leading to design error |
| L-R2 | Engineer cannot find relevant component in large model, wasting time or missing review deadline |
| L-R3 | Engineer cannot distinguish port connections, causing incorrect integration or runtime failure |
| L-R4 | Stakeholder review fails because diagram is unreadable, delaying certification |

### Hazards

| ID | Hazard | Causes |
|----|--------|--------|
| H-R1 | Edge spaghetti obscures connection topology | L-R1, L-R3 |
| H-R2 | No port visibility -- connections appear identical regardless of feature | L-R3, L-R4 |
| H-R3 | Cannot navigate large models -- no zoom/search/filter | L-R2 |
| H-R4 | Layout instability -- small model change causes full re-layout | L-R1, L-R2 |
| H-R5 | No selection/highlighting -- cannot isolate subsystem for review | L-R2, L-R4 |

### Safety Requirements

| ID | Requirement | Mitigates |
|----|-------------|-----------|
| RENDER-REQ-001 | Edges must use orthogonal routing to minimize visual crossings | H-R1 |
| RENDER-REQ-002 | Ports must be visible with directional indicators and type coloring | H-R2 |
| RENDER-REQ-003 | Interactive HTML must support zoom/pan/minimap/search | H-R3 |
| RENDER-REQ-004 | Layout must be deterministic -- same model always produces same layout | H-R4 |
| RENDER-REQ-005 | Selection and group highlighting must be supported | H-R5 |
| RENDER-REQ-006 | Semantic zoom must reduce clutter at overview levels | H-R3 |

## Approach

**Incremental etch upgrades (Approach A):** Upgrade etch itself with port-aware layout, orthogonal routing, and interactive HTML wrapper. Everything stays in Rust. The embedded JavaScript for interactivity is emitted as part of a self-contained HTML document. No external dependencies.

Rejected alternatives:
- **ELK.js integration** -- ~700KB WASM dependency, loss of control over layout internals
- **Browser rendering framework** -- two rendering paths (SVG for CLI, Canvas/WebGL for browser), significantly more frontend code

## Design

### Phase 0: Layout Determinism (Prerequisite)

Before any visual changes, ensure RENDER-REQ-004: same input always produces same output.

**Problem:** etch uses `HashMap` extensively in layout code. HashMap iteration order is non-deterministic in Rust (randomized hashing). This can cause layout jitter between runs.

**Fix:**
- Audit all HashMap usage in etch's layout for order-sensitive paths
- Replace order-sensitive HashMaps with `BTreeMap` or `IndexMap` where iteration order affects output
- Add proptest: generate random graphs, assert layout equality across multiple runs
- This is a prerequisite for Phase 1 because visual regression tests (Playwright screenshots) require stable layouts

### Phase 1: Port-Aware Layout

**Data model additions to etch:**

```rust
pub struct PortInfo {
    pub id: String,
    pub label: String,
    pub side: PortSide,
    pub direction: PortDirection,
    pub port_type: PortType,
}

pub enum PortSide {
    Left,
    Right,
    Top,
    Bottom,
    Auto,  // let layout algorithm choose optimal side
}

pub enum PortDirection { In, Out, InOut }

pub enum PortType {
    Data,       // blue (#4a90d9) -- AADL DataPort, Parameter
    Event,      // orange (#e67e22) -- AADL EventPort
    EventData,  // green (#27ae60) -- AADL EventDataPort
    Access,     // gray (#999) -- AADL DataAccess, BusAccess, SubprogramAccess, SubprogramGroupAccess
    Group,      // purple (#9b59b6) -- AADL FeatureGroup
    Abstract,   // dark gray (#666) -- AADL AbstractFeature
}
```

**NodeInfo change (backward-compatible):**
```rust
pub struct NodeInfo {
    // ...existing fields unchanged...
    pub ports: Vec<PortInfo>,  // NEW -- defaults to empty vec
}
```

`ports` defaults to an empty `Vec`. Existing callers (rivet) that don't use ports continue working unchanged -- edges connect to node centers when no ports are specified. This avoids a breaking API change.

**EdgeInfo change (port-aware edges):**
```rust
pub struct EdgeInfo {
    pub label: String,
    pub source_port: Option<String>,  // NEW -- port ID on source node
    pub target_port: Option<String>,  // NEW -- port ID on target node
}
```

When `source_port`/`target_port` are `None`, edges connect to node centers (backward compat). When set, edges snap to the specified port position.

**LayoutEdge output:**
```rust
pub struct LayoutEdge {
    // ...existing fields...
    pub source_port: Option<String>,  // NEW -- resolved port ID
    pub target_port: Option<String>,  // NEW -- resolved port ID
}
```

**Layout changes:**
- Ports positioned along their designated side, evenly spaced
- Node height grows to accommodate port count: `max(default_height, port_count * port_spacing)`
- Edge endpoints snap to specific ports instead of node centers
- `PortSide::Auto` resolved by layout: inputs default to left (or top in TB layout), outputs to right (or bottom)
- Coordinate assignment accounts for port positions when computing waypoints

**Container node ports:**
- Container nodes can have ports on their outer border
- Connections crossing container boundaries route through the container's port
- Internal connections from a container port to a child node's port are routed inside the container
- This mirrors AADL semantics where a system's ports bridge internal and external connections

**SVG rendering:**
- Small circles (6px diameter) on node borders at port positions
- Port label text alongside each circle
- Color by `PortType` enum (see mapping above)
- Directional indicator: input ports get an inward-pointing triangle, output ports get an outward-pointing triangle
- Edges connect to port circles, not node rectangles

**spar-render bridge -- FeatureKind to PortType mapping:**

| AADL FeatureKind | PortType | Color |
|------------------|----------|-------|
| DataPort | Data | blue |
| EventPort | Event | orange |
| EventDataPort | EventData | green |
| Parameter | Data | blue |
| DataAccess | Access | gray |
| BusAccess | Access | gray |
| SubprogramAccess | Access | gray |
| SubprogramGroupAccess | Access | gray |
| FeatureGroup | Group | purple |
| AbstractFeature | Abstract | dark gray |

**Port side assignment in spar-render:**
- `Direction::In` -> `PortSide::Left` (or `Top` in TB layout)
- `Direction::Out` -> `PortSide::Right` (or `Bottom` in TB layout)
- `Direction::InOut` -> `PortSide::Left` (convention: treat as input side)
- No direction -> `PortSide::Auto`

**Inspiration from GLSP (Eclipse Graphical Language Server Protocol):**
- Separation of layout computation from rendering, analogous to LSP for languages
- etch computes layout and emits a structured `GraphLayout` (the "model"); rendering is a separate concern
- This enables multiple renderers (SVG, HTML+JS, future Canvas/WebGL) from the same layout result

### Phase 2: Orthogonal Edge Routing

**Algorithm:** Visibility graph-based orthogonal router.

1. **Build obstacle set** -- all node rectangles (with padding) become obstacles
2. **Create visibility graph** -- horizontal and vertical line segments connecting port positions via bend points, avoiding obstacles
3. **Shortest path** -- Dijkstra on the visibility graph with cost = segment_length + bend_penalty
4. **Nudging** -- post-process to separate parallel edge segments that overlap

**Configuration:**

```rust
pub enum EdgeRouting {
    Orthogonal,    // right-angle bends (new default)
    CubicBezier,   // current behavior (vertical-tangent cubic bezier)
}

pub struct LayoutOptions {
    // ...existing fields...
    pub edge_routing: EdgeRouting,     // NEW, default Orthogonal
    pub bend_penalty: f64,            // NEW, default 20.0
    pub edge_separation: f64,         // NEW, default 4.0
    pub port_stub_length: f64,        // NEW, default 10.0
}
```

**Key details:**
- Bend penalty tunable -- higher = fewer bends, longer paths
- Edges leaving ports always start with a straight stub (`port_stub_length`) perpendicular to the node border before any bends
- Parallel edge separation prevents overlapping segments
- Container edges route through container border ports (see Phase 1 container port design)
- Self-loops route as a small rectangular loop on the right side of the node
- Multi-edges between the same node pair are nudged apart

**Performance target:** Layout completes in <1s for models with up to 200 nodes. For larger models, consider spatial indexing (R-tree) for obstacle queries.

### Phase 3: Interactive HTML Wrapper

Split into two sub-phases for earlier delivery of core navigation.

**Phase 3a: Core Navigation (must-haves)**

New module: `etch::html`

```rust
pub fn render_html(
    layout: &GraphLayout,
    svg_options: &SvgOptions,
    html_options: &HtmlOptions,
) -> String

pub struct HtmlOptions {
    pub title: String,
    pub minimap: bool,          // default true
    pub search: bool,           // default true
    pub legend: bool,           // default true
    pub semantic_zoom: bool,    // default true
}
```

Features:
- **Pan/zoom** -- mouse wheel zoom, click-drag pan, pinch-zoom on touch. SVG viewBox manipulation for crisp rendering at all zoom levels
- **Selection** -- click node to select (highlighted border), Ctrl+click for multi-select. Selected nodes emit a `CustomEvent("etch-select", { ids: [...] })` for integration
- **Group highlighting** -- click container to highlight children. URL parameter `?highlight=ID` for deep linking

**Phase 3b: Enhanced Navigation (follow-up)**

- **Minimap** -- small overview in bottom-right, viewport rectangle, clickable
- **Search** -- Ctrl+F floating search box, filters by label/id, pans to match
- **Semantic zoom** -- at zoom < 50% hide sublabels and port labels, at zoom < 25% hide edge labels
- **Legend** -- auto-generated from node types, collapsible panel

**CLI integration:**

`spar-render` gains a new function:
```rust
pub fn render_instance_html(
    instance: &SystemInstance,
    render_options: &RenderOptions,
    html_options: &etch::html::HtmlOptions,
) -> String
```

`spar-cli` gains `--format` on the render command:
```
spar render --root Pkg::Type.Impl --format svg  -o diagram.svg  *.aadl  (default)
spar render --root Pkg::Type.Impl --format html -o diagram.html *.aadl
```

spar-wasm does not need HTML output -- the WASM component runs inside rivet's dashboard which provides its own navigation.

**Accessibility (future consideration):**
- Keyboard navigation (Tab through nodes, Enter to select)
- ARIA labels on SVG elements (`role="img"`, `aria-label`)
- High-contrast mode compatibility

### Phase 4: CI Quality Gate Upgrade (Parallel Track)

Upgrade `.github/workflows/ci.yml` to match rivet:

| Gate | Implementation |
|------|---------------|
| Miri | `cargo miri test -p etch -p spar-hir-def --lib` (UB detection) |
| Proptest | `PROPTEST_CASES=1000 cargo test` (extended property testing) |
| Mutation testing | `cargo mutants -p spar-analysis --timeout 120 --jobs 4` |
| Fuzz testing | `cargo fuzz run` targets (main-only, 30s each) |
| MSRV | `cargo check --all` with pinned minimum Rust version |
| cargo-vet | Supply chain verification |

Miri targets both etch (new layout algorithms) and spar-hir-def (arena-indexed instance model).

No code changes needed -- purely workflow configuration.

## Testing Strategy

**Unit tests (Rust):**
- Port positioning calculations (side assignment, spacing, Auto resolution)
- Orthogonal routing path correctness (obstacle avoidance, bend count)
- Edge separation (nudging of parallel segments)
- Self-loop and multi-edge routing
- Layout determinism: proptest generating random graphs, asserting equality across runs
- HTML generation (contains expected elements: SVG, script, controls)
- Container port routing (edges crossing container boundaries)

**Visual regression tests (Playwright):**
- Render test AADL models to HTML
- Screenshot comparison for layout quality
- Verify pan/zoom interactions (scroll to zoom, drag to pan)
- Verify selection (click node, verify highlight)
- Verify group highlighting (click container, children highlighted)
- Phase 3b: verify semantic zoom, search, minimap

**STPA traceability:**
- Each Playwright test mapped to a RENDER-REQ-* in verification.yaml
- Test evidence included in release bundles

## Rivet Artifact Tracing

All design decisions tracked as rivet artifacts:
- `safety/stpa/rendering-analysis.yaml` -- losses (L-R*), hazards (H-R*), requirements (RENDER-REQ-*) from STPA above (separate file from main STPA)
- `artifacts/architecture.yaml` -- ARCH decisions for port layout, orthogonal routing, interactive HTML, GLSP-inspired separation
- `artifacts/verification.yaml` -- VAL records linking Playwright tests to RENDER-REQ-*

## Implementation Order

0. Layout determinism audit (prerequisite, enables visual regression testing)
1. Port-aware layout (etch core) -- unlocks meaningful AADL diagrams
2. Orthogonal edge routing (etch core) -- eliminates spaghetti
3a. Interactive HTML wrapper: pan/zoom/selection (etch html.rs)
3b. Interactive HTML wrapper: minimap/search/semantic zoom/legend
4. CI quality gates (spar CI) -- independent, can run in parallel with 0-3
5. Rivet artifact tracing -- after implementation, link tests to requirements

## Cross-Project Impact

etch is a shared crate used by both rivet and spar. Changes to `NodeInfo` and `EdgeInfo` are designed to be backward-compatible:
- `ports: Vec<PortInfo>` defaults to empty vec -- existing rivet callers unaffected
- `source_port`/`target_port` on `EdgeInfo` are `Option<String>` -- `None` preserves current center-connect behavior
- rivet call sites in `rivet-cli/src/serve.rs` already set `parent: None` -- they will similarly pass empty ports and `None` port refs

## Notes

- rules_lean is now available and should be used instead of manual Lean/Mathlib for any future proof work
- Playwright is used both for visual regression testing and for guiding the user through interactive features during development
