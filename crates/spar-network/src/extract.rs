//! Extract a [`NetworkGraph`] from a [`SystemInstance`] by walking the
//! bus and device components and reading their `Spar_Network::*`
//! properties.
//!
//! This is the bridge between the AADL ItemTree and the WCTT analysis
//! that lands in later Track D commits. The algorithm is intentionally
//! lightweight: it walks the existing [`SystemInstance`] data, classifies
//! buses by `Switch_Type`, classifies devices/processors connected to
//! those buses as end stations, and emits one [`NetworkLink`] per AADL
//! connection that traverses a switched bus.
//!
//! We **complement** rather than duplicate `bus_bandwidth.rs` and
//! `connectivity.rs` in `spar-analysis`: those passes inspect the same
//! AADL data for different purposes (capacity check, dangling
//! connections), whereas this module distills the data into the typed
//! shape required by Network Calculus.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, PropertyExpr};
use spar_hir_def::name::Name;
use spar_hir_def::properties::PropertyMap;
use spar_hir_def::property_value::parse_time_value;

use crate::types::{NetworkGraph, NetworkLink, NetworkNode, NodeKind, SwitchType};

const SPAR_NETWORK: &str = "Spar_Network";

// ── Property accessors ───────────────────────────────────────────────
//
// Mirrors the typed-first / string-fallback idiom from
// `spar-analysis::property_accessors`. We re-implement the small set
// we need locally rather than introducing a `spar-analysis -> spar-network`
// dependency in the wrong direction.

/// Get a typed [`PropertyExpr`] for a `Spar_Network::<name>` property,
/// falling back to an unqualified lookup.
fn get_typed_spar_network<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a PropertyExpr> {
    props
        .get_typed(SPAR_NETWORK, name)
        .or_else(|| props.get_typed("", name))
}

/// Get the raw string value for a `Spar_Network::<name>` property,
/// falling back to an unqualified lookup.
fn get_raw_spar_network<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a str> {
    props
        .get(SPAR_NETWORK, name)
        .or_else(|| props.get("", name))
}

/// Read `Spar_Network::Switch_Type` and return the matched [`SwitchType`].
///
/// Returns `None` if the property is unset, has a value not recognised
/// by [`SwitchType::from_aadl_enum`], or is a typed expression that is
/// neither a [`PropertyExpr::Enum`] nor a [`PropertyExpr::StringLit`].
pub fn read_switch_type(props: &PropertyMap) -> Option<SwitchType> {
    if let Some(expr) = get_typed_spar_network(props, "Switch_Type") {
        match expr {
            PropertyExpr::Enum(name) => return SwitchType::from_aadl_enum(name.as_str()),
            PropertyExpr::StringLit(s) => return SwitchType::from_aadl_enum(s),
            _ => { /* fall through to string */ }
        }
    }

    let raw = get_raw_spar_network(props, "Switch_Type")?;
    SwitchType::from_aadl_enum(raw.trim().trim_matches('"'))
}

/// Read `Spar_Network::Queue_Depth` as a frame count.
pub fn read_queue_depth(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed_spar_network(props, "Queue_Depth")
        && let PropertyExpr::Integer(v, _) = expr
        && *v >= 0
    {
        return Some(*v as u64);
    }

    let raw = get_raw_spar_network(props, "Queue_Depth")?;
    raw.trim().parse::<u64>().ok()
}

/// Read `Spar_Network::Forwarding_Latency` as a `(BCET, WCET)` range in
/// picoseconds.
pub fn read_forwarding_latency_ps(props: &PropertyMap) -> Option<(u64, u64)> {
    if let Some(expr) = get_typed_spar_network(props, "Forwarding_Latency") {
        if let PropertyExpr::Range { min, max, .. } = expr {
            let min_ps = extract_time_ps(min)?;
            let max_ps = extract_time_ps(max)?;
            return Some((min_ps, max_ps));
        }
        if let Some(v) = extract_time_ps(expr) {
            return Some((v, v));
        }
    }

    let raw = get_raw_spar_network(props, "Forwarding_Latency")?;
    if let Some((min_str, max_str)) = raw.split_once("..") {
        let min_ps = parse_time_value(min_str.trim())?;
        let max_ps = parse_time_value(max_str.trim())?;
        Some((min_ps, max_ps))
    } else {
        let v = parse_time_value(raw)?;
        Some((v, v))
    }
}

/// Read `Spar_Network::Output_Rate` as bandwidth in bits per second.
///
/// AADL's `Data_Rate` is a unit-typed integer (`aadlinteger units
/// Data_Rate_Units`). We accept the common units shipped by the AADL
/// project (`bitsps`, `Bytesps`, `KBytesps`, `MBytesps`, `GBytesps`)
/// case-insensitively. A bare integer is interpreted as bits per second.
pub fn read_output_rate_bps(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed_spar_network(props, "Output_Rate")
        && let Some(bps) = extract_data_rate_bps(expr)
    {
        return Some(bps);
    }

    let raw = get_raw_spar_network(props, "Output_Rate")?;
    parse_data_rate_bps(raw)
}

/// Convert a typed [`PropertyExpr`] for time into picoseconds.
fn extract_time_ps(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, Some(unit)) => {
            let factor = time_unit_factor(unit.as_str())?;
            Some((*v as u64).checked_mul(factor)?)
        }
        PropertyExpr::Integer(v, None) => {
            if *v >= 0 {
                Some(*v as u64)
            } else {
                None
            }
        }
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            let factor = time_unit_factor(unit.as_str())?;
            Some((v * factor as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = time_unit_factor(unit.as_str())?;
            match inner.as_ref() {
                PropertyExpr::Integer(v, _) => Some((*v as u64).checked_mul(factor)?),
                PropertyExpr::Real(s, _) => {
                    let v: f64 = s.parse().ok()?;
                    Some((v * factor as f64) as u64)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

const TIME_UNIT_FACTORS_PS: &[(&str, u64)] = &[
    ("ps", 1),
    ("ns", 1_000),
    ("us", 1_000_000),
    ("ms", 1_000_000_000),
    ("sec", 1_000_000_000_000),
    ("min", 60_000_000_000_000),
    ("hr", 3_600_000_000_000_000),
];

fn time_unit_factor(unit: &str) -> Option<u64> {
    TIME_UNIT_FACTORS_PS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

const DATA_RATE_UNITS_BPS: &[(&str, u64)] = &[
    ("bitsps", 1),
    ("Bytesps", 8),
    ("KBytesps", 8 * 1024),
    ("MBytesps", 8 * 1024 * 1024),
    ("GBytesps", 8 * 1024 * 1024 * 1024),
];

fn data_rate_unit_factor(unit: &str) -> Option<u64> {
    DATA_RATE_UNITS_BPS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

fn extract_data_rate_bps(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, Some(unit)) if *v >= 0 => {
            let factor = data_rate_unit_factor(unit.as_str())?;
            (*v as u64).checked_mul(factor)
        }
        PropertyExpr::Integer(v, None) if *v >= 0 => Some(*v as u64),
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            if v < 0.0 {
                return None;
            }
            let factor = data_rate_unit_factor(unit.as_str())?;
            Some((v * factor as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = data_rate_unit_factor(unit.as_str())?;
            match inner.as_ref() {
                PropertyExpr::Integer(v, _) if *v >= 0 => (*v as u64).checked_mul(factor),
                PropertyExpr::Real(s, _) => {
                    let v: f64 = s.parse().ok()?;
                    if v < 0.0 {
                        return None;
                    }
                    Some((v * factor as f64) as u64)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_data_rate_bps(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit, factor) in DATA_RATE_UNITS_BPS {
        if let Some(num_str) = s.strip_suffix(unit).map(|s| s.trim()) {
            if let Ok(v) = num_str.parse::<u64>() {
                return v.checked_mul(factor);
            }
            if let Ok(v) = num_str.parse::<f64>() {
                if v < 0.0 {
                    return None;
                }
                return Some((v * factor as f64) as u64);
            }
        }
    }
    s.parse::<u64>().ok()
}

// ── Graph extraction ─────────────────────────────────────────────────

/// Extract a [`NetworkGraph`] from an AADL [`SystemInstance`].
///
/// Algorithm:
/// 1. Walk all components. Buses (and virtual buses) carrying a
///    recognised `Spar_Network::Switch_Type` are classified as
///    [`NodeKind::Switch`]. Buses without `Switch_Type` are skipped —
///    they are classical (unswitched) AADL buses that other passes
///    such as `bus_bandwidth.rs` already handle.
/// 2. For each switch, walk the connections owned by its parent
///    component. A connection that touches the switch on one side and
///    a `device` or `processor` subcomponent on the other side
///    contributes a [`NetworkLink`] and registers the device/processor
///    as a [`NodeKind::EndStation`].
/// 3. Each link copies the switch's `Output_Rate`, `Forwarding_Latency`
///    and `Queue_Depth` annotations. Missing annotations propagate as
///    `None` — diagnostic emission is the WCTT pass's job, not ours.
pub fn extract_network_graph(instance: &SystemInstance) -> NetworkGraph {
    let mut graph = NetworkGraph::default();

    // Pass 1: identify switches (buses carrying Switch_Type).
    let mut switches: Vec<(ComponentInstanceIdx, SwitchType)> = Vec::new();
    for (idx, comp) in instance.all_components() {
        if !is_bus_category(comp.category) {
            continue;
        }
        let props = instance.properties_for(idx);
        if let Some(switch_type) = read_switch_type(props) {
            switches.push((idx, switch_type));
            graph.nodes.push(NetworkNode {
                idx,
                kind: NodeKind::Switch { switch_type },
                name: comp.name.as_str().to_string(),
            });
        }
    }

    // Pass 2: for each switch, gather links + end stations from
    // connections in the parent component. We register each end station
    // at most once even if it has multiple connections to the same
    // switch.
    let mut end_stations_seen: Vec<ComponentInstanceIdx> = Vec::new();
    for (bus_idx, _switch_type) in &switches {
        let bus_idx = *bus_idx;
        let bus_props = instance.properties_for(bus_idx);
        let bandwidth_bps = read_output_rate_bps(bus_props);
        let forwarding_latency_ps = read_forwarding_latency_ps(bus_props);
        let queue_depth = read_queue_depth(bus_props);

        // The parent of a bus subcomponent is the system that owns it;
        // that system's connections are where we look for links to the
        // bus.
        let bus_comp = instance.component(bus_idx);
        let parent_idx = match bus_comp.parent {
            Some(p) => p,
            None => continue,
        };
        let parent_comp = instance.component(parent_idx);
        let bus_name = bus_comp.name.as_str();

        for &conn_idx in &parent_comp.connections {
            let conn = &instance.connections[conn_idx];

            let src_idx = conn
                .src
                .as_ref()
                .and_then(|end| resolve_subcomponent(instance, parent_idx, &end.subcomponent));
            let dst_idx = conn
                .dst
                .as_ref()
                .and_then(|end| resolve_subcomponent(instance, parent_idx, &end.subcomponent));

            // Determine which endpoint touches the bus and which
            // touches an end station.
            let (other_idx, src_is_bus) = match (src_idx, dst_idx) {
                (Some(s), Some(d)) if s == bus_idx => (d, true),
                (Some(s), Some(d)) if d == bus_idx => (s, false),
                _ => {
                    // Fall back to a name-based check for connections
                    // whose endpoint omits the subcomponent (i.e.,
                    // references the owner's own feature). This is
                    // rare in switched topologies but cheap to handle.
                    if let Some(other) = name_other_endpoint(instance, parent_idx, conn, bus_name) {
                        // We don't know which side was the bus;
                        // assume src is the bus to keep direction
                        // stable. A later commit (the WCTT pass) can
                        // refine this when the connection is
                        // bidirectional.
                        (other, true)
                    } else {
                        continue;
                    }
                }
            };

            if !is_end_station_category(instance.component(other_idx).category) {
                continue;
            }

            let (from, to) = if src_is_bus {
                (bus_idx, other_idx)
            } else {
                (other_idx, bus_idx)
            };

            graph.links.push(NetworkLink {
                from,
                to,
                bus_idx,
                bandwidth_bps,
                forwarding_latency_ps,
                queue_depth,
            });

            if !end_stations_seen.contains(&other_idx) {
                end_stations_seen.push(other_idx);
                let other = instance.component(other_idx);
                graph.nodes.push(NetworkNode {
                    idx: other_idx,
                    kind: NodeKind::EndStation,
                    name: other.name.as_str().to_string(),
                });
            }
        }
    }

    graph
}

fn is_bus_category(cat: ComponentCategory) -> bool {
    matches!(cat, ComponentCategory::Bus | ComponentCategory::VirtualBus)
}

fn is_end_station_category(cat: ComponentCategory) -> bool {
    matches!(
        cat,
        ComponentCategory::Device | ComponentCategory::Processor
    )
}

/// Resolve a connection endpoint subcomponent name to the corresponding
/// [`ComponentInstanceIdx`]. `None` subcomponent means the endpoint is
/// on the owner itself.
fn resolve_subcomponent(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    subcomponent: &Option<Name>,
) -> Option<ComponentInstanceIdx> {
    match subcomponent {
        Some(sub_name) => {
            let owner_comp = instance.component(owner);
            owner_comp
                .children
                .iter()
                .find(|&&child_idx| {
                    instance.component(child_idx).name.as_str() == sub_name.as_str()
                })
                .copied()
        }
        None => Some(owner),
    }
}

/// Fallback for connections that reference a feature on the owner
/// itself rather than a named subcomponent. Returns the *other* endpoint
/// of the connection if the bus name appears in either endpoint's
/// feature path.
fn name_other_endpoint(
    instance: &SystemInstance,
    owner: ComponentInstanceIdx,
    conn: &spar_hir_def::instance::ConnectionInstance,
    bus_name: &str,
) -> Option<ComponentInstanceIdx> {
    let src_sub = conn.src.as_ref().and_then(|e| e.subcomponent.as_ref());
    let dst_sub = conn.dst.as_ref().and_then(|e| e.subcomponent.as_ref());

    let bus_lower = bus_name.to_ascii_lowercase();
    let src_is_bus = src_sub
        .map(|n| n.as_str().eq_ignore_ascii_case(&bus_lower))
        .unwrap_or(false);
    let dst_is_bus = dst_sub
        .map(|n| n.as_str().eq_ignore_ascii_case(&bus_lower))
        .unwrap_or(false);

    let other_sub = if src_is_bus {
        dst_sub
    } else if dst_is_bus {
        src_sub
    } else {
        return None;
    };

    let other_sub = other_sub?;
    let owner_comp = instance.component(owner);
    owner_comp
        .children
        .iter()
        .find(|&&child_idx| instance.component(child_idx).name.as_str() == other_sub.as_str())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::PropertyRef;
    use spar_hir_def::properties::PropertyValue;

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
    fn read_switch_type_typed_enum() {
        let props = make_props(
            SPAR_NETWORK,
            "Switch_Type",
            "FIFO",
            Some(PropertyExpr::Enum(Name::new("FIFO"))),
        );
        assert_eq!(read_switch_type(&props), Some(SwitchType::Fifo));
    }

    #[test]
    fn read_switch_type_string_fallback() {
        let props = make_props(SPAR_NETWORK, "Switch_Type", "Priority", None);
        assert_eq!(read_switch_type(&props), Some(SwitchType::Priority));
    }

    #[test]
    fn read_switch_type_missing_returns_none() {
        let props = PropertyMap::new();
        assert_eq!(read_switch_type(&props), None);
    }

    #[test]
    fn read_queue_depth_typed_integer() {
        let props = make_props(
            SPAR_NETWORK,
            "Queue_Depth",
            "16",
            Some(PropertyExpr::Integer(16, None)),
        );
        assert_eq!(read_queue_depth(&props), Some(16));
    }

    #[test]
    fn read_forwarding_latency_typed_range() {
        let expr = PropertyExpr::Range {
            min: Box::new(PropertyExpr::Integer(5, Some(Name::new("us")))),
            max: Box::new(PropertyExpr::Integer(10, Some(Name::new("us")))),
            delta: None,
        };
        let props = make_props(
            SPAR_NETWORK,
            "Forwarding_Latency",
            "5 us .. 10 us",
            Some(expr),
        );
        assert_eq!(
            read_forwarding_latency_ps(&props),
            Some((5_000_000, 10_000_000))
        );
    }

    #[test]
    fn read_forwarding_latency_string_range() {
        let props = make_props(SPAR_NETWORK, "Forwarding_Latency", "5 us .. 10 us", None);
        assert_eq!(
            read_forwarding_latency_ps(&props),
            Some((5_000_000, 10_000_000))
        );
    }

    #[test]
    fn read_output_rate_kbytesps() {
        let props = make_props(
            SPAR_NETWORK,
            "Output_Rate",
            "1000 KBytesps",
            Some(PropertyExpr::Integer(1000, Some(Name::new("KBytesps")))),
        );
        assert_eq!(read_output_rate_bps(&props), Some(1000 * 8 * 1024));
    }

    #[test]
    fn parse_data_rate_bps_known_unit() {
        assert_eq!(
            parse_data_rate_bps("100 MBytesps"),
            Some(100 * 8 * 1024 * 1024)
        );
        assert_eq!(parse_data_rate_bps("1000"), Some(1000));
    }
}
