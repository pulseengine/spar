//! REQ-SYSML2-ALLOCATE-BINDING-001 — a SysML v2 deployment reaches spar's
//! binding analyses.
//!
//! Two lowering gaps kept every imported model out of `binding_rules`,
//! `scheduling` and RTA:
//!
//! 1. a `part def` could only become `system`, `process` or `abstract`, so
//!    there was never a thread to bind nor a processor to bind it to;
//! 2. `allocate t to cpu;` parsed and was then dropped without a word.
//!
//! These tests pin the category convention (specialising an official
//! SysML-v2-AADL library type, `part def Worker :> Thread;`), the allocation
//! mapping, and that the printed AADL carries the binding as an
//! implementation-level `applies to` association.

use spar_hir_def::item_tree::{ComponentCategory, ItemTree, PropertyExpr};
use spar_sysml2::lower::{item_tree_to_aadl, lower_to_aadl_with_diagnostics};

fn lower(src: &str) -> (ItemTree, Vec<String>) {
    let parse = spar_sysml2::parse(src);
    assert!(parse.ok(), "parse errors: {:?}", parse.errors());
    let (tree, diags) = lower_to_aadl_with_diagnostics(&parse);
    (tree, diags.into_iter().map(|d| d.msg).collect())
}

fn type_category(tree: &ItemTree, name: &str) -> ComponentCategory {
    tree.component_types
        .iter()
        .find(|(_, ct)| ct.name.as_str() == name)
        .unwrap_or_else(|| panic!("no component type {name}"))
        .1
        .category
}

const DEPLOY: &str = r#"
package Deploy {
    private import AADL::*;
    part def Worker :> Thread;
    part def Cpu :> Processor;
    part def Ram :> Memory;
    part def Board :> System;
    part def App :> Process {
        part t : Worker;
    }
    part def Platform :> System {
        part app : App;
        part cpu : Cpu;
        part mem : Ram;
        part board : Board;
        ALLOC
    }
}
"#;

fn deploy(alloc: &str) -> String {
    DEPLOY.replace("ALLOC", alloc)
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn every_official_library_category_name_maps_to_its_aadl_category() {
    let table = [
        ("Abstract", ComponentCategory::Abstract),
        ("Bus", ComponentCategory::Bus),
        ("Data", ComponentCategory::Data),
        ("Device", ComponentCategory::Device),
        ("Memory", ComponentCategory::Memory),
        ("Process", ComponentCategory::Process),
        ("Processor", ComponentCategory::Processor),
        ("Subprogram", ComponentCategory::Subprogram),
        ("SubprogramGroup", ComponentCategory::SubprogramGroup),
        ("System", ComponentCategory::System),
        ("Thread", ComponentCategory::Thread),
        ("ThreadGroup", ComponentCategory::ThreadGroup),
        ("VirtualBus", ComponentCategory::VirtualBus),
        ("VirtualProcessor", ComponentCategory::VirtualProcessor),
    ];
    for (lib, expected) in table {
        for spelled in [lib.to_string(), format!("AADL::{lib}")] {
            let src = format!("package P {{ part def X :> {spelled}; }}");
            let (tree, _) = lower(&src);
            assert_eq!(type_category(&tree, "X"), expected, "`:> {spelled}`");
            let ct = tree.component_types.iter().next().unwrap().1;
            assert!(
                ct.extends.is_none(),
                "`:> {spelled}` is a category marker, not an AADL classifier to extend"
            );
        }
    }
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn library_category_wins_over_sysml_abstract_and_body_heuristic() {
    let (tree, _) = lower("package P { abstract part def X :> Thread { action run; } }");
    assert_eq!(type_category(&tree, "X"), ComponentCategory::Thread);
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn unknown_or_shadowed_supertypes_keep_todays_behaviour() {
    // Unknown supertype: category inferred as before, `extends` kept.
    let (tree, _) = lower("package P { part def Base; part def X :> Base; }");
    assert_eq!(type_category(&tree, "X"), ComponentCategory::System);
    let x = tree
        .component_types
        .iter()
        .find(|(_, ct)| ct.name.as_str() == "X")
        .unwrap()
        .1;
    assert_eq!(x.extends.as_ref().unwrap().type_name.as_str(), "Base");

    // A model that declares its OWN `part def Processor` specialises that,
    // not the library type: no processor appears from a name collision.
    let (tree, _) = lower("package P { part def Processor; part def X :> Processor; }");
    assert_eq!(type_category(&tree, "X"), ComponentCategory::System);
    assert_eq!(type_category(&tree, "Processor"), ComponentCategory::System);

    // ...but the qualified spelling always means the library.
    let (tree, _) = lower("package P { part def Processor; part def X :> AADL::Processor; }");
    assert_eq!(type_category(&tree, "X"), ComponentCategory::Processor);

    // A non-AADL package qualifier is not the library.
    let (tree, _) = lower("package P { part def X :> Other::Thread; }");
    assert_eq!(type_category(&tree, "X"), ComponentCategory::System);
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_processor_lowers_to_actual_processor_binding_applies_to() {
    let (tree, diags) = lower(&deploy("allocate app.t to cpu;"));
    assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
    let platform = tree
        .component_impls
        .iter()
        .find(|(_, ci)| ci.type_name.as_str() == "Platform")
        .unwrap()
        .1;
    assert_eq!(platform.property_associations.len(), 1);
    let pa = &tree.property_associations[platform.property_associations[0]];
    assert_eq!(
        pa.name.to_string(),
        "Deployment_Properties::Actual_Processor_Binding"
    );
    assert_eq!(pa.applies_to.as_deref(), Some("app.t"));
    assert_eq!(pa.value, "(reference (cpu))");
    assert_eq!(
        pa.typed_value,
        Some(PropertyExpr::List(vec![PropertyExpr::ReferenceValue(
            "cpu".to_string()
        )]))
    );

    let aadl = item_tree_to_aadl(&tree);
    assert!(
        aadl.contains(
            "Deployment_Properties::Actual_Processor_Binding => (reference (cpu)) applies to app.t;"
        ),
        "{aadl}"
    );
    // The binding lives in the implementation that owns both parts.
    let impl_start = aadl.find("system implementation Platform.impl").unwrap();
    let impl_end = aadl.find("end Platform.impl;").unwrap();
    let binding = aadl.find("Actual_Processor_Binding").unwrap();
    assert!(impl_start < binding && binding < impl_end, "{aadl}");
    // Categories reach the printed text, and a part typed by a def with an
    // implementation names that implementation (else it is never instanced).
    assert!(aadl.contains("thread Worker\n"), "{aadl}");
    assert!(aadl.contains("processor Cpu\n"), "{aadl}");
    assert!(aadl.contains("app : process App.impl;"), "{aadl}");
    assert!(aadl.contains("cpu : processor Cpu;"), "{aadl}");
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_virtual_processor_is_a_processor_binding() {
    let src = r#"package P {
        part def Vp :> VirtualProcessor;
        part def W :> Thread;
        part def S :> System { part w : W; part vp : Vp; allocate w to vp; }
    }"#;
    let (tree, diags) = lower(src);
    assert!(diags.is_empty(), "{diags:?}");
    let pa = tree.property_associations.iter().next().unwrap().1;
    assert_eq!(pa.name.property_name.as_str(), "Actual_Processor_Binding");
    assert_eq!(pa.applies_to.as_deref(), Some("w"));
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_memory_lowers_to_actual_memory_binding() {
    let (tree, diags) = lower(&deploy("allocate app.t to mem;"));
    assert!(diags.is_empty(), "{diags:?}");
    let pa = tree.property_associations.iter().next().unwrap().1;
    assert_eq!(
        pa.name.to_string(),
        "Deployment_Properties::Actual_Memory_Binding"
    );
    assert_eq!(pa.value, "(reference (mem))");
    assert_eq!(pa.applies_to.as_deref(), Some("app.t"));
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_to_other_category_still_emits_processor_binding_and_says_so() {
    let (tree, diags) = lower(&deploy("allocate app.t to board;"));
    let pa = tree.property_associations.iter().next().unwrap().1;
    assert_eq!(pa.name.property_name.as_str(), "Actual_Processor_Binding");
    assert_eq!(pa.value, "(reference (board))");
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(
        diags[0].contains("board") && diags[0].contains("system"),
        "{diags:?}"
    );
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn unresolvable_allocate_ends_are_diagnosed_not_silently_dropped() {
    for (alloc, culprit) in [
        ("allocate nope to cpu;", "nope"),
        ("allocate app.t to nowhere;", "nowhere"),
        ("allocate app.zz to cpu;", "app.zz"),
        // `cpu` is typed by a def without an implementation: no `x` inside.
        ("allocate app.t to cpu.x;", "cpu.x"),
    ] {
        let (tree, diags) = lower(&deploy(alloc));
        assert_eq!(
            tree.property_associations.len(),
            0,
            "`{alloc}` must not emit a binding"
        );
        assert_eq!(diags.len(), 1, "`{alloc}`: {diags:?}");
        assert!(diags[0].contains(culprit), "`{alloc}`: {diags:?}");
    }
}

// rivet: verifies REQ-SYSML2-ALLOCATE-BINDING-001
#[test]
fn allocate_in_a_part_def_without_parts_is_diagnosed() {
    let (tree, diags) = lower("package P { part def S { allocate a to b; } }");
    assert_eq!(tree.property_associations.len(), 0);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(diags[0].contains("allocate a to b"), "{diags:?}");
}
