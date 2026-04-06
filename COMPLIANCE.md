# AS5506 AADL v2.2 Compliance Gap Analysis

**Updated**: 2026-04-06 (v0.5.0)
**Source**: 102 HTML files from OSATE2 (`org.osate.help/html/std/`)
**Toolchain**: spar (1200+ tests passing across 16 crates)

---

## Executive Summary

| Layer | Status | Score |
|-------|--------|-------|
| **Parser (syntax)** | Excellent | ~95% — all major constructs parsed, doubled-quote strings |
| **ItemTree (declaration capture)** | Good | ~85% — modes, prototypes, calls, flow impls, array dims, in_modes, requires_modes |
| **Name Resolution** | Partial | ~55% — cross-file resolution + naming rules + duplicate package detection |
| **Legality Rules (L-rules)** | Partial | ~15% — engine scaffold with ~20 rules from 6 categories |
| **Naming Rules (N-rules)** | Partial | ~25% — duplicate detection, with-clause hygiene, scope uniqueness |
| **Instance Model** | Good | ~80% — hierarchy + modes + semantic connections + features + **extends inheritance** |
| **Property System** | Good | ~65% — typed PropertyExpr (14 variants), text fallback parser, unit conversion |
| **Modes** | Good | ~65% — in ItemTree + instance model + requires_modes + modal filtering |
| **Predeclared Property Sets** | Good | ~90% — 8/8 sets, 102 properties (corrected classifications) |

---

## Section-by-Section Status

### Chapter 1-3: Scope, Overview, Definitions — N/A (informative)

### Chapter 4: Packages & Namespaces

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 4.1 | AADL Specifications | DONE | DONE | PARTIAL — duplicate name detection via naming_rules |
| 4.2 | Packages | DONE | DONE | PARTIAL — `with` clause hygiene enforced, `renames` not lowered, private visibility not enforced |
| 4.3 | Component Types | DONE | DONE | PARTIAL — duplicate feature/flow/mode detection, category restriction checks |
| 4.4 | Component Impls | DONE | DONE | PARTIAL — duplicate subcomponent/connection detection, impl-type match check |
| 4.5 | Subcomponents | DONE | DONE | PARTIAL — category restriction checks, in_modes preserved |
| 4.6 | Calls | DONE | DONE | PARTIAL — call sequences lowered to ItemTree |
| 4.7 | Prototypes | DONE | DONE | PARTIAL — lowered to ItemTree, resolution + binding validation |
| 4.8 | Annexes | DONE | PARTIAL | PARTIAL — library only, subclauses dropped |

### Chapter 5: Software Components (data, subprogram, thread, process)

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 5.1 | Data | DONE | DONE | DONE — feature/subcomponent restrictions checked via category_rules |
| 5.2 | Subprogram | DONE | DONE | DONE — parameter restrictions checked |
| 5.3 | Subprogram Group | DONE | DONE | DONE — category restrictions checked |
| 5.4 | Thread | DONE | DONE | PARTIAL — category checked, dispatch protocol not required |
| 5.5 | Thread Group | DONE | DONE | DONE — category restrictions checked |
| 5.6 | Process | DONE | DONE | PARTIAL — category checked, thread requirement not enforced |

### Chapter 6: Execution Platform (processor, memory, bus, device)

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 6.1 | Processor | DONE | DONE | DONE — feature restrictions checked |
| 6.2 | Virtual Processor | DONE | DONE | DONE — category restrictions checked |
| 6.3 | Memory | DONE | DONE | DONE — category restrictions checked |
| 6.4 | Bus | DONE | DONE | DONE — category restrictions checked |
| 6.5 | Virtual Bus | DONE | DONE | DONE — category restrictions checked |
| 6.6 | Device | DONE | DONE | DONE — category restrictions checked |
| 6.7 | System | DONE | DONE | DONE — category restrictions checked |

### Chapter 7-8: Features

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 8.1 | Ports | DONE | DONE | PARTIAL — direction rules enforced for connections |
| 8.2 | Port Groups | DONE | DONE | PARTIAL — feature group expansion utility exists |
| 8.3 | Data Access | DONE | DONE | PARTIAL — provides/requires in ItemTree, access kind tracked |
| 8.4 | Bus Access | DONE | DONE | PARTIAL — access kind tracked |
| 8.5 | Subprogram Access | DONE | DONE | PARTIAL — access kind tracked |
| 8.6 | Feature Groups | DONE | DONE | PARTIAL — expansion with inverse_of, complement not yet validated |
| 8.7 | Abstract Features | DONE | DONE | DONE |

### Chapter 9: Connections

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 9.1 | Overview | DONE | DONE | PARTIAL — semantic connection tracing (across), no feature array connections |
| 9.2 | Feature Groups | DONE | DONE | PARTIAL — FG expansion utility exists, not yet integrated into connection instances |
| 9.3 | Port Connections | DONE | DONE | DONE — direction rules enforced (across/up/down classification) |
| 9.4 | Direction Rules | DONE | DONE | DONE — DirectionRuleAnalysis checks in/out/in_out compatibility |
| 9.5 | Access Connections | DONE | DONE | PARTIAL — provides/requires tracked, direction rules applied |
| 9.6 | Bus/Data/Subprogram Access | DONE | DONE | PARTIAL — access kind differentiated |
| 9.7 | Parameter Connections | DONE | DONE | PARTIAL — direction rules applied, call context not validated |
| 9.8 | Connection Patterns | PARTIAL | PARTIAL | MISSING — array patterns not handled |

### Chapter 10: Flows

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 10.1 | Flow Specifications | DONE | DONE | PARTIAL — endpoints stored, direction validated by FlowCheckAnalysis |
| 10.2 | Flow Spec Legality | DONE | DONE | PARTIAL — source/sink/path direction checks |
| 10.3 | Flow Implementations | DONE | DONE | PARTIAL — lowered to ItemTree, segment validation exists |
| 10.4 | End-to-End Flows | DONE | DONE | PARTIAL — segments validated, alternation check (flow/conn) |
| 10.5 | Flow Properties | N/A | PARTIAL | MISSING — latency properties defined but not evaluated |
| 10.6 | Latency Analysis | N/A | N/A | MISSING — no analysis |

### Chapter 11: Properties

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 11.1 | Property Sets | DONE | DONE | PARTIAL — 8/8 predeclared sets, type defs exist |
| 11.2 | Property Types | DONE | PARTIAL | PARTIAL — PropertyTypeDef enum with 10 type variants |
| 11.3 | Property Expressions | DONE | PARTIAL | PARTIAL — PropertyExpr enum (13 variants), type checking, unit conversion |
| 11.4 | Property Associations | DONE | DONE | PARTIAL — inheritance chain, append support, modal values not fully resolved |

### Chapter 12: Modes

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 12.1 | Mode Overview | DONE | DONE | DONE — ModeItem in ItemTree, ModeInstance in instance model |
| 12.2 | Mode Declarations | DONE | DONE | DONE — initial mode validation, uniqueness via naming_rules |
| 12.3 | Mode Transitions | DONE | DONE | DONE — ModeTransitionItem/Instance, trigger validation |
| 12.4 | Modal Configurations | DONE | DONE | PARTIAL — `in_modes` preserved on connections/subcomponents, not yet filtered |
| 12.5 | Requires Modes | DONE | DONE | MISSING — not yet validated |

### Chapter 13: Execution & Binding

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 13.1 | System Binding | N/A | N/A | PARTIAL — BindingCheckAnalysis validates processor/memory binding targets |

### Chapter 14: Instance Model

| Section | Title | Parser | ItemTree | Semantics |
|---------|-------|--------|----------|-----------|
| 14.1 | System Instances | N/A | N/A | DONE — recursive hierarchy expansion |
| 14.2 | Semantic Connections | N/A | N/A | PARTIAL — across connections traced, up/down multi-level pending |
| 14.3 | Binding Instances | N/A | N/A | PARTIAL — binding property validation exists |
| 14.4 | System Lifecycle | N/A | N/A | MISSING |
| 14.5 | Mode Instances/SOMs | N/A | N/A | PARTIAL — mode instances exist, SOM computation pending |
| 14.6 | System Operation Modes | N/A | N/A | PARTIAL — mode instances populated |
| 14.7 | Mode Transition Semantics | N/A | N/A | PARTIAL — transitions instantiated, runtime semantics not modeled |
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
| A | Predeclared Property Sets | DONE — 8/8 sets (104 properties): Timing, Communication, Memory, Thread, Deployment, Programming, Modeling, AADL_Project |
| B | Glossary | N/A (informative) |
| C | Syntax Summary | N/A (informative) |
| D | Graphical Notation | MISSING (normative but optional for textual tools) |
| E | XML/AAXL2 Meta Model | MISSING — no OSATE interop serialization |
| F | UML Profile | N/A |
| G | Profiles/Extensions | MISSING |

---

## Gap Status After T1-T16 Implementation

### Critical Gaps

| Gap | Title | Status | Notes |
|-----|-------|--------|-------|
| G1 | Semantic Connection Instances | **Partially closed** | Across connections traced; up/down multi-level tracing in progress |
| G2 | Port Direction Rules | **Closed** | DirectionRuleAnalysis enforces across/up/down direction compatibility |
| G3 | Property Type System | **Partially closed** | PropertyExpr (13 variants) + PropertyTypeDef (10 types) + type checking + unit conversion; CST→PropertyExpr lowering in progress |
| G4 | Modes in HIR | **Closed** | ModeItem/ModeTransitionItem in ItemTree; ModeInstance/ModeTransitionInstance in instance model; ModeCheckAnalysis validates |
| G5 | Legality Rule Engine | **Partially closed** | LegalityEngine scaffold with ~20 rules across 6 categories (naming, category, direction, binding, flow, hierarchy) |
| G6 | Feature Group Expansion | **Partially closed** | Expansion utility with inverse_of; not yet integrated into connection instance resolution |

### Major Gaps

| Gap | Title | Status | Notes |
|-----|-------|--------|-------|
| G7 | ItemTree Information Loss | **Mostly closed** | Modes, prototypes, call sequences, flow impls, array dims, in_modes, access kind all lowered |
| G8 | Naming Rule Validation | **Partially closed** | Duplicate detection (features, subcomponents, connections, modes, properties), with-clause hygiene |
| G9 | Category Restrictions | **Closed** | Static restriction tables for all 14 categories + CategoryCheck analysis pass |
| G10 | Flow Implementation Lowering | **Closed** | FlowImplItem in ItemTree + FlowCheckAnalysis validates direction and segments |
| G11 | Binding Analysis | **Closed** | BindingCheckAnalysis validates processor/memory binding targets |
| G12 | Predeclared Property Sets | **Closed** | 8/8 sets, 104 properties matching AS5506 Appendix A |

---

## Remaining Work

### High Priority
- Complete multi-level semantic connection tracing (up/down patterns through hierarchy)
- Complete CST→PropertyExpr lowering (currently property values still lower as strings)
- SOM computation (cartesian product of modes across modal subcomponents)
- LSP server for IDE integration

### Medium Priority
- Feature group expansion integration into connection instance resolution
- `in_modes` filtering of connections and properties at runtime
- `requires_modes` validation
- Scheduling analysis (RMA, processor utilization)
- Latency analysis (end-to-end flow latency)

### Lower Priority
- Resource budget analysis (memory, MIPS, bandwidth)
- Bus load analysis
- ARINC 653 partition scheduling
- Connection array patterns
- AAXL2/XML serialization for OSATE interop
- Private section visibility enforcement
- `renames` resolution

---

## What's Working Well

- **Parser coverage is excellent** (~95%). All major AADL constructs parse correctly.
- **CST is lossless** — every byte preserved via Rowan red-green trees.
- **ItemTree captures ~85%** of parsed constructs including modes, prototypes, flows, properties.
- **Cross-file name resolution** works with case-insensitive lookup.
- **Property system** has typed expressions, type checking, unit conversion, and inheritance.
- **Instance model** expands hierarchy with features, connections, modes, and semantic connections.
- **10 analysis passes** cover connectivity, hierarchy, completeness, direction, binding, flow, mode, category, naming, and legality.
- **Category restriction tables** enforce feature/subcomponent rules for all 14 categories.
- **Mode support** spans ItemTree through instance model with validation.
- **Annex system** supports EMV2 + Behavior Annex sublanguage parsing.
- **Salsa integration** provides incremental recomputation foundation.
- **104 predeclared properties** across all 8 standard property sets.
