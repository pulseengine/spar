//! Integration test for the `GptpOutOfBudget` reconciliation check.
//!
//! Unit tests in `engine.rs` cover the pure check logic against a
//! hand-built [`DeclaredModel`]. This test exercises the other half:
//! [`DeclaredModel::from_instance`] reading `Spar_TSN::Sync_Error` off a
//! real instantiated AADL model, with unit conversion (the property is
//! `aadlinteger units Time_Units` — a value written as `1000 ns` must
//! materialise as 1_000_000 picoseconds in the declared model).

use spar_hir_def::{
    HirDefDatabase, Name, file_item_tree, instance::SystemInstance, resolver::GlobalScope,
};
use spar_trace_topology::engine::{DeclaredModel, check_gptp_out_of_budget};
use spar_trace_topology::ingest::{PtpPortObservation, PtpSample, PtpTimeSource};
use spar_trace_topology::reconcile::ReconcileFinding;

const GPTP_FIXTURE: &str = r#"
package GptpFixture
public

  bus tsn_bus
    properties
      Spar_TSN::Sync_Error => 1000 ns;
  end tsn_bus;
  bus implementation tsn_bus.impl
  end tsn_bus.impl;

  system Sys
  end Sys;
  system implementation Sys.impl
    subcomponents
      net : bus tsn_bus.impl;
  end Sys.impl;
end GptpFixture;
"#;

fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(
        &db,
        "gptp_out_of_budget.aadl".to_string(),
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

struct VecPtp(Vec<PtpPortObservation>);

impl PtpTimeSource for VecPtp {
    fn grandmaster(&self) -> Option<&str> {
        None
    }
    fn domain(&self) -> Option<u8> {
        None
    }
    fn ports(&self) -> &[PtpPortObservation] {
        &self.0
    }
}

fn port(name: &str, sync_errors_ns: &[u64]) -> PtpPortObservation {
    PtpPortObservation {
        name: name.to_string(),
        samples: sync_errors_ns
            .iter()
            .enumerate()
            .map(|(i, &e)| PtpSample {
                timestamp_ns: i as u64,
                sync_error_ns: e,
            })
            .collect(),
    }
}

/// FQN spar produces for the bus subcomponent — `<system-impl>/<subcomp>`.
const BUS_PATH: &str = "Sys.impl/net";

#[test]
fn from_instance_reads_sync_error_with_unit_conversion() {
    let inst = instantiate(GPTP_FIXTURE, "GptpFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);

    // `Spar_TSN::Sync_Error => 1000 ns` must materialise as 1_000_000 ps.
    assert_eq!(model.knows_sync_budget(BUS_PATH), Some(1_000_000));
}

#[test]
fn observed_below_budget_reconciles_clean() {
    let inst = instantiate(GPTP_FIXTURE, "GptpFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);
    // Budget = 1_000_000 ps; worst observed = 500 ns × 1000 = 500_000 ps.
    let ptp = VecPtp(vec![port("swp1", &[200, 500, 300])]);

    let findings = check_gptp_out_of_budget(&model, &ptp);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn observed_above_budget_raises_gptp_out_of_budget() {
    let inst = instantiate(GPTP_FIXTURE, "GptpFixture", "Sys", "impl");
    let model = DeclaredModel::from_instance(&inst);
    // Worst observed = 1500 ns × 1000 = 1_500_000 ps > 1_000_000.
    let ptp = VecPtp(vec![port("swp1", &[1500])]);

    let findings = check_gptp_out_of_budget(&model, &ptp);
    assert_eq!(
        findings,
        vec![ReconcileFinding::GptpOutOfBudget {
            bus_or_processor: BUS_PATH.to_string(),
            budget_ps: 1_000_000,
            observed_ps: 1_500_000,
        }]
    );
}
