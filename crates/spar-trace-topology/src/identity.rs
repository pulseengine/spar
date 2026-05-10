//! Typed accessors for the `Spar_Identity::*` property surface.
//!
//! Each accessor consults the typed [`PropertyExpr`] first and falls
//! back to the raw string blob, mirroring the pattern established by
//! `spar-network::tsn`. The accessors return `Option<T>` so callers
//! can distinguish "absent" from "malformed".
//!
//! See `docs/designs/v0.10.0-trace-topology.md` §"Spar_Identity
//! property surface" for the property semantics and reconciliation
//! intent.

use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::PropertyMap;

const SPAR_IDENTITY: &str = "Spar_Identity";

fn get_typed<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a PropertyExpr> {
    props
        .get_typed(SPAR_IDENTITY, name)
        .or_else(|| props.get_typed("", name))
}

fn get_raw<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a str> {
    props
        .get(SPAR_IDENTITY, name)
        .or_else(|| props.get("", name))
}

/// Strip surrounding quote characters from an `aadlstring` raw form.
///
/// String-fallback parsing sees the source-text-preserved value, which
/// for `aadlstring` properties typically retains the surrounding
/// double quotes. The typed path returns the unquoted contents
/// directly via `PropertyExpr::StringLit`.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_string()
}

/// Read [`Spar_Identity::MAC_Address`] — the canonical L2 MAC of a
/// device or processor as observed by PCAPNG/LLDP.
///
/// Returns the raw declared string (e.g. `"aa:bb:cc:dd:ee:ff"`); no
/// canonicalisation here — the v0.11.0 reconciliation engine
/// normalises before comparison.
pub fn get_mac_address(props: &PropertyMap) -> Option<String> {
    if let Some(PropertyExpr::StringLit(s)) = get_typed(props, "MAC_Address") {
        return Some(s.clone());
    }
    get_raw(props, "MAC_Address").map(unquote)
}

/// Read [`Spar_Identity::VLAN_ID`] — the 802.1Q VLAN ID of a
/// connection or bus, range `0..=4094`.
///
/// Values outside `0..=4094` (including the reserved `4095`) return
/// `None`.
pub fn get_vlan_id(props: &PropertyMap) -> Option<u16> {
    if let Some(expr) = get_typed(props, "VLAN_ID")
        && let PropertyExpr::Integer(v, _) = expr
        && (0..=4094).contains(v)
    {
        return Some(*v as u16);
    }
    let raw = get_raw(props, "VLAN_ID")?;
    let v: u16 = raw.trim().parse().ok()?;
    if v <= 4094 { Some(v) } else { None }
}

/// Read [`Spar_Identity::Stream_Handle`] — the 802.1Qcc CB stream
/// handle of a reserved connection. Returns `None` if the property
/// is unset, negative, or larger than `u32::MAX`.
pub fn get_stream_handle(props: &PropertyMap) -> Option<u32> {
    if let Some(expr) = get_typed(props, "Stream_Handle")
        && let PropertyExpr::Integer(v, _) = expr
        && *v >= 0
        && *v <= u32::MAX as i64
    {
        return Some(*v as u32);
    }
    let raw = get_raw(props, "Stream_Handle")?;
    raw.trim().parse::<u32>().ok()
}

/// Read [`Spar_Identity::Multicast_Group`] — the L2 destination
/// multicast MAC for a multicast stream. Raw declared form; no
/// canonicalisation.
pub fn get_multicast_group(props: &PropertyMap) -> Option<String> {
    if let Some(PropertyExpr::StringLit(s)) = get_typed(props, "Multicast_Group") {
        return Some(s.clone());
    }
    get_raw(props, "Multicast_Group").map(unquote)
}

/// Read [`Spar_Identity::LLDP_Chassis_Id`] — the LLDP chassis-id of
/// a device, processor, or bus endpoint as reported by the runtime
/// LLDP topology snapshot.
pub fn get_lldp_chassis_id(props: &PropertyMap) -> Option<String> {
    if let Some(PropertyExpr::StringLit(s)) = get_typed(props, "LLDP_Chassis_Id") {
        return Some(s.clone());
    }
    get_raw(props, "LLDP_Chassis_Id").map(unquote)
}

/// Read [`Spar_Identity::LLDP_Port_Id`] — the LLDP port-id of a bus
/// access feature as reported by the runtime LLDP snapshot.
pub fn get_lldp_port_id(props: &PropertyMap) -> Option<String> {
    if let Some(PropertyExpr::StringLit(s)) = get_typed(props, "LLDP_Port_Id") {
        return Some(s.clone());
    }
    get_raw(props, "LLDP_Port_Id").map(unquote)
}
