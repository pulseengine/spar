//! End-to-end oracle for EMV2 propagation-chain reporting
//! (REQ-EMV2-PROPAGATION-002 layer 4, GitHub #294).
//!
//! RED-BEFORE-GREEN: before L4 the analyze path fed `compute_error_propagation`
//! an EMPTY `Emv2Overlay` (`Emv2Overlay::new()`), so EVERY real model reported
//! "0 chain(s)". L4 builds the overlay from the EMV2 models parsed (L1),
//! retained on the item-tree (L2), and projected onto instances (L3), so a
//! source→sink error path across a connection now yields a chain.
//!
//! Non-circular: the model is parsed from real AADL through the whole pipeline
//! (parse → item-tree → instantiate → overlay → traversal); nothing is
//! hand-built. The chain forms because Sender's `out propagation {ServiceError}`
//! matches Receiver's `in propagation {ServiceError}` across the `dataxfer`
//! port connection (the traversal matches on error type across
//! semantic connections).

use spar_analysis::emv2_propagation::{
    Emv2Overlay, Emv2PropagationAnalysis, compute_error_propagation,
};
use spar_analysis::{Analysis, AnalysisDiagnostic};
use spar_hir_def::{
    HirDefDatabase, Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope,
};

fn instantiate(src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(&db, "emv2_chain.aadl".to_string(), src.to_string());
    let tree = file_item_tree(&db, file);
    let scope = GlobalScope::from_trees(vec![tree]);
    let inst = SystemInstance::instantiate(
        &scope,
        &Name::new(pkg),
        &Name::new(sys),
        &Name::new(sys_impl),
    );
    assert!(
        inst.component_count() > 0,
        "instantiation failed; diagnostics: {:?}",
        inst.diagnostics
    );
    inst
}

/// Two leaf systems directly connected by a port connection: Sender emits
/// `ServiceError` out its `output` port, Receiver receives it on `input`.
const CHAIN_AADL: &str = r#"package emv2_chain_pkg
public
  system Sender
    features
      output : out data port;
    annex EMV2 {**
error propagations
  output: out propagation {ServiceError};
end propagations;
    **};
  end Sender;

  system Receiver
    features
      input : in data port;
    annex EMV2 {**
error propagations
  input: in propagation {ServiceError};
end propagations;
    **};
  end Receiver;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      snd : system Sender;
      rcv : system Receiver;
    connections
      dataxfer : port snd.output -> rcv.input;
  end Top.impl;
end emv2_chain_pkg;
"#;

#[test]
fn emv2_propagation_reports_a_chain_across_a_port_connection() {
    let inst = instantiate(CHAIN_AADL, "emv2_chain_pkg", "Top", "impl");

    // Structural: the overlay built from the projected instance EMV2 yields a
    // source→sink chain. Empty on main before L4 (the #294 bug) ⇒ 0 chains.
    let overlay = Emv2Overlay::from_instance(&inst);
    let report = compute_error_propagation(&inst, &overlay);
    assert!(
        !report.chains.is_empty(),
        "expected >=1 EMV2 propagation chain, got 0 — the overlay is empty (#294)"
    );
    let chain = &report.chains[0];
    assert_eq!(chain.error_type.to_lowercase(), "serviceerror");
    assert_eq!(
        chain.downstream.len(),
        1,
        "exactly one downstream recipient (rcv)"
    );

    // Authoritative: the analyze pass emits the per-chain diagnostic that was
    // never produced for any real model before L4.
    let diags: Vec<AnalysisDiagnostic> = Emv2PropagationAnalysis.analyze(&inst);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("error propagation chain:")),
        "analyze must report the propagation chain; messages: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn empty_overlay_yields_no_chain_the_pre_l4_state() {
    // The pre-L4 behaviour, made permanent as the RED half of red-before-green:
    // an EMPTY overlay (`Emv2Overlay::new()`, exactly what `analyze` used before
    // #294 was fixed) yields 0 chains on the SAME instance that
    // `from_instance` turns into a chain. This is the delta L4 closes — proof
    // the passing chain test is not vacuous.
    let inst = instantiate(CHAIN_AADL, "emv2_chain_pkg", "Top", "impl");
    let report = compute_error_propagation(&inst, &Emv2Overlay::new());
    assert!(
        report.chains.is_empty(),
        "empty overlay must yield 0 chains — the #294 bug state"
    );
}

/// A component declaring error source/sink/path flows — exercises the flow
/// projection in `from_instance` (all three `ErrorFlowKind` arms), which the
/// propagation-only fixtures above don't touch. Flows surface in the report's
/// `local_flows` (they are not traversed into chains, per the pass design).
const FLOWS_AADL: &str = r#"package emv2_flows_pkg
public
  system Node
    features
      p_in : in data port;
      p_out : out data port;
    annex EMV2 {**
error propagations
  p_out: out propagation {ServiceError};
  p_in: in propagation {ServiceError};
flows
  f_src: error source p_out {ServiceError};
  f_snk: error sink p_in {ServiceError};
  f_pth: error path p_in {ServiceError} -> p_out;
end propagations;
    **};
  end Node;

  system Top
  end Top;

  system implementation Top.impl
    subcomponents
      n : system Node;
  end Top.impl;
end emv2_flows_pkg;
"#;

#[test]
fn emv2_flows_are_projected_into_local_flows() {
    let inst = instantiate(FLOWS_AADL, "emv2_flows_pkg", "Top", "impl");
    let overlay = Emv2Overlay::from_instance(&inst);
    let report = compute_error_propagation(&inst, &overlay);
    // The Node's source + sink + path flows are all projected.
    assert_eq!(
        report.local_flows.len(),
        3,
        "source + sink + path flows must be projected into local_flows: {:?}",
        report.local_flows
    );
    let kinds: std::collections::BTreeSet<&str> =
        report.local_flows.iter().map(|f| f.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["path", "sink", "source"].into_iter().collect(),
        "all three flow kinds must round-trip through from_instance"
    );
}

#[test]
fn emv2_negated_out_propagation_yields_no_chain() {
    // A `not out propagation` asserts the component does NOT emit the type;
    // projecting it must NOT fabricate a chain (from_instance skips negated).
    let src = CHAIN_AADL.replace("output: out propagation", "output: not out propagation");
    let inst = instantiate(&src, "emv2_chain_pkg", "Top", "impl");
    let overlay = Emv2Overlay::from_instance(&inst);
    let report = compute_error_propagation(&inst, &overlay);
    assert!(
        report.chains.is_empty(),
        "a negated out-propagation must not create a chain"
    );
}
