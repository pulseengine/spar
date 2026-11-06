//! Tree shapes of the SysML v2 state grammar (REQ-SYSML2-FLOWS-MODES-001).
//!
//! `state_machine_modes.rs` checks what lowering reads; this file checks the
//! token boundaries lowering never sees: where a trigger, a guard and a state
//! action END on malformed or less common input, and which token is a name.
//! Each case is the one that killed a mutant the PR's in-diff mutation run
//! reported as surviving (DEC-MUTANTS-IN-DIFF-001).

use spar_sysml2::{SyntaxKind, SyntaxNode, parse};

fn tree(src: &str) -> (SyntaxNode, usize) {
    let p = parse(src);
    (p.syntax_node(), p.errors().len())
}

fn count(root: &SyntaxNode, kind: SyntaxKind) -> usize {
    root.descendants().filter(|n| n.kind() == kind).count()
}

fn text_of(root: &SyntaxNode, kind: SyntaxKind) -> Vec<String> {
    root.descendants()
        .filter(|n| n.kind() == kind)
        .map(|n| n.text().to_string().trim().to_string())
        .collect()
}

/// The state usages directly inside the first machine: a following member
/// that a runaway token loop swallowed is missing from this list.
fn states_after(src: &str) -> usize {
    count(&tree(src).0, SyntaxKind::STATE_USAGE)
}

#[test]
fn a_feature_named_accept_or_do_is_not_a_transition_or_an_action() {
    for src in ["part def P { accept : T; }", "part def P { do : T; }"] {
        let (root, errs) = tree(src);
        assert_eq!(errs, 0, "`{src}`");
        assert_eq!(
            count(&root, SyntaxKind::TARGET_TRANSITION_USAGE),
            0,
            "`{src}`"
        );
        assert_eq!(count(&root, SyntaxKind::STATE_ACTION), 0, "`{src}`");
    }
}

#[test]
fn exhibit_state_declares_a_name_and_exhibit_alone_refers_to_one() {
    let (root, errs) = tree("part def P { exhibit state s { state a; } }");
    assert_eq!(errs, 0);
    let su = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::STATE_USAGE)
        .unwrap();
    assert!(
        su.children().any(|c| c.kind() == SyntaxKind::NAME),
        "{su:#?}"
    );
    assert!(
        !su.children().any(|c| c.kind() == SyntaxKind::NAME_REF),
        "{su:#?}"
    );

    let (root, errs) = tree("part def P { exhibit vehicleStates { state a; } }");
    assert_eq!(errs, 0);
    let su = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::STATE_USAGE)
        .unwrap();
    assert!(
        su.children().any(|c| c.kind() == SyntaxKind::NAME_REF),
        "{su:#?}"
    );
    assert!(
        !su.children().any(|c| c.kind() == SyntaxKind::NAME),
        "{su:#?}"
    );
}

#[test]
fn an_anonymous_state_parses_without_a_name() {
    for src in [
        "part def P { state { state a; } }",
        "part def P { state : S; }",
    ] {
        assert_eq!(tree(src).1, 0, "`{src}`");
    }
}

#[test]
fn a_transition_without_first_has_a_source_and_no_name() {
    let (root, errs) =
        tree("part def P { state m { state initial; transition initial then off; state off; } }");
    assert_eq!(errs, 0);
    assert_eq!(text_of(&root, SyntaxKind::TRANSITION_SOURCE), ["initial"]);
    let t = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::TRANSITION_USAGE)
        .unwrap();
    assert!(
        !t.children().any(|c| c.kind() == SyntaxKind::NAME),
        "{t:#?}"
    );
}

#[test]
fn a_trigger_ends_at_do_when_there_is_no_guard() {
    let (root, errs) = tree(
        "part def P { port p; state m { state a; transition first a accept E via p do send e to p then b; state b; } }",
    );
    assert_eq!(errs, 0);
    assert_eq!(text_of(&root, SyntaxKind::TRIGGER), ["accept E via p"]);
    assert_eq!(text_of(&root, SyntaxKind::EFFECT), ["do send e to p"]);
}

/// A trigger, a guard: each must stop at a `;` or the enclosing `}` so the
/// error stays local and the next member still parses.
#[test]
fn a_malformed_transition_does_not_swallow_the_members_after_it() {
    for src in [
        "part def P { state m { state a; accept E; state b; } state c; }",
        "part def P { state m { state a; transition first a if ok; state b; } state c; }",
    ] {
        let (_, errs) = tree(src);
        assert!(errs > 0, "`{src}` is malformed");
        assert_eq!(states_after(src), 4, "`{src}`: m, a, b, c");
    }
    for src in [
        "part def P { state m { state a; accept E } state c; }",
        "part def P { state m { state a; transition first a if ok } state c; }",
    ] {
        let (_, errs) = tree(src);
        assert!(errs > 0, "`{src}` is malformed");
        assert_eq!(states_after(src), 3, "`{src}`: m, a, c");
    }
}

#[test]
fn a_state_action_body_with_nested_braces_ends_at_its_own_brace() {
    let src = "part def P { state m { entry action e { action f { } } state b; } }";
    let (root, errs) = tree(src);
    assert_eq!(errs, 0);
    let m = root
        .descendants()
        .find(|n| n.kind() == SyntaxKind::STATE_USAGE)
        .unwrap();
    let inner = m
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::STATE_USAGE)
        .count();
    assert_eq!(inner, 2, "`state b` is inside `m`: {m:#?}");
}
