//! Property-based tests for the AADL parser.
//!
//! These tests use proptest to verify robustness properties of the parser:
//! 1. No panics on arbitrary input
//! 2. Lossless roundtrip (rowan preserves all source text)
//! 3. Valid AADL templates parse without errors

use proptest::prelude::*;

// ── Strategy helpers ────────────────────────────────────────────────

/// AADL keywords that cannot be used as identifiers.
const AADL_KEYWORDS: &[&str] = &[
    "aadl",
    "abstract",
    "access",
    "all",
    "and",
    "annex",
    "applies",
    "binding",
    "bus",
    "calls",
    "classifier",
    "compute",
    "connections",
    "constant",
    "data",
    "delta",
    "device",
    "end",
    "enumeration",
    "event",
    "extends",
    "false",
    "feature",
    "features",
    "flow",
    "flows",
    "group",
    "implementation",
    "in",
    "inherit",
    "initial",
    "inverse",
    "is",
    "list",
    "memory",
    "mode",
    "modes",
    "none",
    "not",
    "of",
    "or",
    "out",
    "package",
    "parameter",
    "path",
    "port",
    "private",
    "process",
    "processor",
    "properties",
    "property",
    "prototypes",
    "provides",
    "public",
    "range",
    "record",
    "reference",
    "refined",
    "renames",
    "requires",
    "self",
    "server",
    "set",
    "sink",
    "source",
    "subcomponents",
    "subprogram",
    "system",
    "thread",
    "to",
    "true",
    "type",
    "units",
    "virtual",
    "with",
];

/// Generate a valid AADL identifier that doesn't collide with keywords.
fn aadl_ident() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{2,19}".prop_filter("must not be AADL keyword", |s| {
        !AADL_KEYWORDS.contains(&s.to_lowercase().as_str())
    })
}

/// AADL component categories.
fn aadl_category() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("system"),
        Just("process"),
        Just("thread"),
        Just("thread group"),
        Just("data"),
        Just("subprogram"),
        Just("processor"),
        Just("memory"),
        Just("bus"),
        Just("device"),
        Just("abstract"),
        Just("virtual processor"),
        Just("virtual bus"),
    ]
}

// ── Test 1: No panics on arbitrary byte strings ─────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100)
    ))]

    /// Feeding arbitrary strings to the parser must never panic, even if the
    /// input is complete garbage. The parser may return errors, but it must
    /// always return a `Parse` value.
    #[test]
    fn no_panic_on_arbitrary_input(input in "\\PC{0,500}") {
        // This must not panic — that is the entire assertion.
        let parse = spar_syntax::parse(&input);
        // We can also access the syntax node without panicking.
        let _node = parse.syntax_node();
        let _errors = parse.errors();
    }

    /// Same as above but with raw byte-ish strings that include NUL,
    /// control characters, and multi-byte UTF-8.
    #[test]
    fn no_panic_on_arbitrary_unicode(input in ".{0,300}") {
        let parse = spar_syntax::parse(&input);
        let _node = parse.syntax_node();
    }
}

// ── Test 2: Lossless roundtrip (rowan property) ─────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100)
    ))]

    /// After parsing any input, the text of the root syntax node must be
    /// exactly the original source text. This is the fundamental lossless
    /// property of rowan-based parsers.
    #[test]
    fn roundtrip_preserves_source(input in "\\PC{0,500}") {
        let parse = spar_syntax::parse(&input);
        let root = parse.syntax_node();
        prop_assert_eq!(
            root.text().to_string(),
            input,
            "syntax tree text must match original source"
        );
    }
}

// ── Test 3: Valid AADL templates parse successfully ─────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(100)
    ))]

    /// Generate AADL package declarations from templates and verify they
    /// parse without errors. The template produces valid AADL:
    ///
    /// ```aadl
    /// package PkgName
    /// public
    ///   <category> TypeName
    ///   end TypeName;
    /// end PkgName;
    /// ```
    #[test]
    fn valid_aadl_package_template(
        pkg_name in aadl_ident(),
        type_name in aadl_ident(),
        category in aadl_category(),
    ) {
        let source = format!(
            "package {pkg}\npublic\n  {cat} {typ}\n  end {typ};\nend {pkg};\n",
            pkg = pkg_name,
            cat = category,
            typ = type_name,
        );

        let parse = spar_syntax::parse(&source);
        let root = parse.syntax_node();

        // Lossless roundtrip must hold for generated AADL too.
        prop_assert_eq!(
            root.text().to_string(),
            source.clone(),
            "roundtrip failed for generated AADL"
        );

        // A well-formed package should parse without errors.
        prop_assert!(
            parse.ok(),
            "parse errors for valid template: {:?}\nsource:\n{}",
            parse.errors(),
            source,
        );
    }

    /// Generate AADL type extensions and verify they parse.
    ///
    /// ```aadl
    /// package PkgName
    /// public
    ///   <category> TypeName extends BaseType
    ///   end TypeName;
    /// end PkgName;
    /// ```
    #[test]
    fn valid_aadl_extends_template(
        pkg_name in aadl_ident(),
        type_name in aadl_ident(),
        base_name in aadl_ident(),
        category in aadl_category(),
    ) {
        let source = format!(
            "package {pkg}\npublic\n  {cat} {typ} extends {base}\n  end {typ};\nend {pkg};\n",
            pkg = pkg_name,
            cat = category,
            typ = type_name,
            base = base_name,
        );

        let parse = spar_syntax::parse(&source);

        prop_assert_eq!(
            parse.syntax_node().text().to_string(),
            source.clone(),
            "roundtrip failed"
        );

        prop_assert!(
            parse.ok(),
            "parse errors for extends template: {:?}\nsource:\n{}",
            parse.errors(),
            source,
        );
    }
}
