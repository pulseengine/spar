//! Round-trip tests for the `Spar_Identity::*` typed accessors.
//!
//! Each test exercises both the typed `PropertyExpr` path and the
//! string-fallback path so the v0.11.0 reconciliation engine can
//! depend on either parser-typed or hand-written test fixtures.

use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::name::{Name, PropertyRef};
use spar_hir_def::properties::{PropertyMap, PropertyValue};
use spar_trace_topology::identity;

fn make_props(set: &str, name: &str, value: &str, expr: Option<PropertyExpr>) -> PropertyMap {
    let mut props = PropertyMap::new();
    props.add(PropertyValue {
        name: PropertyRef {
            property_set: if set.is_empty() {
                None
            } else {
                Some(Name::new(set))
            },
            property_name: Name::new(name),
        },
        value: value.to_string(),
        typed_expr: expr,
        is_append: false,
    });
    props
}

#[test]
fn mac_address_typed_and_string_fallback() {
    // Typed PropertyExpr::StringLit path.
    let typed = make_props(
        "Spar_Identity",
        "MAC_Address",
        "\"aa:bb:cc:dd:ee:ff\"",
        Some(PropertyExpr::StringLit("aa:bb:cc:dd:ee:ff".to_string())),
    );
    assert_eq!(
        identity::get_mac_address(&typed).as_deref(),
        Some("aa:bb:cc:dd:ee:ff")
    );

    // String-fallback path — quoted source-text form.
    let raw = make_props(
        "Spar_Identity",
        "MAC_Address",
        "\"11:22:33:44:55:66\"",
        None,
    );
    assert_eq!(
        identity::get_mac_address(&raw).as_deref(),
        Some("11:22:33:44:55:66")
    );

    // Absent => None.
    let empty = PropertyMap::new();
    assert_eq!(identity::get_mac_address(&empty), None);
}

#[test]
fn vlan_id_in_and_out_of_range() {
    // Typed integer in range.
    let typed = make_props(
        "Spar_Identity",
        "VLAN_ID",
        "100",
        Some(PropertyExpr::Integer(100, None)),
    );
    assert_eq!(identity::get_vlan_id(&typed), Some(100));

    // Typed integer at the upper bound 4094 — accepted.
    let upper = make_props(
        "Spar_Identity",
        "VLAN_ID",
        "4094",
        Some(PropertyExpr::Integer(4094, None)),
    );
    assert_eq!(identity::get_vlan_id(&upper), Some(4094));

    // Typed integer 4095 (reserved) — rejected.
    let reserved = make_props(
        "Spar_Identity",
        "VLAN_ID",
        "4095",
        Some(PropertyExpr::Integer(4095, None)),
    );
    assert_eq!(identity::get_vlan_id(&reserved), None);

    // Negative value via typed path — rejected.
    let neg = make_props(
        "Spar_Identity",
        "VLAN_ID",
        "-1",
        Some(PropertyExpr::Integer(-1, None)),
    );
    assert_eq!(identity::get_vlan_id(&neg), None);

    // String-fallback in range.
    let raw = make_props("Spar_Identity", "VLAN_ID", "42", None);
    assert_eq!(identity::get_vlan_id(&raw), Some(42));

    // String-fallback out of range.
    let raw_oor = make_props("Spar_Identity", "VLAN_ID", "9999", None);
    assert_eq!(identity::get_vlan_id(&raw_oor), None);
}

#[test]
fn stream_handle_typed_and_string_fallback() {
    // Typed integer.
    let typed = make_props(
        "Spar_Identity",
        "Stream_Handle",
        "12345",
        Some(PropertyExpr::Integer(12345, None)),
    );
    assert_eq!(identity::get_stream_handle(&typed), Some(12345));

    // String-fallback.
    let raw = make_props("Spar_Identity", "Stream_Handle", "67890", None);
    assert_eq!(identity::get_stream_handle(&raw), Some(67890));

    // Negative => rejected (Stream_Handle is unsigned).
    let neg = make_props(
        "Spar_Identity",
        "Stream_Handle",
        "-5",
        Some(PropertyExpr::Integer(-5, None)),
    );
    assert_eq!(identity::get_stream_handle(&neg), None);
}

#[test]
fn multicast_group_typed_and_string_fallback() {
    let typed = make_props(
        "Spar_Identity",
        "Multicast_Group",
        "\"01:1b:19:00:00:00\"",
        Some(PropertyExpr::StringLit("01:1b:19:00:00:00".to_string())),
    );
    assert_eq!(
        identity::get_multicast_group(&typed).as_deref(),
        Some("01:1b:19:00:00:00")
    );

    let raw = make_props(
        "Spar_Identity",
        "Multicast_Group",
        "\"33:33:00:00:00:01\"",
        None,
    );
    assert_eq!(
        identity::get_multicast_group(&raw).as_deref(),
        Some("33:33:00:00:00:01")
    );
}

#[test]
fn lldp_chassis_id_typed_and_string_fallback() {
    let typed = make_props(
        "Spar_Identity",
        "LLDP_Chassis_Id",
        "\"ECU3\"",
        Some(PropertyExpr::StringLit("ECU3".to_string())),
    );
    assert_eq!(
        identity::get_lldp_chassis_id(&typed).as_deref(),
        Some("ECU3")
    );

    let raw = make_props("Spar_Identity", "LLDP_Chassis_Id", "\"sw-A\"", None);
    assert_eq!(identity::get_lldp_chassis_id(&raw).as_deref(), Some("sw-A"));
}

#[test]
fn lldp_port_id_typed_and_string_fallback() {
    let typed = make_props(
        "Spar_Identity",
        "LLDP_Port_Id",
        "\"eth0\"",
        Some(PropertyExpr::StringLit("eth0".to_string())),
    );
    assert_eq!(identity::get_lldp_port_id(&typed).as_deref(), Some("eth0"));

    let raw = make_props("Spar_Identity", "LLDP_Port_Id", "\"swp3\"", None);
    assert_eq!(identity::get_lldp_port_id(&raw).as_deref(), Some("swp3"));

    // Absent => None.
    let empty = PropertyMap::new();
    assert_eq!(identity::get_lldp_port_id(&empty), None);
}
