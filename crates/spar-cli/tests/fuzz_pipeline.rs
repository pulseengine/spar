//! Adversarial/fuzz tests for the full AADL pipeline.
//!
//! These tests feed inputs through the ENTIRE pipeline:
//!   1. Parse (spar-syntax)
//!   2. HIR construction (spar-hir)
//!   3. Instantiation (spar-hir)
//!   4. Analysis (spar-analysis)
//!
//! Also tests the SysML v2 -> AADL -> analysis pipeline.
//!
//! Panics, hangs, and crashes are failures.

use std::time::{Duration, Instant};

/// Run the full AADL pipeline: parse -> hir -> instantiate -> analyze.
/// Returns a description of what happened (for debugging).
fn full_aadl_pipeline(label: &str, source: &str, root: &str) -> String {
    let start = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Step 1: Parse
        let _parsed = spar_syntax::parse(source);

        // Step 2: HIR
        let db = spar_hir::Database::from_aadl(&[("fuzz.aadl".to_string(), source.to_string())]);
        let packages = db.packages();

        // Step 3: Instantiate
        let instance = db.instantiate(root);

        // Step 4: Analyze (if instantiation succeeded)
        if let Some(inst) = &instance {
            let mut runner = spar_analysis::AnalysisRunner::new();
            runner.register_all();
            let diags = runner.run_all(inst.inner());
            format!("pkgs={}, inst=OK, diags={}", packages.len(), diags.len())
        } else {
            format!("pkgs={}, inst=None", packages.len())
        }
    }));
    let elapsed = start.elapsed();

    match result {
        Ok(status) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: took {elapsed:?}"
            );
            eprintln!("[PIPE]  {label}: {status} ({elapsed:?})");
            status
        }
        Err(panic_info) => {
            panic!("[PANIC] {label}: {panic_info:?}");
        }
    }
}

/// Run parse + HIR only (no instantiation needed).
fn aadl_hir_must_not_panic(label: &str, source: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let db = spar_hir::Database::from_aadl(&[("fuzz.aadl".to_string(), source.to_string())]);
        db.packages()
    }));
    let elapsed = start.elapsed();

    match result {
        Ok(pkgs) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: took {elapsed:?}"
            );
            eprintln!("[HIR]   {label}: {} packages ({elapsed:?})", pkgs.len());
        }
        Err(panic_info) => {
            panic!("[PANIC] {label}: {panic_info:?}");
        }
    }
}

/// Run the SysML v2 -> AADL -> analysis pipeline.
fn sysml2_to_aadl_pipeline(label: &str, sysml_source: &str) {
    let start = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Parse SysML v2
        let parse = spar_sysml2::parse(sysml_source);

        // Lower to AADL ItemTree
        let (tree, diags) = spar_sysml2::lower::lower_to_aadl_with_diagnostics(&parse);

        // Extract requirements
        let reqs_yaml = spar_sysml2::extract::extract_requirements(&parse);

        (
            parse.errors().len(),
            diags.len(),
            tree.packages.len(),
            reqs_yaml.len(),
        )
    }));
    let elapsed = start.elapsed();

    match result {
        Ok((parse_errs, lower_diags, pkgs, yaml_len)) => {
            assert!(
                elapsed < Duration::from_secs(10),
                "[HANG] {label}: took {elapsed:?}"
            );
            eprintln!(
                "[SYS2]  {label}: parse_errs={parse_errs}, lower_diags={lower_diags}, pkgs={pkgs}, yaml={yaml_len} ({elapsed:?})"
            );
        }
        Err(panic_info) => {
            panic!("[PANIC] {label}: {panic_info:?}");
        }
    }
}

// ═════════════════════���═════════════════════════════════════════════════
// AADL Full Pipeline Tests
// ════════════════════════════════��═════════════════════════════���════════

#[test]
fn pipeline_empty_file() {
    aadl_hir_must_not_panic("empty file", "");
}

#[test]
fn pipeline_whitespace() {
    aadl_hir_must_not_panic("whitespace", "   \n\t\n   ");
}

#[test]
fn pipeline_valid_minimal() {
    full_aadl_pipeline(
        "valid minimal",
        r#"package P public
  system S end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_valid_with_features() {
    full_aadl_pipeline(
        "valid with features",
        r#"package P public
  system S
    features
      inp: in data port;
      outp: out data port;
  end S;
  system implementation S.I
    connections
      c1: port inp -> outp;
  end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_valid_with_subcomponents() {
    full_aadl_pipeline(
        "valid with subcomponents",
        r#"package P public
  system Inner end Inner;
  system implementation Inner.I end Inner.I;
  system Outer end Outer;
  system implementation Outer.I
    subcomponents
      child1: system Inner.I;
      child2: system Inner.I;
  end Outer.I;
end P;"#,
        "P::Outer.I",
    );
}

#[test]
fn pipeline_circular_extends() {
    full_aadl_pipeline(
        "circular extends",
        "package P public system A extends B end A; system B extends A end B; system implementation A.I end A.I; end P;",
        "P::A.I",
    );
}

#[test]
fn pipeline_self_extending() {
    full_aadl_pipeline(
        "self-extending",
        "package P public system A extends A end A; system implementation A.I end A.I; end P;",
        "P::A.I",
    );
}

#[test]
fn pipeline_recursive_subcomponent() {
    // This could cause infinite recursion during instantiation
    full_aadl_pipeline(
        "recursive subcomponent",
        r#"package Rec public
  system S end S;
  system implementation S.I
    subcomponents child: system S.I;
  end S.I;
end Rec;"#,
        "Rec::S.I",
    );
}

#[test]
fn pipeline_deep_hierarchy() {
    // 20-level deep hierarchy
    let mut input = String::from("package Deep public\n");
    input.push_str("  system L0 end L0;\n");
    input.push_str("  system implementation L0.I end L0.I;\n");
    for i in 1..20 {
        input.push_str(&format!("  system L{i} end L{i};\n"));
        input.push_str(&format!(
            "  system implementation L{i}.I subcomponents child: system L{prev}.I; end L{i}.I;\n",
            prev = i - 1
        ));
    }
    input.push_str("end Deep;\n");
    full_aadl_pipeline("20-level deep hierarchy", &input, "Deep::L19.I");
}

#[test]
fn pipeline_nonexistent_root() {
    full_aadl_pipeline(
        "nonexistent root",
        "package P public system S end S; end P;",
        "P::NonExistent.I",
    );
}

#[test]
fn pipeline_invalid_root_format() {
    full_aadl_pipeline(
        "invalid root format",
        "package P public system S end S; end P;",
        "not-a-valid-root",
    );
}

#[test]
fn pipeline_empty_package() {
    full_aadl_pipeline("empty package", "package P public end P;", "P::S.I");
}

#[test]
fn pipeline_1000_components() {
    let mut input = String::from("package P public\n");
    for i in 0..1000 {
        input.push_str(&format!("  system S{i} end S{i};\n"));
    }
    input.push_str("  system implementation S0.I end S0.I;\n");
    input.push_str("end P;\n");
    full_aadl_pipeline("1000 components", &input, "P::S0.I");
}

#[test]
fn pipeline_many_connections() {
    let mut input = String::from("package P public\n");
    input.push_str("  system Inner features p: in out data port; end Inner;\n");
    input.push_str("  system implementation Inner.I end Inner.I;\n");
    input.push_str("  system Top end Top;\n");
    input.push_str("  system implementation Top.I\n    subcomponents\n");
    for i in 0..50 {
        input.push_str(&format!("      s{i}: system Inner.I;\n"));
    }
    input.push_str("    connections\n");
    for i in 0..49 {
        input.push_str(&format!(
            "      c{i}: port s{i}.p -> s{next}.p;\n",
            next = i + 1
        ));
    }
    input.push_str("  end Top.I;\nend P;\n");
    full_aadl_pipeline(
        "50 subcomponents with chain connections",
        &input,
        "P::Top.I",
    );
}

#[test]
fn pipeline_properties_stress() {
    full_aadl_pipeline(
        "massive property values",
        r#"package P public
  system S
    properties
      Period => 999999999999999999999999999999 ms;
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_annex_blocks() {
    full_aadl_pipeline(
        "EMV2 annex",
        r#"package P public
  system S
    annex EMV2 {**
      use types ErrorLib;
      error propagations
        outp: out propagation {ServiceError};
      end propagations;
    **};
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_malformed_annex() {
    full_aadl_pipeline(
        "malformed EMV2 annex",
        r#"package P public
  system S
    annex EMV2 {** this is garbage not valid EMV2 at all **};
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_keyword_names() {
    aadl_hir_must_not_panic(
        "keywords as identifiers",
        "package package public system system end end;",
    );
}

#[test]
fn pipeline_null_bytes() {
    aadl_hir_must_not_panic("null bytes", "package P\0\0 public end P;");
}

#[test]
fn pipeline_unicode() {
    aadl_hir_must_not_panic(
        "unicode",
        "package \u{00DC}n\u{00EF}c\u{00F6}d\u{00E9} public end \u{00DC}n\u{00EF}c\u{00F6}d\u{00E9};",
    );
}

#[test]
fn pipeline_binary_garbage() {
    let garbage: String = (0..256).map(|b| b as u8 as char).collect();
    aadl_hir_must_not_panic("binary garbage", &garbage);
}

#[test]
fn pipeline_modes() {
    full_aadl_pipeline(
        "modes",
        r#"package P public
  system S
    modes
      m1: initial mode;
      m2: mode;
      t1: m1 -[inp]-> m2;
    features
      inp: in event port;
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_many_modes() {
    let mut input = String::from("package P public\n  system S\n    modes\n");
    input.push_str("      m0: initial mode;\n");
    for i in 1..50 {
        input.push_str(&format!("      m{i}: mode;\n"));
    }
    input.push_str("  end S;\n  system implementation S.I end S.I;\nend P;\n");
    full_aadl_pipeline("50 modes", &input, "P::S.I");
}

#[test]
fn pipeline_process_thread() {
    full_aadl_pipeline(
        "process and thread",
        r#"package P public
  thread T
    properties
      Dispatch_Protocol => Periodic;
      Period => 10 ms;
      Compute_Execution_Time => 1 ms .. 5 ms;
  end T;
  thread implementation T.I end T.I;
  process Proc end Proc;
  process implementation Proc.I
    subcomponents
      t1: thread T.I;
  end Proc.I;
  system S end S;
  system implementation S.I
    subcomponents
      p1: process Proc.I;
  end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_bus_and_memory() {
    full_aadl_pipeline(
        "bus and memory",
        r#"package P public
  bus B end B;
  bus implementation B.I end B.I;
  memory M end M;
  memory implementation M.I end M.I;
  processor CPU end CPU;
  processor implementation CPU.I end CPU.I;
  system S end S;
  system implementation S.I
    subcomponents
      b1: bus B.I;
      m1: memory M.I;
      cpu1: processor CPU.I;
  end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_flows() {
    full_aadl_pipeline(
        "flows",
        r#"package P public
  system S
    features
      inp: in data port;
      outp: out data port;
    flows
      f1: flow source outp;
      f2: flow sink inp;
      f3: flow path inp -> outp;
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

#[test]
fn pipeline_feature_groups() {
    full_aadl_pipeline(
        "feature groups",
        r#"package P public
  feature group FG
    features
      d1: in data port;
      d2: out data port;
  end FG;
  system S
    features
      fg1: feature group FG;
  end S;
  system implementation S.I end S.I;
end P;"#,
        "P::S.I",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// SysML v2 -> AADL Pipeline Tests
// ═════════════════════════════════════════���═════════════════════════════

#[test]
fn sysml2_pipeline_empty() {
    sysml2_to_aadl_pipeline("empty", "");
}

#[test]
fn sysml2_pipeline_complex() {
    sysml2_to_aadl_pipeline(
        "complex model",
        r#"
package SensorSystem {
    import ISQ::*;
    attribute def Temperature;
    port def SensorPort { out item data : Temperature; }
    port def ProcessorPort { in item data : Temperature; }
    part def Sensor { port sensorOut : SensorPort; }
    part def Processor { port processorIn : ProcessorPort; }
    part def System {
        part sensor : Sensor;
        part processor : Processor;
        connect sensor.sensorOut to processor.processorIn;
    }
    requirement def LatencyReq { doc "Latency must be < 10ms" }
    satisfy LatencyReq by processor;
}
"#,
    );
}

#[test]
fn sysml2_pipeline_self_referential() {
    sysml2_to_aadl_pipeline("self-referential", "part def A { part b : A; }");
}

#[test]
fn sysml2_pipeline_deeply_nested() {
    let mut input = String::new();
    for i in 0..50 {
        input.push_str(&format!("package P{i} {{ "));
    }
    input.push_str("part def Inner { }");
    for _ in 0..50 {
        input.push_str(" }");
    }
    sysml2_to_aadl_pipeline("50 nested packages", &input);
}

#[test]
fn sysml2_pipeline_garbage() {
    let garbage: String = (0..256).map(|b| b as u8 as char).collect();
    sysml2_to_aadl_pipeline("binary garbage", &garbage);
}

#[test]
fn sysml2_pipeline_many_defs() {
    let mut input = String::from("package Big {\n");
    for i in 0..500 {
        input.push_str(&format!("  part def S{i} {{ }}\n"));
    }
    input.push_str("}\n");
    sysml2_to_aadl_pipeline("500 part defs", &input);
}

#[test]
fn sysml2_pipeline_many_requirements() {
    let mut input = String::from("package Reqs {\n");
    for i in 0..100 {
        input.push_str(&format!("  requirement def R{i} {{ doc \"Req {i}\" }}\n"));
    }
    for i in 0..50 {
        input.push_str(&format!("  satisfy R{i} by component{i};\n"));
    }
    input.push_str("}\n");
    sysml2_to_aadl_pipeline("100 requirements + 50 satisfy", &input);
}

#[test]
fn sysml2_pipeline_specialization_chain() {
    let mut input = String::from("package Chain {\n");
    input.push_str("  part def Base { }\n");
    for i in 1..30 {
        let prev = if i == 1 {
            "Base".to_string()
        } else {
            format!("Level{}", i - 1)
        };
        input.push_str(&format!("  part def Level{i} specializes {prev} {{ }}\n"));
    }
    input.push_str("}\n");
    sysml2_to_aadl_pipeline("30-long specialization chain", &input);
}
