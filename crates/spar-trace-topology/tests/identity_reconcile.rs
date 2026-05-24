//! Integration test for the `IdentityUnknown` reconciliation check.
//!
//! Unit tests in `engine.rs` cover the pure check logic against a
//! hand-built [`DeclaredModel`]. This test exercises the other half: the
//! [`DeclaredModel::from_instance`] bridge that walks a *real*
//! instantiated AADL model and reads its `Spar_Identity::*` property
//! surface — the path the v0.11.0 CLI subcommand will use.

use spar_hir_def::{
    HirDefDatabase, Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope,
};
use spar_trace_topology::engine::{DeclaredModel, check_identity_unknown};
use spar_trace_topology::ingest::{
    CapturedFrame, FrameSource, IngestError, LldpNeighbor, TopologySource,
};
use spar_trace_topology::reconcile::ReconcileFinding;

/// Two devices, each declaring a `Spar_Identity::MAC_Address`; one also
/// declares a `Spar_Identity::LLDP_Chassis_Id`. The MAC on `ecu_b` is
/// written upper-case with `-` separators on purpose — the model must
/// fold it to canonical form.
const IDENT_FIXTURE: &str = r#"
package IdentFixture
public

  device ecu_a
    properties
      Spar_Identity::MAC_Address => "02:00:00:00:00:01";
  end ecu_a;
  device implementation ecu_a.impl
  end ecu_a.impl;

  device ecu_b
    properties
      Spar_Identity::MAC_Address     => "02-00-00-00-00-02";
      Spar_Identity::LLDP_Chassis_Id => "core-switch-1";
  end ecu_b;
  device implementation ecu_b.impl
  end ecu_b.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      a : device ecu_a.impl;
      b : device ecu_b.impl;
  end Sys.impl;
end IdentFixture;
"#;

fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(
        &db,
        "identity_reconcile.aadl".to_string(),
        aadl_src.to_string(),
    );
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

struct VecFrames(Vec<CapturedFrame>);

impl FrameSource for VecFrames {
    fn frames(&mut self) -> Box<dyn Iterator<Item = Result<CapturedFrame, IngestError>> + '_> {
        Box::new(std::mem::take(&mut self.0).into_iter().map(Ok))
    }
}

struct VecTopology(Vec<LldpNeighbor>);

impl TopologySource for VecTopology {
    fn neighbors(&self) -> &[LldpNeighbor] {
        &self.0
    }
}

fn frame(src: [u8; 6], dst: [u8; 6]) -> CapturedFrame {
    CapturedFrame {
        mac_src: src,
        mac_dst: dst,
        vlan_id: None,
        pcp: None,
        timestamp_ns: 0,
    }
}

#[test]
fn from_instance_reads_spar_identity_property_surface() {
    let inst = instantiate(IDENT_FIXTURE, "IdentFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);

    // Both declared MACs are recognised, case- and separator-insensitively.
    assert!(model.knows_mac("02:00:00:00:00:01"));
    assert!(model.knows_mac("02:00:00:00:00:02"));
    assert!(model.knows_chassis_id("CORE-SWITCH-1"));
    assert!(!model.knows_mac("02:00:00:00:00:ff"));
}

#[test]
fn declared_traffic_reconciles_clean() {
    let inst = instantiate(IDENT_FIXTURE, "IdentFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);

    let a = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    let b = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
    let mut frames = VecFrames(vec![frame(a, b)]);
    let topology = VecTopology(vec![]);

    let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn undeclared_station_raises_identity_unknown() {
    let inst = instantiate(IDENT_FIXTURE, "IdentFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);

    let declared = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
    let rogue = [0x02, 0x00, 0x00, 0x00, 0x00, 0xff];
    let mut frames = VecFrames(vec![frame(declared, rogue)]);
    let topology = VecTopology(vec![]);

    let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
    assert_eq!(
        findings,
        vec![ReconcileFinding::IdentityUnknown {
            observed: "02:00:00:00:00:ff".to_string(),
            context: "PCAPNG frame capture".to_string(),
        }]
    );
}
