//! Integration tests for `extract_network_graph`.
//!
//! Each test pipes a small inline AADL string through the spar-hir-def
//! pipeline (`file_item_tree` → `GlobalScope` → `SystemInstance::instantiate`)
//! and then asks `extract_network_graph` for the typed
//! [`spar_network::NetworkGraph`]. The fixtures are deliberately small
//! so the tests stay focused on the extractor's classification and link
//! emission logic rather than full AADL semantics.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::{HirDefDatabase, Name, file_item_tree, resolver::GlobalScope};
use spar_network::extract::extract_network_graph;
use spar_network::types::{NodeKind, SwitchType};

/// Parse an AADL source string and instantiate the named root system.
fn instantiate(aadl_src: &str, pkg: &str, sys: &str, sys_impl: &str) -> SystemInstance {
    let db = HirDefDatabase::default();
    let file = spar_base_db::SourceFile::new(&db, "fixture.aadl".to_string(), aadl_src.to_string());
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

/// Common AADL source: one FIFO switch carrying two end stations
/// connected by a single bus-access connection on each side.
const SIMPLE_FIFO_TOPOLOGY: &str = r#"
package Net
public

  bus eth_switch
    properties
      Spar_Network::Switch_Type        => FIFO;
      Spar_Network::Queue_Depth        => 32;
      Spar_Network::Forwarding_Latency => 5 us .. 10 us;
      Spar_Network::Output_Rate        => 100 MBytesps;
  end eth_switch;

  bus implementation eth_switch.impl
  end eth_switch.impl;

  device sensor
    features
      net : requires bus access;
  end sensor;

  device implementation sensor.impl
  end sensor.impl;

  device actuator
    features
      net : requires bus access;
  end actuator;

  device implementation actuator.impl
  end actuator.impl;

  system Sys
  end Sys;

  system implementation Sys.impl
    subcomponents
      sw  : bus eth_switch.impl;
      s   : device sensor.impl;
      a   : device actuator.impl;
    connections
      c1 : bus access sw -> s.net;
      c2 : bus access sw -> a.net;
  end Sys.impl;

end Net;
"#;

#[test]
fn simple_switched_topology() {
    let inst = instantiate(SIMPLE_FIFO_TOPOLOGY, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    // 1 switch + 2 end stations.
    assert_eq!(
        graph.switches().count(),
        1,
        "expected exactly one switch, got {}",
        graph.switches().count()
    );
    assert_eq!(
        graph.end_stations().count(),
        2,
        "expected exactly two end stations, got {}",
        graph.end_stations().count()
    );

    // 2 links (one per bus-access connection).
    assert_eq!(graph.links().len(), 2, "expected two links");

    // The single switch should be classified as FIFO.
    let sw = graph.switches().next().unwrap();
    assert_eq!(
        sw.kind,
        NodeKind::Switch {
            switch_type: SwitchType::Fifo
        }
    );
    assert_eq!(sw.name, "sw");
}

#[test]
fn priority_switch_classified_correctly() {
    let src = r#"
package Net
public

  bus prio_switch
    properties
      Spar_Network::Switch_Type => Priority;
  end prio_switch;

  bus implementation prio_switch.impl
  end prio_switch.impl;

  device d
    features
      net : requires bus access;
  end d;

  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;

  system implementation Sys.impl
    subcomponents
      sw : bus prio_switch.impl;
      x  : device d.impl;
    connections
      c1 : bus access sw -> x.net;
  end Sys.impl;

end Net;
"#;
    let inst = instantiate(src, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    let sw = graph
        .switches()
        .next()
        .expect("priority switch should be present");
    assert_eq!(
        sw.kind,
        NodeKind::Switch {
            switch_type: SwitchType::Priority
        }
    );
}

#[test]
fn tsn_switch_remains_opaque() {
    let src = r#"
package Net
public

  bus tsn_switch
    properties
      Spar_Network::Switch_Type        => TSN;
      Spar_Network::Queue_Depth        => 64;
      Spar_Network::Forwarding_Latency => 1 us .. 2 us;
      Spar_Network::Output_Rate        => 1000 MBytesps;
  end tsn_switch;

  bus implementation tsn_switch.impl
  end tsn_switch.impl;

  device sensor
    features
      net : requires bus access;
  end sensor;

  device implementation sensor.impl
  end sensor.impl;

  system Sys
  end Sys;

  system implementation Sys.impl
    subcomponents
      sw : bus tsn_switch.impl;
      s  : device sensor.impl;
    connections
      c1 : bus access sw -> s.net;
  end Sys.impl;

end Net;
"#;
    let inst = instantiate(src, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    let sw = graph
        .switches()
        .next()
        .expect("TSN switch should be classified");
    assert_eq!(
        sw.kind,
        NodeKind::Switch {
            switch_type: SwitchType::Tsn
        },
        "TSN must be classified even though Phase 1 leaves analysis opaque"
    );

    // The link to the end station still populates bandwidth/latency/queue
    // even though Phase 1 will not feed them through Network Calculus
    // service curves yet.
    let link = graph
        .links()
        .iter()
        .next()
        .expect("expected a link to the sensor");
    assert!(link.bandwidth_bps.is_some(), "Output_Rate should populate");
    assert!(
        link.forwarding_latency_ps.is_some(),
        "Forwarding_Latency should populate"
    );
    assert_eq!(link.queue_depth, Some(64));
}

#[test]
fn unannotated_bus_skipped() {
    // Same shape as the simple FIFO topology, except the bus carries no
    // Spar_Network properties. The extractor must treat it as a
    // classical (non-switched) AADL bus and skip it.
    let src = r#"
package Net
public

  bus plain_bus
  end plain_bus;

  bus implementation plain_bus.impl
  end plain_bus.impl;

  device d
    features
      net : requires bus access;
  end d;

  device implementation d.impl
  end d.impl;

  system Sys
  end Sys;

  system implementation Sys.impl
    subcomponents
      b : bus plain_bus.impl;
      x : device d.impl;
    connections
      c1 : bus access b -> x.net;
  end Sys.impl;

end Net;
"#;
    let inst = instantiate(src, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    assert_eq!(
        graph.switches().count(),
        0,
        "unannotated bus must not appear as a switch"
    );
    assert_eq!(
        graph.end_stations().count(),
        0,
        "without a switch there is no end station to register"
    );
    assert!(graph.links().is_empty(), "no links without a switch");
}

#[test]
fn forwarding_latency_range_propagates() {
    let inst = instantiate(SIMPLE_FIFO_TOPOLOGY, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    let link = graph.links().first().expect("expected at least one link");
    assert_eq!(
        link.forwarding_latency_ps,
        Some((5_000_000, 10_000_000)),
        "Forwarding_Latency => 5 us .. 10 us should lower to (5_000_000, 10_000_000) ps"
    );
}

#[test]
fn reachable_from_traverses_links() {
    let inst = instantiate(SIMPLE_FIFO_TOPOLOGY, "Net", "Sys", "impl");
    let graph = extract_network_graph(&inst);

    // From any node we should reach the full graph (1 switch + 2 stations).
    let any_node = graph.nodes().first().unwrap().idx;
    let reachable = graph.reachable_from(any_node);
    assert_eq!(
        reachable.len(),
        3,
        "expected to reach all 3 nodes from the single switch"
    );
}
