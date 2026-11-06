//! Tree shapes of the SysML v2 flow grammar (REQ-SYSML2-FLOWS-MODES-001):
//! which token is a flow's name and which is its source end, and that a
//! malformed member never takes the members after it.

use spar_sysml2::{SyntaxKind, SyntaxNode, parse};

fn flow(src: &str) -> (SyntaxNode, usize) {
    let p = parse(src);
    let n = p.errors().len();
    let f = p
        .syntax_node()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::FLOW_USAGE)
        .unwrap_or_else(|| panic!("`{src}` has no FLOW_USAGE"));
    (f, n)
}

fn child_texts(n: &SyntaxNode, kind: SyntaxKind) -> Vec<String> {
    n.children()
        .filter(|c| c.kind() == kind)
        .map(|c| c.text().to_string().trim().to_string())
        .collect()
}

#[test]
fn a_named_flow_keeps_its_name_apart_from_its_ends() {
    let (f, errs) = flow("part def P { flow f of Fuel from a.p to b.q; }");
    assert_eq!(errs, 0);
    assert_eq!(child_texts(&f, SyntaxKind::NAME), ["f"]);
    assert_eq!(child_texts(&f, SyntaxKind::FLOW_PAYLOAD), ["of Fuel"]);
    assert_eq!(
        child_texts(&f, SyntaxKind::CONNECT_ENDPOINT),
        ["a.p", "b.q"]
    );
}

#[test]
fn a_flow_whose_first_token_starts_a_chain_is_anonymous() {
    for (src, ends) in [
        ("part def P { flow a.p to b.q; }", ["a.p", "b.q"]),
        ("part def P { flow p to q; }", ["p", "q"]),
        (
            "part def P { flow 'x y'.p to 'z'.q; }",
            ["'x y'.p", "'z'.q"],
        ),
        ("part def P { flow from a.p to b.q; }", ["a.p", "b.q"]),
        (
            "part def P { flow of Fuel from a.p.f to b.q.f; }",
            ["a.p.f", "b.q.f"],
        ),
    ] {
        let (f, errs) = flow(src);
        assert_eq!(errs, 0, "`{src}`");
        assert!(
            child_texts(&f, SyntaxKind::NAME).is_empty(),
            "`{src}`: {f:#?}"
        );
        assert_eq!(
            child_texts(&f, SyntaxKind::CONNECT_ENDPOINT),
            ends,
            "`{src}`"
        );
    }
}

/// `ServerSequenceOutsideRealization-2`: a redefining flow whose ends are
/// given in its body, not after `from`.
#[test]
fn a_flow_with_only_a_body_has_no_ends() {
    let (f, errs) = flow("part def P { flow :>> m : T { end a; end b; } }");
    assert_eq!(errs, 0);
    assert!(
        child_texts(&f, SyntaxKind::CONNECT_ENDPOINT).is_empty(),
        "{f:#?}"
    );
}

/// `succession` that does not begin a `succession flow` is not a flow: it
/// must be reported, not crash the parser expecting `flow`.
#[test]
fn a_succession_without_flow_is_an_error_not_a_crash() {
    let p = parse("part def P { succession s first a then b; part c; }");
    assert!(!p.ok());
    assert!(
        !p.syntax_node()
            .descendants()
            .any(|n| n.kind() == SyntaxKind::FLOW_USAGE)
    );
}

/// REGRESSION: before the flow grammar, `flow` was an ERROR node that
/// swallowed every member up to the closing brace.
#[test]
fn a_part_after_a_flow_survives() {
    let p = parse("part def P { flow a.p to b.q; part c; }");
    assert!(p.ok(), "{:?}", p.errors());
    assert!(
        p.syntax_node()
            .descendants()
            .any(|n| n.kind() == SyntaxKind::PART_USAGE)
    );
}
