//! Minimal snippets for the three grammar constructs `REQ-SYSML2-CORPUS-RATCHET-001`
//! added, chosen from the first-error histogram of the official corpus.
//!
//! Each `*_parses` case was run against `origin/main` before the fix and
//! FAILED there; the `*_rejected` / `*_still_*` cases pin the edges of each
//! construct so the grammar does not over-accept. Assertions use only
//! `parse().ok()` and the lowering diagnostics' text, never the new
//! `SyntaxKind`s, so this file also compiles against the pre-fix parser — that
//! is how the red run was taken.

use spar_sysml2::lower::lower_to_aadl_with_diagnostics;
use spar_sysml2::parse;

fn ok(src: &str) -> bool {
    parse(src).ok()
}

fn assert_parses(src: &str) {
    let p = parse(src);
    assert!(
        p.ok(),
        "expected {src:?} to parse, errors: {:?}",
        p.errors()
    );
}

fn assert_rejected(src: &str) {
    assert!(!ok(src), "expected {src:?} to be REJECTED");
}

/// Lowering diagnostics whose message names `kind`.
fn unsupported(src: &str, kind: &str) -> usize {
    let p = parse(src);
    assert!(p.ok(), "fixture must parse: {:?}", p.errors());
    let (_tree, diags) = lower_to_aadl_with_diagnostics(&p);
    diags
        .iter()
        .filter(|d| d.msg.contains("unsupported") && d.msg.contains(kind))
        .count()
}

// ---------------------------------------------------------------------------
// 1. FeatureSpecializationPart: multiplicity after the typing
//    FeatureSpecialization+ MultiplicityPart? FeatureSpecialization*
// ---------------------------------------------------------------------------

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn multiplicity_after_typing_parses() {
    assert_parses("part def V { part cyl : Cylinder[4..6]; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn specialization_after_multiplicity_parses() {
    // `FeatureSpecialization*` after the MultiplicityPart.
    assert_parses("part def V { part w : Wheel[4] :> parts; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn two_specializations_in_sequence_parse() {
    assert_parses("part def V { part e : Engine :> parts; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn a_second_multiplicity_after_typing_is_rejected() {
    // The production allows ONE MultiplicityPart.
    assert_rejected("part def V { part cyl : Cylinder[4][6]; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn multiplicity_after_typing_is_lowered_not_dropped() {
    // MULTIPLICITY stays a child of the usage, so the existing array-dimension
    // lowering sees it: `[4]` becomes a subcomponent array dimension of 4.
    let p = parse("package P { part def Cyl; part def V { part cyl : Cyl[4]; } }");
    assert!(p.ok(), "{:?}", p.errors());
    let tree = lower_to_aadl_with_diagnostics(&p).0;
    let cyl = tree
        .subcomponents
        .iter()
        .map(|(_, s)| s)
        .find(|s| s.name.as_str() == "cyl")
        .expect("cyl subcomponent");
    assert_eq!(cyl.array_dimensions.len(), 1);
    assert_eq!(
        cyl.array_dimensions[0].size,
        Some(spar_hir_def::item_tree::ArraySize::Literal(4))
    );
}

// ---------------------------------------------------------------------------
// 2. KerML classifiers: Class | Classifier | DataType | Behavior
//    = TypePrefix kw ClassifierDeclaration TypeBody
// ---------------------------------------------------------------------------

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn kerml_class_parses() {
    assert_parses("package P { class A; class B :> A { } }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn kerml_classifier_datatype_behavior_parse() {
    assert_parses("package P { classifier C; }");
    assert_parses("package P { datatype D; }");
    assert_parses("package P { behavior B { } }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn kerml_superclassing_list_parses() {
    // SuperclassingPart = SPECIALIZES OwnedSubclassification (',' ...)*
    assert_parses("package P { classifier C specializes A, B; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn kerml_classifier_multiplicity_parses() {
    // ClassifierDeclaration = Identification OwnedMultiplicity? ...
    assert_parses("package P { class A[1]; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn a_feature_named_class_is_still_a_feature() {
    // `class` is not reserved: followed by `:` it is a feature's name, not a
    // classifier keyword. This parses before and after the fix.
    assert_parses("part def P { class : Integer; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn unsupported_classifier_relationships_are_still_rejected() {
    // TypeRelationshipPart is not implemented and must not be swallowed.
    assert_rejected("package P { classifier D disjoint from C; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn kerml_classifier_lowering_reports_it_once() {
    assert_eq!(unsupported("package P { class A; }", "KERML_CLASSIFIER"), 1);
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn a_nested_kerml_classifier_is_reported_too() {
    // The package walk reports the outer classifier but never enters it.
    assert_eq!(
        unsupported("package P { class A { class B; } }", "KERML_CLASSIFIER"),
        2
    );
}

// ---------------------------------------------------------------------------
// 3. PerformActionUsage
//    'perform' ( OwnedReferenceSubsetting FeatureSpecializationPart?
//              | 'action' UsageDeclaration ) ActionBody
// ---------------------------------------------------------------------------

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_action_parses() {
    assert_parses("part def P { perform action providePower; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_action_with_typing_and_body_parses() {
    assert_parses("part def P { perform action 'provide power' : 'Provide Power' { } }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_feature_chain_parses() {
    assert_parses("part def P { perform providePower.generateTorque; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_reference_with_redefinition_parses() {
    assert_parses("part def P { perform 'provide power' :>> VehicleA::'provide power'; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn unnamed_perform_action_redefines_parses() {
    // `redefines` opens the FeatureSpecializationPart; it is not the name.
    assert_parses("part def P { perform action redefines providePower; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn unnamed_perform_action_subsets_parses() {
    assert_parses("part def P { perform action subsets actions; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_at_top_level_parses() {
    assert_parses("perform action a;");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_without_a_target_is_rejected() {
    assert_rejected("part def P { perform; }");
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_in_a_part_def_is_reported_once() {
    assert_eq!(
        unsupported("part def P { perform action a; }", "PERFORM_ACTION_USAGE"),
        1
    );
}

// rivet: verifies REQ-SYSML2-CORPUS-RATCHET-001
#[test]
fn perform_in_a_part_usage_body_is_reported() {
    // The part-usage lowering never walks its body; without the post-pass the
    // performed action would vanish with no diagnostic.
    assert_eq!(
        unsupported(
            "part def V { part v : V { perform action a; } }",
            "PERFORM_ACTION_USAGE"
        ),
        1
    );
}
