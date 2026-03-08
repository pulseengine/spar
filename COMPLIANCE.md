# AS5506 AADL v2.2 Compliance Gap Analysis

**Generated**: 2026-03-08
**Source**: 102 HTML files from OSATE2 (`org.osate.help/html/std/`)
**Toolchain**: spar (394 tests passing across 8 crates)

---

## Executive Summary

| Layer | Status | Score |
|-------|--------|-------|
| **Parser (syntax)** | Excellent | ~95% — all major constructs parsed |
| **ItemTree (declaration capture)** | Partial | ~55% — many parsed constructs not lowered |
| **Name Resolution** | Partial | ~30% — basic resolution works, no validation |
| **Legality Rules (L-rules)** | Missing | ~0% — zero of ~200+ rules enforced |
| **Naming Rules (N-rules)** | Missing | ~2% — case-insensitivity only |
| **Instance Model** | Partial | ~35% — hierarchy works, no semantic connections |
| **Property System** | Partial | ~25% — inheritance works, no types/evaluation |
| **Modes** | Missing | ~5% — parsed only, not in HIR |
| **Predeclared Property Sets** | Partial | ~60% — 5/7 sets, subset of properties |

---

## Section-by-Section Status

### Chapter 1-3: Scope, Overview, Definitions — N/A (informative)

### Chapter 4: Packages & Namespaces

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 4.1 | AADL Specifications | DONE | DONE | PARTIAL — no duplicate name detection |
| 4.2 | Packages | DONE | PARTIAL | PARTIAL — `with` not enforced, `renames` not lowered, private visibility not enforced |
| 4.3 | Component Types | DONE | PARTIAL | MISSING — 0/9 N-rules, 0/6 L-rules |
| 4.4 | Component Impls | DONE | PARTIAL | MISSING — 0/9 N-rules, 0/11 L-rules |
| 4.5 | Subcomponents | DONE | PARTIAL | MISSING — 0/6 N-rules, 0/11 L-rules |
| 4.6 | Calls | DONE | MISSING | MISSING — not lowered to ItemTree |
| 4.7 | Prototypes | DONE | MISSING | MISSING — not lowered to ItemTree |
| 4.8 | Annexes | DONE | PARTIAL | PARTIAL — library only, subclauses dropped |

### Chapter 5: Software Components (data, subprogram, thread, process)

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 5.1 | Data | DONE | DONE | MISSING — feature/subcomponent restrictions not checked |
| 5.2 | Subprogram | DONE | DONE | MISSING — parameter restrictions not checked |
| 5.3 | Subprogram Group | DONE | DONE | MISSING |
| 5.4 | Thread | DONE | DONE | MISSING — dispatch protocol not required |
| 5.5 | Thread Group | DONE | DONE | MISSING |
| 5.6 | Process | DONE | DONE | MISSING — thread requirement not checked |

### Chapter 6: Execution Platform (processor, memory, bus, device)

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 6.1 | Processor | DONE | DONE | MISSING — feature restrictions not checked |
| 6.2 | Virtual Processor | DONE | DONE | MISSING |
| 6.3 | Memory | DONE | DONE | MISSING |
| 6.4 | Bus | DONE | DONE | MISSING |
| 6.5 | Virtual Bus | DONE | DONE | MISSING |
| 6.6 | Device | DONE | DONE | MISSING |
| 6.7 | System | DONE | DONE | MISSING |

### Chapter 7-8: Features

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 8.1 | Ports | DONE | PARTIAL | MISSING — direction rules not enforced |
| 8.2 | Port Groups | DONE | PARTIAL | MISSING — complement rules, expansion |
| 8.3 | Data Access | DONE | PARTIAL | MISSING — provides/requires not captured |
| 8.4 | Bus Access | DONE | PARTIAL | MISSING |
| 8.5 | Subprogram Access | DONE | PARTIAL | MISSING |
| 8.6 | Feature Groups | DONE | DONE | MISSING — inverse/complement validation |
| 8.7 | Abstract Features | DONE | DONE | MISSING |

### Chapter 9: Connections

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 9.1 | Overview | DONE | PARTIAL | MISSING — no feature array connections |
| 9.2 | Feature Groups | DONE | PARTIAL | MISSING — no FG expansion for connections |
| 9.3 | Port Connections | DONE | DONE | MISSING — 0/16 direction rules |
| 9.4 | Direction Rules | DONE | DONE | **MISSING — CRITICAL: zero direction enforcement** |
| 9.5 | Access Connections | DONE | PARTIAL | MISSING — provides/requires not tracked |
| 9.6 | Bus/Data/Subprogram Access | DONE | PARTIAL | MISSING — subcategories collapsed |
| 9.7 | Parameter Connections | DONE | DONE | MISSING — call context not validated |
| 9.8 | Connection Patterns | PARTIAL | MISSING | MISSING — array patterns not handled |

### Chapter 10: Flows

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 10.1 | Flow Specifications | DONE | PARTIAL | MISSING — endpoints not stored in FlowSpecItem |
| 10.2 | Flow Spec Legality | DONE | PARTIAL | MISSING — 0/6 L-rules |
| 10.3 | Flow Implementations | DONE | **MISSING** | MISSING — parsed then discarded |
| 10.4 | End-to-End Flows | DONE | DONE | PARTIAL — segments not typed (flow vs conn) |
| 10.5 | Flow Properties | N/A | MISSING | MISSING — no latency properties |
| 10.6 | Latency Analysis | N/A | N/A | MISSING — no analysis |

### Chapter 11: Properties

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 11.1 | Property Sets | DONE | PARTIAL | PARTIAL — no type/applies-to storage |
| 11.2 | Property Types | DONE | **MISSING** | **MISSING — no type representation in HIR** |
| 11.3 | Property Expressions | DONE | **MISSING** | **MISSING — values are opaque strings** |
| 11.4 | Property Associations | DONE | PARTIAL | PARTIAL — basic inheritance, 0/11 L-rules |

### Chapter 12: Modes

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 12.1 | Mode Overview | DONE | **MISSING** | **MISSING — parsed then discarded** |
| 12.2 | Mode Declarations | DONE | **MISSING** | MISSING — no initial/unique validation |
| 12.3 | Mode Transitions | DONE | **MISSING** | MISSING — no trigger validation |
| 12.4 | Modal Configurations | DONE | **MISSING** | MISSING — `in modes` not preserved |
| 12.5 | Requires Modes | DONE | **MISSING** | MISSING |

### Chapter 13: Execution & Binding

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 13.1 | System Binding | N/A | N/A | **MISSING — no binding validation** |

### Chapter 14: Instance Model

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 14.1 | System Instances | N/A | N/A | PARTIAL — hierarchy works |
| 14.2 | Semantic Connections | N/A | N/A | **MISSING — CRITICAL: no end-to-end tracing** |
| 14.3 | Binding Instances | N/A | N/A | MISSING |
| 14.4 | System Lifecycle | N/A | N/A | MISSING |
| 14.5 | Mode Instances/SOMs | N/A | N/A | **MISSING — no SOM computation** |
| 14.6 | System Operation Modes | N/A | N/A | MISSING |
| 14.7 | Mode Transition Semantics | N/A | N/A | MISSING |
| 14.8 | Tool Requirements | N/A | N/A | PARTIAL |

### Chapter 15: Lexical Elements

| Section | Title | Status |
|---------|-------|--------|
| 15.1 | Character Set | PARTIAL — ASCII only, no Unicode identifier_letter |
| 15.2 | Delimiters | DONE — all compound/single delimiters |
| 15.3 | Identifiers | DONE — minor: accepts `__` and trailing `_` |
| 15.4 | Numeric Literals | DONE — decimal, based, real, exponents |
| 15.5 | String Literals | PARTIAL — missing `""` doubled-quote escape |
| 15.6 | Comments | DONE |
| 15.7 | Reserved Words | DONE — all 76 keywords |

### Appendices

| Appendix | Title | Status |
|----------|-------|--------|
| A | Predeclared Property Sets | PARTIAL — 5/7 sets, subset of props each. Missing: Programming_Properties, Modeling_Properties, AADL_Project |
| B | Glossary | N/A (informative) |
| C | Syntax Summary | N/A (informative) |
| D | Graphical Notation | MISSING (normative but optional for textual tools) |
| E | XML/AAXL2 Meta Model | MISSING — no OSATE interop serialization |
| F | UML Profile | N/A |
| G | Profiles/Extensions | MISSING |

---

## Critical Gaps (Priority 1 — blocks standard compliance)

### G1: No Semantic Connection Instances (§14.2)
Connection instances are copied from declarative model, not traced end-to-end through hierarchy.
The standard requires resolving `sub_a.out -> sub_b.in` through intermediate components to find
the ultimate source/destination ports. OSATE calls these "connection instances."

### G2: No Port Direction Rule Enforcement (§9.3-9.4)
16+ legality rules for port direction (up/down/across connections, in/out matching).
Zero enforced. This is the most basic connection validation.

### G3: No Property Type System (§11.2-11.3)
Property values stored as opaque strings. Cannot validate types (aadlinteger, Time, enumeration),
cannot evaluate expressions, cannot do unit conversion. Blocks all property-based analyses.

### G4: Modes Not in HIR (§12)
Modes are parsed but entirely discarded during CST→ItemTree lowering.
No mode declarations, transitions, or `in modes` clauses preserved.
Blocks: modal property values, mode-specific connections, SOM computation.

### G5: No Legality Rule Engine
~200+ legality rules across the standard, zero implemented.
Need a dedicated validation pass over the ItemTree + instance model.

### G6: No Feature Group Expansion (§9.2)
Feature group connections must be expanded into individual port connections.
Required for connection validation and instance model completeness.

---

## Major Gaps (Priority 2 — significant missing areas)

### G7: ItemTree Information Loss
Parsed but not lowered to ItemTree:
- Prototypes and bindings
- Call sequences
- Flow implementations (only E2E flows kept)
- Mode declarations and transitions
- Array dimensions on subcomponents/features
- `in modes` clauses on connections/subcomponents/flows
- `provides`/`requires` on access features
- Property associations on features
- Annex subclauses on classifiers

### G8: No Naming Rule Validation
~100+ naming rules, only case-insensitivity implemented:
- No uniqueness checks for any identifier
- No `with` clause enforcement
- No end-name-matches-start-name validation
- No private section visibility enforcement
- No `renames` resolution

### G9: No Category-Specific Restrictions (§5-6)
Each of 14 component categories has rules about which features/subcomponents it allows.
None enforced. Parser accepts `bus access` on a `data` component.

### G10: No Flow Implementation Lowering (§10.3)
Flow implementations (mapping flow specs through subcomponents) are parsed then discarded.
FlowSpecItem lacks endpoint references.

### G11: No Binding Analysis (§13-14)
Standard binding properties exist in standard_properties.rs but no validation:
- Thread → processor binding
- Process → memory binding
- Connection → bus binding

### G12: Incomplete Predeclared Property Sets (Appendix A)
Missing: Programming_Properties, Modeling_Properties, AADL_Project.
Existing 5 sets are subsets of the full standard definitions.

---

## Actionable Work Packages

### WP1: Enrich ItemTree (addresses G7)
Add fields to existing ItemTree structs for: prototypes, calls, modes, mode transitions,
flow implementations, array dimensions, `in modes`, provides/requires, feature properties.
**Estimate**: Medium — data structure changes + lowering code.

### WP2: Legality Rule Engine (addresses G5, G8, G9)
Create a validation pass that walks the ItemTree and checks L-rules and N-rules.
Start with naming uniqueness, end-name matching, category restrictions.
**Estimate**: Large — ~200 rules to implement.

### WP3: Property Type System (addresses G3)
Replace opaque string values with typed AST: `PropertyExpr` enum with
Integer, Real, String, Boolean, Enum, List, Record, Range, Classifier, Reference, Compute.
Add type definitions to PropertySetItem.
**Estimate**: Large — fundamental data model change.

### WP4: Mode Support (addresses G4)
Add ModeItem, ModeTransitionItem to ItemTree. Lower from CST. Store `in_modes` on
connections, subcomponents, flows, property associations. Implement SOM computation.
**Estimate**: Medium-Large.

### WP5: Semantic Connection Instances (addresses G1, G2, G6)
Trace connections end-to-end through the hierarchy during instantiation.
Expand feature group connections. Enforce direction rules.
**Estimate**: Large — core instance model rework.

### WP6: Complete Predeclared Properties (addresses G12)
Add Programming_Properties, Modeling_Properties, AADL_Project.
Expand existing sets to full standard definitions.
**Estimate**: Small.

---

## What's Working Well

- **Parser coverage is excellent** (~95%). All major AADL constructs parse correctly.
- **CST is lossless** — every byte preserved via Rowan red-green trees.
- **ItemTree pattern** works well for the constructs it captures.
- **Cross-file name resolution** works with case-insensitive lookup.
- **Property inheritance chain** (type→impl→subcomponent) with append support.
- **Instance model hierarchy** expands correctly across packages.
- **Connection endpoints** tracked at item tree and instance level.
- **Standard property sets** auto-registered without `with` imports.
- **Analysis framework** is pluggable with structured diagnostics.
- **Annex system** supports EMV2 + Behavior Annex sublanguage parsing.
- **Salsa integration** provides incremental recomputation foundation.
