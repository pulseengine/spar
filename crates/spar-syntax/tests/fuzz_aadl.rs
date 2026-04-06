//! Adversarial/fuzz tests for the AADL parser.
//!
//! These tests feed malformed, edge-case, and adversarial inputs to the parser
//! and verify it does not panic, hang, or crash. Parse errors are expected and
//! acceptable; panics are not.

use std::time::{Duration, Instant};

/// Helper: parse AADL input and assert no panic within a timeout.
fn parse_must_not_panic(label: &str, input: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(|| spar_syntax::parse(input));
    let elapsed = start.elapsed();

    match result {
        Ok(parse) => {
            // Check for hang (>10 seconds)
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: took {elapsed:?}"
            );
            // We don't assert parse.ok() -- errors are fine for adversarial inputs.
            // Just print status for debugging.
            if parse.ok() {
                eprintln!("[OK]    {label} ({elapsed:?})");
            } else {
                eprintln!(
                    "[ERR]   {label} ({} errors, {elapsed:?})",
                    parse.errors().len()
                );
            }
        }
        Err(panic_info) => {
            panic!("[PANIC] {label}: {panic_info:?}");
        }
    }
}

// ── Basic edge cases ────────────────────────────────────────────────

#[test]
fn fuzz_empty_file() {
    parse_must_not_panic("empty file", "");
}

#[test]
fn fuzz_whitespace_only() {
    parse_must_not_panic("whitespace only", "   \n\t\n   ");
}

#[test]
fn fuzz_single_semicolon() {
    parse_must_not_panic("single semicolon", ";");
}

#[test]
fn fuzz_just_newlines() {
    parse_must_not_panic("just newlines", "\n\n\n\n\n");
}

#[test]
fn fuzz_just_comments() {
    parse_must_not_panic(
        "just comments",
        "-- this is a comment\n-- another comment\n",
    );
}

// ── Unclosed blocks ─────────────────────────────────────────────────

#[test]
fn fuzz_unclosed_package() {
    parse_must_not_panic("unclosed package", "package P public");
}

#[test]
fn fuzz_unclosed_system() {
    parse_must_not_panic("unclosed system", "package P public system S features");
}

#[test]
fn fuzz_unclosed_system_impl() {
    parse_must_not_panic(
        "unclosed system impl",
        "package P public system implementation S.I subcomponents",
    );
}

#[test]
fn fuzz_missing_end_keyword() {
    parse_must_not_panic(
        "missing end keyword",
        "package P public system S end S; end",
    );
}

#[test]
fn fuzz_extra_end_keywords() {
    parse_must_not_panic(
        "extra end keywords",
        "package P public system S end S; end P; end P; end;",
    );
}

// ── Keywords as identifiers ─────────────────────────────────────────

#[test]
fn fuzz_keywords_as_identifiers() {
    parse_must_not_panic(
        "keywords as identifiers",
        "package package public system system end end;",
    );
}

#[test]
fn fuzz_end_as_name() {
    parse_must_not_panic(
        "end as component name",
        "package P public system end end end; end P;",
    );
}

#[test]
fn fuzz_features_keyword_overload() {
    parse_must_not_panic(
        "features keyword overload",
        "package features public system features features inp: in data port; end features; end features;",
    );
}

// ── Very long identifiers ───────────────────────────────────────────

#[test]
fn fuzz_long_identifier() {
    let name = "A".repeat(10000);
    let input = format!("package {name} public end {name};");
    parse_must_not_panic("10000-char identifier", &input);
}

#[test]
fn fuzz_very_long_identifier() {
    let name = "B".repeat(100000);
    let input = format!("package {name} public end {name};");
    parse_must_not_panic("100000-char identifier", &input);
}

// ── Null bytes and binary data ──────────────────────────────────────

#[test]
fn fuzz_null_bytes() {
    parse_must_not_panic("null bytes", "package P\0\0 public end P;");
}

#[test]
fn fuzz_embedded_null() {
    parse_must_not_panic(
        "embedded null in identifier",
        "package P\0Q public end P\0Q;",
    );
}

#[test]
fn fuzz_binary_garbage() {
    let garbage: String = (0..256).map(|b| b as u8 as char).collect();
    parse_must_not_panic("binary garbage (0-255)", &garbage);
}

#[test]
fn fuzz_all_zero_bytes() {
    let zeros = "\0".repeat(1000);
    parse_must_not_panic("1000 null bytes", &zeros);
}

// ── Unicode and non-ASCII ───────────────────────────────────────────

#[test]
fn fuzz_unicode_identifiers() {
    parse_must_not_panic(
        "unicode identifiers",
        "package \u{00DC}n\u{00EF}c\u{00F6}d\u{00E9} public end \u{00DC}n\u{00EF}c\u{00F6}d\u{00E9};",
    );
}

#[test]
fn fuzz_emoji_input() {
    parse_must_not_panic("emoji input", "package \u{1F600} public end \u{1F600};");
}

#[test]
fn fuzz_cjk_characters() {
    parse_must_not_panic(
        "CJK characters",
        "package \u{4E16}\u{754C} public end \u{4E16}\u{754C};",
    );
}

#[test]
fn fuzz_rtl_text() {
    parse_must_not_panic(
        "RTL Arabic text",
        "package \u{0645}\u{0631}\u{062D}\u{0628}\u{0627} public end \u{0645}\u{0631}\u{062D}\u{0628}\u{0627};",
    );
}

#[test]
fn fuzz_bom_prefix() {
    parse_must_not_panic(
        "BOM prefix",
        "\u{FEFF}package P public system S end S; end P;",
    );
}

// ── Massive numeric properties ──────────────────────────────────────

#[test]
fn fuzz_massive_property_value() {
    parse_must_not_panic(
        "massive property value",
        "package P public system S properties Period => 999999999999999999999999999999 ms; end S; end P;",
    );
}

#[test]
fn fuzz_negative_property_value() {
    parse_must_not_panic(
        "negative property value",
        "package P public system S properties Period => -1 ms; end S; end P;",
    );
}

#[test]
fn fuzz_zero_property_value() {
    parse_must_not_panic(
        "zero property value",
        "package P public system S properties Period => 0 ms; end S; end P;",
    );
}

// ── Deeply nested property expressions ──────────────────────────────

#[test]
fn fuzz_nested_property_expr() {
    parse_must_not_panic(
        "nested parens in property",
        "package P public system S properties X => ((((((((((1))))))))); end S; end P;",
    );
}

#[test]
fn fuzz_deeply_nested_parens() {
    let open = "(".repeat(200);
    let close = ")".repeat(200);
    let input = format!("package P public system S properties X => {open}1{close}; end S; end P;");
    parse_must_not_panic("200-deep nested parens", &input);
}

// ── Many components ─────────────────────────────────────────────────

#[test]
fn fuzz_1000_components() {
    let mut input = String::from("package P public\n");
    for i in 0..1000 {
        input.push_str(&format!("  system S{i} end S{i};\n"));
    }
    input.push_str("end P;\n");
    parse_must_not_panic("1000 components", &input);
}

#[test]
fn fuzz_500_features() {
    let mut input = String::from("package P public\n  system S\n    features\n");
    for i in 0..500 {
        input.push_str(&format!("      p{i}: in data port;\n"));
    }
    input.push_str("  end S;\nend P;\n");
    parse_must_not_panic("500 features", &input);
}

// ── Circular and recursive definitions ──────────────────────────────

#[test]
fn fuzz_circular_extends() {
    parse_must_not_panic(
        "circular extends",
        "package P public system A extends B end A; system B extends A end B; end P;",
    );
}

#[test]
fn fuzz_self_extending() {
    parse_must_not_panic(
        "self-extending type",
        "package P public system A extends A end A; end P;",
    );
}

#[test]
fn fuzz_recursive_subcomponent() {
    parse_must_not_panic(
        "recursive subcomponent",
        r#"package Rec public
  system S end S;
  system implementation S.I
    subcomponents child: system S.I;
  end S.I;
end Rec;"#,
    );
}

// ── Annex blocks ────────────────────────────────────────────────────

#[test]
fn fuzz_empty_annex() {
    parse_must_not_panic(
        "empty annex",
        "package P public system S annex EMV2 {** **}; end S; end P;",
    );
}

#[test]
fn fuzz_malformed_annex() {
    parse_must_not_panic(
        "malformed annex",
        "package P public system S annex EMV2 {** this is not valid EMV2 **}; end S; end P;",
    );
}

#[test]
fn fuzz_annex_with_nested_braces() {
    parse_must_not_panic(
        "annex with nested braces",
        "package P public system S annex EMV2 {** { nested { braces } } **}; end S; end P;",
    );
}

#[test]
fn fuzz_unclosed_annex() {
    parse_must_not_panic(
        "unclosed annex",
        "package P public system S annex EMV2 {** not closed end S; end P;",
    );
}

// ── Repeated/duplicate declarations ─────────────────────────────────

#[test]
fn fuzz_duplicate_packages() {
    parse_must_not_panic(
        "duplicate packages",
        "package P public end P; package P public end P;",
    );
}

#[test]
fn fuzz_duplicate_components() {
    parse_must_not_panic(
        "duplicate components",
        "package P public system S end S; system S end S; end P;",
    );
}

// ── Mixed valid/invalid ─────────────────────────────────────────────

#[test]
fn fuzz_garbage_between_valid() {
    parse_must_not_panic(
        "garbage between valid decls",
        "package P public system S end S; @@@ $$$; system T end T; end P;",
    );
}

#[test]
fn fuzz_sql_injection() {
    parse_must_not_panic(
        "SQL injection attempt",
        "package P public system S'; DROP TABLE components;-- end S; end P;",
    );
}

// ── Very large input ────────────────────────────────────────────────

#[test]
fn fuzz_large_comment() {
    let comment = format!("-- {}\n", "x".repeat(100000));
    let input = format!("{comment}package P public end P;");
    parse_must_not_panic("100KB comment", &input);
}

#[test]
fn fuzz_repeated_keywords() {
    let input = "package ".repeat(10000);
    parse_must_not_panic("10000 repeated 'package'", &input);
}

// ── Property set edge cases ─────────────────────────────────────────

#[test]
fn fuzz_empty_property_set() {
    parse_must_not_panic("empty property set", "property set PS is end PS;");
}

#[test]
fn fuzz_property_set_with_many_props() {
    let mut input = String::from("property set PS is\n");
    for i in 0..200 {
        input.push_str(&format!("  Prop{i}: aadlinteger applies to (all);\n"));
    }
    input.push_str("end PS;\n");
    parse_must_not_panic("property set with 200 properties", &input);
}

// ── Connection edge cases ───────────────────────────────────────────

#[test]
fn fuzz_connection_missing_endpoints() {
    parse_must_not_panic(
        "connection missing endpoints",
        "package P public system implementation S.I connections c1: port -> ; end S.I; system S end S; end P;",
    );
}

#[test]
fn fuzz_deeply_nested_dotted_name() {
    let name = (0..50)
        .map(|i| format!("a{i}"))
        .collect::<Vec<_>>()
        .join(".");
    let input = format!(
        "package P public system implementation S.I connections c1: port {name} -> {name}; end S.I; system S end S; end P;"
    );
    parse_must_not_panic("50-deep dotted name in connection", &input);
}

// ── Stress: many files concept (simulated) ──────────────────────────

#[test]
fn fuzz_100_packages() {
    let mut input = String::new();
    for i in 0..100 {
        input.push_str(&format!(
            "package Pkg{i} public system Sys{i} end Sys{i}; end Pkg{i};\n"
        ));
    }
    parse_must_not_panic("100 packages in one file", &input);
}

// ── Flow spec edge cases ────────────────────────────────────────────

#[test]
fn fuzz_flow_without_endpoints() {
    parse_must_not_panic(
        "flow without endpoints",
        "package P public system S flows f1: flow source; end S; end P;",
    );
}

// ── Mode edge cases ─────────────────────────────────────────────────

#[test]
fn fuzz_many_modes() {
    let mut input = String::from("package P public system S modes\n");
    for i in 0..100 {
        if i == 0 {
            input.push_str(&format!("  m{i}: initial mode;\n"));
        } else {
            input.push_str(&format!("  m{i}: mode;\n"));
        }
    }
    input.push_str("end S; end P;\n");
    parse_must_not_panic("100 modes", &input);
}

// ── Entirely random bytes ───────────────────────────────────────────

#[test]
fn fuzz_random_ascii() {
    // Deterministic "random" for reproducibility
    let input: String = (0..5000)
        .map(|i| {
            let b = ((i * 7 + 13) % 95 + 32) as u8;
            b as char
        })
        .collect();
    parse_must_not_panic("5000 pseudo-random ASCII chars", &input);
}

#[test]
fn fuzz_alternating_valid_invalid() {
    let mut input = String::new();
    for i in 0..50 {
        if i % 2 == 0 {
            input.push_str(&format!(
                "package P{i} public system S{i} end S{i}; end P{i};\n"
            ));
        } else {
            input.push_str("@#$%^& !!! ???\n");
        }
    }
    parse_must_not_panic("alternating valid/invalid lines", &input);
}
