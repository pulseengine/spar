//! `analyze_aadl_from_fs` and `component_runner`, compiled natively
//! (REQ-WASM-COMPONENT-HEADLESS-001).
//!
//! The component's `analyze` export calls `analyze_aadl_from_fs`, which reads
//! `.aadl` files from the current directory. This binary holds one test, so
//! changing the process's directory cannot race another test. The headless
//! oracle (crates/spar-cli/tests/wasm_component_agreement.rs) runs the built
//! component only in CI; this case keeps the same function under the
//! ordinary test run.

const PLANTED: &str = "package Planted
public
  process Node
    features
      p : in data port;
  end Node;
  process implementation Node.impl
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      a : process Node.impl;
      b : process Node.impl;
    connections
      c : port a.p -> b.p;
  end Top.impl;
end Planted;
";

#[test]
fn analyze_from_fs_runs_the_component_runner_on_the_current_directory() {
    let names = spar_wasm::component_runner().names().join(",");
    let mut all = spar_analysis::AnalysisRunner::new();
    all.register_all();
    let all_names = all.names();
    for n in names.split(',') {
        assert!(all_names.contains(&n), "{n} is not in register_all");
    }

    let dir = std::env::temp_dir().join(format!("spar_wasm_fs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("planted.aadl"), PLANTED).unwrap();
    std::env::set_current_dir(&dir).unwrap();
    let diags = spar_wasm::analyze_aadl_from_fs("Planted::Top.impl").unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        diags.iter().any(|d| d.analysis == "direction_rules"
            && d.severity == spar_analysis::Severity::Error
            && d.message.contains("connection 'c'")),
        "the planted direction error is missing: {diags:#?}"
    );
    for d in &diags {
        assert!(
            names.split(',').any(|n| n == d.analysis),
            "{} is not one of component_runner()'s analyses",
            d.analysis
        );
    }
}
