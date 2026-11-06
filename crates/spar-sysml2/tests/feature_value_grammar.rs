//! A directed feature bound to a value, `in x = a.b;`: the SysML v2
//! `FeatureValue` on a parameter, as the official corpus writes it inside
//! actions, performs, calculations and exhibited states
//! (`examples/Vehicle Example`, `training/18. Calculations`, KerML
//! `Simple Tests/Expressions`).
//!
//! Before this, `in x = y;` was a parse error that took the members after it.

use spar_sysml2::{SyntaxKind, SyntaxNode, parse};

fn tree(src: &str) -> (SyntaxNode, usize) {
    let p = parse(src);
    (p.syntax_node(), p.errors().len())
}

fn texts(root: &SyntaxNode, kind: SyntaxKind) -> Vec<String> {
    root.descendants()
        .filter(|n| n.kind() == kind)
        .map(|n| n.text().to_string().trim().to_string())
        .collect()
}

#[test]
fn a_directed_feature_takes_a_value_up_to_the_end_of_the_member() {
    for (src, value) in [
        ("part def P { action a { in x = y; } }", "= y"),
        (
            "part def P { perform a { in m = vehicle.mass; } }",
            "= vehicle.mass",
        ),
        (
            "part def P { action a { in c = 'provide power'::fuelCmd; } }",
            "= 'provide power'::fuelCmd",
        ),
        (
            "part def P { action a { in p = profile#(i); } }",
            "= profile#(i)",
        ),
        ("part def P { exhibit vs { in controller = c; } }", "= c"),
        ("part def P { action a { out o : T = y; } }", "= y"),
    ] {
        let (root, errs) = tree(src);
        assert_eq!(errs, 0, "`{src}`");
        assert_eq!(texts(&root, SyntaxKind::FEATURE_VALUE), [value], "`{src}`");
    }
}

/// KerML `Simple Tests/Expressions`: a value followed by a body.
#[test]
fn a_value_ends_where_the_features_body_begins() {
    let (root, errs) = tree("part def P { action a { in o = w::self { in f; } } }");
    assert_eq!(errs, 0);
    assert_eq!(texts(&root, SyntaxKind::FEATURE_VALUE), ["= w::self"]);
}

/// The member after a bound parameter is its sibling, not part of its value.
#[test]
fn the_member_after_a_bound_parameter_survives() {
    let (root, errs) = tree("part def P { action a { in x = y; in z = w; } part q; }");
    assert_eq!(errs, 0);
    assert_eq!(texts(&root, SyntaxKind::FEATURE_VALUE), ["= y", "= w"]);
    assert!(
        root.descendants()
            .any(|n| n.kind() == SyntaxKind::PART_USAGE && n.text().to_string().contains('q'))
    );
}

/// Without a value nothing changes.
#[test]
fn a_directed_feature_without_a_value_has_none() {
    let (root, errs) = tree("state def S { in x : T; }");
    assert_eq!(errs, 0);
    assert!(texts(&root, SyntaxKind::FEATURE_VALUE).is_empty());
}

/// Brackets nest: a `{`, `;` or `}` inside `(...)` or `[...]` belongs to the
/// value (an expression body passed as an argument), not to the member.
#[test]
fn a_stop_token_inside_brackets_does_not_end_the_value() {
    for (src, value) in [
        (
            "part def P { action a { in x = select(xs, {in e; e.ok}); } }",
            "= select(xs, {in e; e.ok})",
        ),
        (
            "part def P { action a { in x = xs[{in e; e}]; } }",
            "= xs[{in e; e}]",
        ),
    ] {
        let (root, errs) = tree(src);
        assert_eq!(errs, 0, "`{src}`");
        assert_eq!(texts(&root, SyntaxKind::FEATURE_VALUE), [value], "`{src}`");
    }
}
