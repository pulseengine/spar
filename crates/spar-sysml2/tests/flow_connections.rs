//! SysML v2 flow usages lower to AADL port connections
//! (REQ-SYSML2-FLOWS-MODES-001, the flows half: connections).
//!
//! The oracle is the lowered AADL text plus the lowering diagnostics: a flow
//! whose ends do not resolve to a feature of the part or a port of one of
//! its parts is REPORTED and absent, never wired to a guessed feature.

use spar_sysml2::lower::{item_tree_to_aadl, lower_to_aadl_with_diagnostics};

fn lower(src: &str) -> (String, Vec<String>) {
    let wrapped = format!("package T {{ {src} }}");
    let parse = spar_sysml2::parse(&wrapped);
    assert!(parse.ok(), "`{wrapped}` must parse: {:?}", parse.errors());
    let (tree, diags) = lower_to_aadl_with_diagnostics(&parse);
    (
        item_tree_to_aadl(&tree),
        diags.into_iter().map(|d| d.msg).collect(),
    )
}

fn has(msgs: &[String], needle: &str) -> bool {
    msgs.iter().any(|m| m.contains(needle))
}

/// Types whose ports the flows below resolve against.
const TYPES: &str = "port def FuelPort; \
     part def Tank { port fuelTankPort : FuelPort; } \
     part def Eng { port engineFuelPort : FuelPort; }";

fn with_types(body: &str) -> String {
    format!("{TYPES} part def V {{ part tank : Tank; part eng : Eng; {body} }}")
}

// rivet: partially-verifies REQ-SYSML2-FLOWS-MODES-001
#[test]
fn a_named_flow_is_a_port_connection_under_its_name() {
    let (aadl, d) = lower(&with_types(
        "flow supply of Fuel from tank.fuelTankPort to eng.engineFuelPort;",
    ));
    assert!(
        aadl.contains("    supply : port tank.fuelTankPort -> eng.engineFuelPort;\n"),
        "{aadl}"
    );
    assert!(d.is_empty(), "{d:?}");
}

/// `training/13. Flows/Flow Usage Example`: the end names the item a port
/// carries; the AADL connection is between the ports.
#[test]
fn an_end_through_a_port_to_its_item_connects_the_port() {
    let (aadl, d) = lower(&with_types(
        "flow of Fuel from tank.fuelTankPort.fuelSupply to eng.engineFuelPort.fuelSupply;",
    ));
    assert!(
        aadl.contains("    tank_to_eng : port tank.fuelTankPort -> eng.engineFuelPort;\n"),
        "{aadl}"
    );
    assert!(!aadl.contains("fuelSupply"), "{aadl}");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn an_end_at_a_feature_of_the_part_itself_has_no_subcomponent() {
    let (aadl, d) = lower(&format!(
        "{TYPES} part def V {{ port feed : FuelPort; part eng : Eng; \
         flow feed to eng.engineFuelPort; }}"
    ));
    assert!(
        aadl.contains("    feed_to_eng : port feed -> eng.engineFuelPort;\n"),
        "{aadl}"
    );
    assert!(d.is_empty(), "{d:?}");
}

/// AADL connection names are unique in an implementation: a flow takes the
/// name a `connect` between the same parts would, made unique; an explicit
/// name that collides is made unique the same way.
#[test]
fn flow_names_stay_unique_beside_connects_and_each_other() {
    let (aadl, _) = lower(&with_types(
        "connect tank.fuelTankPort to eng.engineFuelPort; \
         flow tank.fuelTankPort to eng.engineFuelPort; \
         flow tank.fuelTankPort to eng.engineFuelPort; \
         flow tank_to_eng from tank.fuelTankPort to eng.engineFuelPort;",
    ));
    for n in [
        "tank_to_eng :",
        "tank_to_eng_2 :",
        "tank_to_eng_3 :",
        "tank_to_eng_4 :",
    ] {
        assert_eq!(aadl.matches(n).count(), 1, "`{n}`:\n{aadl}");
    }
}

#[test]
fn an_end_that_is_not_a_port_of_the_parts_type_is_reported_and_dropped() {
    let (aadl, d) = lower(&with_types("flow from tank.nope to eng.engineFuelPort;"));
    assert!(!aadl.contains("connections"), "{aadl}");
    assert!(has(&d, "`nope` is not a port of `tank`"), "{d:?}");
}

#[test]
fn an_end_below_a_part_of_unknown_type_is_lowered_only_when_it_names_a_feature() {
    // `Ext` is not in this model: `x.p` is read as its feature `p`, as a
    // `connect` would be; `x.p.i` cannot be told from a nested part's port.
    let (aadl, d) = lower("part def V { part x : Ext; part y : Ext; flow x.p to y.q; }");
    assert!(aadl.contains("    x_to_y : port x.p -> y.q;\n"), "{aadl}");
    assert!(d.is_empty(), "{d:?}");

    let (aadl, d) = lower("part def V { part x : Ext; part y : Ext; flow x.p.i to y.q; }");
    assert!(!aadl.contains("connections"), "{aadl}");
    assert!(has(&d, "the type of `x` is not in this model"), "{d:?}");
}

#[test]
fn an_end_that_names_no_feature_or_part_is_reported() {
    let (aadl, d) = lower(&with_types("flow ghost.p to eng.engineFuelPort;"));
    assert!(!aadl.contains("connections"), "{aadl}");
    assert!(
        has(
            &d,
            "`ghost` is neither a feature of this part nor one of its parts"
        ),
        "{d:?}"
    );

    let (_, d) = lower(&with_types("flow from tank to eng;"));
    assert!(has(&d, "`tank` is a part, not a port of one"), "{d:?}");
}

#[test]
fn a_flow_without_ends_is_reported() {
    let (aadl, d) = lower(&with_types("flow :>> m : M { end a; end b; }"));
    assert!(!aadl.contains("connections"), "{aadl}");
    assert!(has(&d, "only when it names its ends"), "{d:?}");
}

/// Only a flow directly in a part def body becomes a connection; one in a
/// part USAGE or an action body is reported where it sits, and so is a
/// `flow def`. A lowered flow is not ALSO reported as unsupported.
#[test]
fn a_flow_outside_a_part_def_and_a_flow_def_are_reported_and_a_lowered_one_is_not() {
    let (_, d) = lower(&with_types("flow tank.fuelTankPort to eng.engineFuelPort;"));
    assert!(!has(&d, "FLOW_USAGE"), "{d:?}");

    for src in [
        "part v { part a; part b; flow a.p to b.q; }",
        "action def A { action s; action t; flow s.o to t.i; }",
        "flow def F { end a; end b; }",
    ] {
        let (_, d) = lower(src);
        assert!(
            has(&d, "unsupported SysML v2 construct: FLOW_"),
            "`{src}`: {d:?}"
        );
    }
}

/// REGRESSION: `connect a.b.p` was lowered as `a.p`, a feature `a`'s type does
/// not have (`Annex A SimpleVehicleModel`: `connect rearAxle.leftHalfAxle.
/// axleToWheelPort to ...`). An AADL connection end is `part.feature`, so an
/// end below a part's own feature is reported and the connect is not lowered.
#[test]
fn a_connect_end_inside_a_nested_part_is_reported_not_shortened() {
    let src = "port def FP; part def Tank { port p : FP; } \
               part def Assy { part tank : Tank; } part def Eng { port q : FP; } \
               part def V { part assy : Assy; part eng : Eng; connect assy.tank.p to eng.q; }";
    let (aadl, d) = lower(src);
    assert!(!aadl.contains("assy.p"), "{aadl}");
    assert!(!aadl.contains("connections"), "{aadl}");
    assert!(
        has(&d, "`assy.tank.p` names a feature below `assy.tank`"),
        "{d:?}"
    );

    // Outside a part def the same end is reported the same way.
    let (aadl, d) = lower("connect a.b.c to d.e;");
    assert!(!aadl.contains("a.c"), "{aadl}");
    assert!(has(&d, "`a.b.c` names a feature below `a.b`"), "{d:?}");
}

/// The one- and two-segment ends a connect has always lowered stay lowered.
#[test]
fn a_connect_between_parts_ports_and_own_features_still_lowers() {
    let (aadl, d) = lower(&with_types(
        "port feed : FuelPort; connect tank.fuelTankPort to eng.engineFuelPort; \
         connect feed to eng.engineFuelPort;",
    ));
    assert!(
        aadl.contains("    tank_to_eng : port tank.fuelTankPort -> eng.engineFuelPort;\n"),
        "{aadl}"
    );
    assert!(
        aadl.contains("    feed_to_eng : port feed -> eng.engineFuelPort;\n"),
        "{aadl}"
    );
    assert!(!has(&d, "names a feature below"), "{d:?}");
}
