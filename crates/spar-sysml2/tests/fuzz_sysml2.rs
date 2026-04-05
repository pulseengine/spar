//! Adversarial/fuzz tests for the SysML v2 parser, lowering, and extraction.
//!
//! These tests feed malformed, edge-case, and adversarial inputs through:
//!   1. `spar_sysml2::parse()` -- the parser
//!   2. `spar_sysml2::lower::lower_to_aadl()` -- SysML v2 to AADL lowering
//!   3. `spar_sysml2::extract::extract_requirements()` -- requirements extraction
//!
//! Panics, hangs, and crashes are failures. Parse errors are acceptable.

use std::time::{Duration, Instant};

/// Parse SysML v2 input and assert no panic within timeout.
fn parse_must_not_panic(label: &str, input: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(|| spar_sysml2::parse(input));
    let elapsed = start.elapsed();

    match result {
        Ok(parse) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: parsing took {elapsed:?}"
            );
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
            panic!("[PANIC] {label} during parse: {panic_info:?}");
        }
    }
}

/// Parse + lower SysML v2 input and assert no panic.
fn lower_must_not_panic(label: &str, input: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(|| {
        let parse = spar_sysml2::parse(input);
        spar_sysml2::lower::lower_to_aadl(&parse)
    });
    let elapsed = start.elapsed();

    match result {
        Ok(_tree) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: lowering took {elapsed:?}"
            );
            eprintln!("[LOWER] {label} ({elapsed:?})");
        }
        Err(panic_info) => {
            panic!("[PANIC] {label} during lower: {panic_info:?}");
        }
    }
}

/// Parse + extract requirements from SysML v2 and assert no panic.
fn extract_must_not_panic(label: &str, input: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(|| {
        let parse = spar_sysml2::parse(input);
        spar_sysml2::extract::extract_requirements(&parse)
    });
    let elapsed = start.elapsed();

    match result {
        Ok(_) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: extraction took {elapsed:?}"
            );
            eprintln!("[EXTR]  {label} ({elapsed:?})");
        }
        Err(panic_info) => {
            panic!("[PANIC] {label} during extract: {panic_info:?}");
        }
    }
}

/// Run all three stages (parse, lower, extract) on the input.
fn full_pipeline_must_not_panic(label: &str, input: &str) {
    parse_must_not_panic(label, input);
    lower_must_not_panic(label, input);
    extract_must_not_panic(label, input);
}

// ── Basic edge cases ────────────────────────────────────────────────

#[test]
fn fuzz_empty() {
    full_pipeline_must_not_panic("empty", "");
}

#[test]
fn fuzz_whitespace() {
    full_pipeline_must_not_panic("whitespace", "   \n\t\n   ");
}

#[test]
fn fuzz_just_comment() {
    full_pipeline_must_not_panic("just comment", "// just a comment");
}

#[test]
fn fuzz_block_comment() {
    full_pipeline_must_not_panic("block comment", "/* block comment */");
}

#[test]
fn fuzz_unclosed_block_comment() {
    full_pipeline_must_not_panic("unclosed block comment", "/* never closed");
}

#[test]
fn fuzz_single_semicolon() {
    full_pipeline_must_not_panic("single semicolon", ";");
}

// ── Minimal constructs ──────────────────────────────────────────────

#[test]
fn fuzz_empty_package() {
    full_pipeline_must_not_panic("empty package", "package { }");
}

#[test]
fn fuzz_named_empty_package() {
    full_pipeline_must_not_panic("named empty package", "package Pkg { }");
}

#[test]
fn fuzz_package_semicolon() {
    full_pipeline_must_not_panic("package semicolon form", "package Pkg;");
}

#[test]
fn fuzz_empty_part_def() {
    full_pipeline_must_not_panic("empty part def", "part def { }");
}

#[test]
fn fuzz_empty_requirement_def() {
    full_pipeline_must_not_panic("empty requirement def", "requirement def { }");
}

#[test]
fn fuzz_empty_port_def() {
    full_pipeline_must_not_panic("empty port def", "port def { }");
}

#[test]
fn fuzz_empty_connection_def() {
    full_pipeline_must_not_panic("empty connection def", "connection def { }");
}

#[test]
fn fuzz_empty_attribute_def() {
    full_pipeline_must_not_panic("empty attribute def", "attribute def { }");
}

#[test]
fn fuzz_empty_action_def() {
    full_pipeline_must_not_panic("empty action def", "action def { }");
}

#[test]
fn fuzz_empty_state_def() {
    full_pipeline_must_not_panic("empty state def", "state def { }");
}

#[test]
fn fuzz_empty_enum_def() {
    full_pipeline_must_not_panic("empty enum def", "enum def { }");
}

#[test]
fn fuzz_empty_constraint_def() {
    full_pipeline_must_not_panic("empty constraint def", "constraint def { }");
}

#[test]
fn fuzz_empty_calc_def() {
    full_pipeline_must_not_panic("empty calc def", "calc def { }");
}

#[test]
fn fuzz_empty_allocation_def() {
    full_pipeline_must_not_panic("empty allocation def", "allocation def { }");
}

#[test]
fn fuzz_empty_interface_def() {
    full_pipeline_must_not_panic("empty interface def", "interface def { }");
}

#[test]
fn fuzz_empty_item_def() {
    full_pipeline_must_not_panic("empty item def", "item def { }");
}

// ── Self-referencing / circular ─────────────────────────────────────

#[test]
fn fuzz_self_specializing_part() {
    full_pipeline_must_not_panic("self-specializing part def", "part def A specializes A { }");
}

#[test]
fn fuzz_recursive_part_usage() {
    full_pipeline_must_not_panic("recursive part usage", "part def A { part b : A; }");
}

#[test]
fn fuzz_mutually_recursive() {
    full_pipeline_must_not_panic(
        "mutually recursive",
        r#"
package Cycle {
    part def A { part b : B; }
    part def B { part a : A; }
}
"#,
    );
}

// ── Incomplete / truncated ──────────────────────────────────────────

#[test]
fn fuzz_truncated_allocate() {
    full_pipeline_must_not_panic("truncated allocate", "allocate X to ;");
}

#[test]
fn fuzz_truncated_satisfy() {
    full_pipeline_must_not_panic("truncated satisfy", "satisfy by ;");
}

#[test]
fn fuzz_truncated_derive() {
    full_pipeline_must_not_panic("truncated derive", "derive from ;");
}

#[test]
fn fuzz_truncated_verify() {
    full_pipeline_must_not_panic("truncated verify", "verify by ;");
}

#[test]
fn fuzz_truncated_refine() {
    full_pipeline_must_not_panic("truncated refine", "refine by ;");
}

#[test]
fn fuzz_unclosed_brace() {
    full_pipeline_must_not_panic("unclosed brace", "package P {");
}

#[test]
fn fuzz_extra_close_brace() {
    full_pipeline_must_not_panic("extra close brace", "package P { } }");
}

#[test]
fn fuzz_mismatched_braces() {
    full_pipeline_must_not_panic("mismatched braces", "package P { part def A } { }");
}

// ── Quoted names ────────────────────────────────────────────────────

#[test]
fn fuzz_quoted_name() {
    full_pipeline_must_not_panic("quoted name", r#"requirement def "quoted name" { }"#);
}

#[test]
fn fuzz_empty_quoted_name() {
    full_pipeline_must_not_panic("empty quoted name", r#"part def "" { }"#);
}

#[test]
fn fuzz_quoted_name_with_escapes() {
    full_pipeline_must_not_panic(
        "quoted name with escapes",
        r#"part def "name with \"escapes\"" { }"#,
    );
}

// ── Unresolved references ───────────────────────────────────────────

#[test]
fn fuzz_unresolved_port_type() {
    full_pipeline_must_not_panic(
        "unresolved port type",
        "part def A { port p : NonExistent; }",
    );
}

#[test]
fn fuzz_unresolved_specialization() {
    full_pipeline_must_not_panic(
        "unresolved specialization",
        "part def A specializes DoesNotExist { }",
    );
}

#[test]
fn fuzz_connect_nonexistent() {
    full_pipeline_must_not_panic(
        "connect nonexistent ports",
        "connect nonExistent1.p to nonExistent2.q;",
    );
}

// ── Deeply nested packages ──────────────────────────────────────────

#[test]
fn fuzz_100_nested_packages() {
    let mut input = String::new();
    for i in 0..100 {
        input.push_str(&format!("package P{i} {{ "));
    }
    input.push_str("part def Inner { }");
    for _ in 0..100 {
        input.push_str(" }");
    }
    full_pipeline_must_not_panic("100 nested packages", &input);
}

#[test]
fn fuzz_50_nested_part_defs() {
    let mut input = String::new();
    for i in 0..50 {
        input.push_str(&format!("part def P{i} {{ "));
    }
    input.push_str("attribute x : Integer;");
    for _ in 0..50 {
        input.push_str(" }");
    }
    full_pipeline_must_not_panic("50 nested part defs", &input);
}

// ── Long identifiers ───────────────────────────────────────────────

#[test]
fn fuzz_long_identifier() {
    let name = "A".repeat(10000);
    let input = format!("part def {name} {{ }}");
    full_pipeline_must_not_panic("10000-char identifier", &input);
}

#[test]
fn fuzz_very_long_identifier() {
    let name = "B".repeat(100000);
    let input = format!("part def {name} {{ }}");
    full_pipeline_must_not_panic("100000-char identifier", &input);
}

// ── Null bytes and binary ───────────────────────────────────────────

#[test]
fn fuzz_null_bytes() {
    full_pipeline_must_not_panic("null bytes", "package P\0\0 { }");
}

#[test]
fn fuzz_binary_garbage() {
    let garbage: String = (0..256).map(|b| b as u8 as char).collect();
    full_pipeline_must_not_panic("binary garbage (0-255)", &garbage);
}

#[test]
fn fuzz_all_zeros() {
    let zeros = "\0".repeat(1000);
    full_pipeline_must_not_panic("1000 null bytes", &zeros);
}

// ── Unicode ─────────────────────────────────────────────────────────

#[test]
fn fuzz_unicode_names() {
    full_pipeline_must_not_panic(
        "unicode names",
        "part def \u{00DC}n\u{00EF}c\u{00F6}d\u{00E9} { }",
    );
}

#[test]
fn fuzz_emoji() {
    full_pipeline_must_not_panic("emoji", "part def \u{1F680} { }");
}

#[test]
fn fuzz_bom() {
    full_pipeline_must_not_panic("BOM prefix", "\u{FEFF}package Pkg { part def A { } }");
}

// ── Many declarations ───────────────────────────────────────────────

#[test]
fn fuzz_1000_part_defs() {
    let mut input = String::from("package Big {\n");
    for i in 0..1000 {
        input.push_str(&format!("  part def S{i} {{ }}\n"));
    }
    input.push_str("}\n");
    full_pipeline_must_not_panic("1000 part defs", &input);
}

#[test]
fn fuzz_500_ports() {
    let mut input = String::from("part def ManyPorts {\n");
    for i in 0..500 {
        input.push_str(&format!("  port p{i} : SomePort;\n"));
    }
    input.push_str("}\n");
    full_pipeline_must_not_panic("500 ports", &input);
}

#[test]
fn fuzz_200_connections() {
    let mut input = String::from("package Net {\n  part def Hub {\n");
    for i in 0..200 {
        input.push_str(&format!("    connect a{i}.p to b{i}.q;\n"));
    }
    input.push_str("  }\n}\n");
    full_pipeline_must_not_panic("200 connections", &input);
}

#[test]
fn fuzz_100_requirements() {
    let mut input = String::from("package Reqs {\n");
    for i in 0..100 {
        input.push_str(&format!(
            "  requirement def Req{i} {{ doc \"Requirement {i}\" }}\n"
        ));
    }
    input.push_str("}\n");
    full_pipeline_must_not_panic("100 requirements", &input);
}

// ── Mixed valid/invalid ─────────────────────────────────────────────

#[test]
fn fuzz_garbage_between_valid() {
    full_pipeline_must_not_panic(
        "garbage between valid",
        "part def A { } @@@ $$$ part def B { }",
    );
}

#[test]
fn fuzz_sql_injection() {
    full_pipeline_must_not_panic("SQL injection", "part def A { }; DROP TABLE parts;--");
}

#[test]
fn fuzz_html_injection() {
    full_pipeline_must_not_panic(
        "HTML injection",
        "part def A { doc \"<script>alert('xss')</script>\" }",
    );
}

// ── Repeated keywords ───────────────────────────────────────────────

#[test]
fn fuzz_repeated_part_keyword() {
    full_pipeline_must_not_panic("repeated part keyword", "part part part part def A { }");
}

#[test]
fn fuzz_repeated_def_keyword() {
    full_pipeline_must_not_panic("repeated def keyword", "part def def def A { }");
}

#[test]
fn fuzz_only_keywords() {
    full_pipeline_must_not_panic(
        "only keywords",
        "package part def port connection attribute requirement",
    );
}

// ── Large comments ──────────────────────────────────────────────────

#[test]
fn fuzz_large_line_comment() {
    let comment = format!("// {}\n", "x".repeat(100000));
    let input = format!("{comment}part def A {{ }}");
    full_pipeline_must_not_panic("100KB line comment", &input);
}

#[test]
fn fuzz_large_block_comment() {
    let comment = format!("/* {} */", "y".repeat(100000));
    let input = format!("{comment} part def A {{ }}");
    full_pipeline_must_not_panic("100KB block comment", &input);
}

// ── Random ASCII / deterministic fuzz ───────────────────────────────

#[test]
fn fuzz_random_ascii() {
    let input: String = (0..5000)
        .map(|i| {
            let b = ((i * 7 + 13) % 95 + 32) as u8;
            b as char
        })
        .collect();
    full_pipeline_must_not_panic("5000 pseudo-random ASCII", &input);
}

#[test]
fn fuzz_alternating_valid_invalid() {
    let mut input = String::new();
    for i in 0..50 {
        if i % 2 == 0 {
            input.push_str(&format!("part def P{i} {{ }}\n"));
        } else {
            input.push_str("@#$%^& !!! ???\n");
        }
    }
    full_pipeline_must_not_panic("alternating valid/invalid", &input);
}

// ── Complex valid model through full pipeline ───────────────────────

#[test]
fn fuzz_complex_valid_model() {
    let source = r#"
package SensorSystem {
    import ISQ::*;

    attribute def Temperature;
    attribute def Pressure;

    port def SensorPort {
        out item data : Temperature;
    }

    port def ProcessorPort {
        in item data : Temperature;
    }

    part def Sensor {
        port sensorOut : SensorPort;
        attribute accuracy : Real;
    }

    part def Processor {
        port processorIn : ProcessorPort;
    }

    connection def SensorConnection {
        connect source.sensorOut to target.processorIn;
    }

    part def SensorSystem {
        part sensor : Sensor;
        part processor : Processor;
        connect sensor.sensorOut to processor.processorIn;
    }

    requirement def LatencyReq {
        doc "System latency must be below 10ms"
        attribute maxLatency : Real;
    }

    requirement safetyReq : LatencyReq;
    satisfy LatencyReq by processor;
    verify LatencyReq by safetyTest;

    action def ProcessData { }
    state def Operational { }
    enum def Status { }
    constraint def TimingConstraint { }
    calc def TotalMass { }
    allocation def TaskAlloc { }
    interface def SensorInterface { }
}
"#;
    full_pipeline_must_not_panic("complex valid model", source);
}

// ── Specialization chains ───────────────────────────────────────────

#[test]
fn fuzz_long_specialization_chain() {
    let mut input = String::from("package Chain {\n");
    input.push_str("  part def Base { }\n");
    for i in 1..50 {
        input.push_str(&format!(
            "  part def Level{i} specializes Level{prev} {{ }}\n",
            prev = i - 1
        ));
    }
    // First level specializes Base
    input = input.replace("specializes Level0", "specializes Base");
    input.push_str("}\n");
    full_pipeline_must_not_panic("50-long specialization chain", &input);
}

// ── Edge case: only semicolons ──────────────────────────────────────

#[test]
fn fuzz_many_semicolons() {
    let input = ";".repeat(10000);
    full_pipeline_must_not_panic("10000 semicolons", &input);
}

// ── Edge case: only braces ──────────────────────────────────────────

#[test]
fn fuzz_many_open_braces() {
    let input = "{".repeat(1000);
    full_pipeline_must_not_panic("1000 open braces", &input);
}

#[test]
fn fuzz_many_close_braces() {
    let input = "}".repeat(1000);
    full_pipeline_must_not_panic("1000 close braces", &input);
}

#[test]
fn fuzz_balanced_empty_braces() {
    let input = "{}".repeat(5000);
    full_pipeline_must_not_panic("5000 balanced empty braces", &input);
}

// ── Edge case: doc strings ──────────────────────────────────────────

#[test]
fn fuzz_unclosed_string() {
    full_pipeline_must_not_panic("unclosed string", r#"part def A { doc "never closed }"#);
}

#[test]
fn fuzz_empty_doc() {
    full_pipeline_must_not_panic("empty doc", r#"part def A { doc "" }"#);
}

#[test]
fn fuzz_very_long_doc() {
    let long_text = "a".repeat(50000);
    let input = format!(r#"part def A {{ doc "{long_text}" }}"#);
    full_pipeline_must_not_panic("50000-char doc string", &input);
}
