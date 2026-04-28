//! Time-Sensitive Networking (TSN) primitives — placeholder.
//!
//! Phase 2 (v0.8.x) will implement TAS gate-window service curves
//! (802.1Qbv), CBS credit-pool tracking (802.1Qav), and frame preemption
//! (802.1Qbu). v0.8.1 commit 1 ships only the type surface and
//! `Spar_TSN::*` property reader; subsequent commits add the math.
//!
//! See `docs/designs/track-d-tsn-wctt-research.md` §5.1 (property-set
//! design) and §5.2 (switch modeling) for the design rationale.

use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::PropertyMap;

const SPAR_TSN: &str = "Spar_TSN";

// ── Types ────────────────────────────────────────────────────────────

/// Gate-window — one entry in a TAS gate-control list. Phase 2 will
/// parse these from the [`Spar_TSN::Gate_Control_List`] string; for
/// v0.8.1 the type exists but no parser is wired.
///
/// [`Spar_TSN::Gate_Control_List`]: get_gate_control_list_raw
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateWindow {
    /// Offset from the start of the GCL cycle, picoseconds.
    pub offset_ps: u64,
    /// Window duration, picoseconds.
    pub duration_ps: u64,
    /// Bitmask of class-of-service priorities allowed during this window.
    pub allowed_cos_mask: u8,
}

/// Class of Service — 802.1Q priority (0-7).
///
/// Constructed via [`ClassOfService::new`] which enforces the 0..=7
/// range. Implements `Ord` so callers can compare priorities directly
/// (higher value = higher priority in 802.1Q convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClassOfService(pub u8);

impl ClassOfService {
    /// Construct a [`ClassOfService`] if `c` is in `0..=7`, otherwise
    /// `None`. Mirrors the 802.1Q PCP three-bit field.
    pub fn new(c: u8) -> Option<Self> {
        if c <= 7 { Some(Self(c)) } else { None }
    }
}

/// CBS credit-pool descriptor — Phase 2 will compute fill from
/// class-of-service rate. v0.8.1: type only.
///
/// `idle_slope_bps` and `send_slope_bps` carry the 802.1Qav
/// parameters in bits per second; `max_credit_bytes` is the
/// hi-credit bound.
#[derive(Debug, Clone)]
pub struct CreditPool {
    /// Maximum positive credit, bytes (`hiCredit` in 802.1Qav).
    pub max_credit_bytes: u64,
    /// Idle slope in bits per second (rate at which credit accumulates
    /// while the queue has nothing to send).
    pub idle_slope_bps: u64,
    /// Send slope in bits per second (rate at which credit drains
    /// while the queue is transmitting; conventionally negative in
    /// the spec, stored here as the absolute magnitude).
    pub send_slope_bps: u64,
}

// ── Property accessors ───────────────────────────────────────────────
//
// Mirrors the typed-first / string-fallback idiom from
// `crates/spar-network/src/extract.rs`. Each accessor first consults
// the typed [`PropertyExpr`] and falls back to the raw string blob so
// hand-written test fixtures and parser-typed paths both work.

fn get_typed<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a PropertyExpr> {
    props
        .get_typed(SPAR_TSN, name)
        .or_else(|| props.get_typed("", name))
}

fn get_raw<'a>(props: &'a PropertyMap, name: &str) -> Option<&'a str> {
    props.get(SPAR_TSN, name).or_else(|| props.get("", name))
}

/// Read [`Spar_TSN::Stream_ID`] — the per-stream identifier required
/// by TAS gate-control lists and stream reservation.
///
/// Returns `None` if the property is unset, negative, or larger than
/// `u32::MAX` (the AADL-declared range is `0..2**32-1`).
pub fn get_stream_id(props: &PropertyMap) -> Option<u32> {
    if let Some(expr) = get_typed(props, "Stream_ID")
        && let PropertyExpr::Integer(v, _) = expr
        && *v >= 0
        && *v <= u32::MAX as i64
    {
        return Some(*v as u32);
    }
    let raw = get_raw(props, "Stream_ID")?;
    raw.trim().parse::<u32>().ok()
}

/// Read [`Spar_TSN::Class_of_Service`] — the 802.1Q priority (0..=7).
///
/// Values outside `0..=7` return `None`.
pub fn get_class_of_service(props: &PropertyMap) -> Option<ClassOfService> {
    if let Some(expr) = get_typed(props, "Class_of_Service")
        && let PropertyExpr::Integer(v, _) = expr
        && (0..=7).contains(v)
    {
        return ClassOfService::new(*v as u8);
    }
    let raw = get_raw(props, "Class_of_Service")?;
    let v: u8 = raw.trim().parse().ok()?;
    ClassOfService::new(v)
}

/// Read [`Spar_TSN::Max_Frame_Size`] as a byte count.
///
/// Accepts the standard AADL size units (`bits`, `Bytes`, `KByte`,
/// `MByte`, `GByte`, `TByte`) on the typed path. A bare integer is
/// interpreted as bytes (matching the documented declaration of this
/// property as `aadlinteger units Size_Units`, where the canonical
/// unit reported by the design doc is bytes).
pub fn get_max_frame_size_bytes(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed(props, "Max_Frame_Size") {
        return extract_size_bytes(expr);
    }
    let raw = get_raw(props, "Max_Frame_Size")?;
    parse_size_bytes(raw)
}

/// Read [`Spar_TSN::Frame_Preemption`] — whether frames in this
/// class can be pre-empted by Express traffic (802.1Qbu).
pub fn get_frame_preemption(props: &PropertyMap) -> Option<bool> {
    if let Some(expr) = get_typed(props, "Frame_Preemption")
        && let PropertyExpr::Boolean(b) = expr
    {
        return Some(*b);
    }
    let raw = get_raw(props, "Frame_Preemption")?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Read [`Spar_TSN::Gate_Control_List`] as the raw string blob.
///
/// v0.8.1 commit 1 surface only — the structured form (a list of
/// [`GateWindow`] entries) lands in v0.8.1 commit 2 once the TAS
/// service-curve math is wired up.
pub fn get_gate_control_list_raw(props: &PropertyMap) -> Option<String> {
    if let Some(expr) = get_typed(props, "Gate_Control_List")
        && let PropertyExpr::StringLit(s) = expr
    {
        return Some(s.clone());
    }
    get_raw(props, "Gate_Control_List").map(|s| s.trim().trim_matches('"').to_string())
}

// ── Internal helpers ─────────────────────────────────────────────────

const SIZE_UNIT_FACTORS_BYTES: &[(&str, u64)] = &[
    ("bits", 0), // sentinel — bits do not lower to whole bytes < 8
    ("Bytes", 1),
    ("KByte", 1024),
    ("MByte", 1024 * 1024),
    ("GByte", 1024 * 1024 * 1024),
    ("TByte", 1024 * 1024 * 1024 * 1024),
];

fn size_unit_factor_bytes(unit: &str) -> Option<u64> {
    // Special-case `bits`: convert by integer division (8 bits = 1 byte).
    // We surface this through the caller so we can keep the lookup
    // table dense and avoid mis-multiplying by 0.
    if unit.eq_ignore_ascii_case("bits") {
        return None;
    }
    SIZE_UNIT_FACTORS_BYTES
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

fn extract_size_bytes(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, Some(unit)) if *v >= 0 => {
            if unit.as_str().eq_ignore_ascii_case("bits") {
                Some((*v as u64) / 8)
            } else {
                let factor = size_unit_factor_bytes(unit.as_str())?;
                (*v as u64).checked_mul(factor)
            }
        }
        PropertyExpr::Integer(v, None) if *v >= 0 => Some(*v as u64),
        PropertyExpr::UnitValue(inner, unit) => {
            let bits = unit.as_str().eq_ignore_ascii_case("bits");
            match inner.as_ref() {
                PropertyExpr::Integer(v, _) if *v >= 0 => {
                    if bits {
                        Some((*v as u64) / 8)
                    } else {
                        let factor = size_unit_factor_bytes(unit.as_str())?;
                        (*v as u64).checked_mul(factor)
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_size_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit, factor) in SIZE_UNIT_FACTORS_BYTES {
        if let Some(num_str) = s.strip_suffix(unit).map(|s| s.trim()) {
            let v = num_str.parse::<u64>().ok()?;
            if unit.eq_ignore_ascii_case("bits") {
                return Some(v / 8);
            }
            return v.checked_mul(factor);
        }
    }
    s.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::{Name, PropertyRef};
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
    fn class_of_service_in_range() {
        // 0..=7 accepted.
        for c in 0u8..=7 {
            assert_eq!(ClassOfService::new(c), Some(ClassOfService(c)));
        }
        // >7 rejected.
        assert_eq!(ClassOfService::new(8), None);
        assert_eq!(ClassOfService::new(255), None);
    }

    #[test]
    fn class_of_service_ordering() {
        let lo = ClassOfService::new(0).unwrap();
        let hi = ClassOfService::new(7).unwrap();
        assert!(hi > lo);
        assert!(lo < hi);
        assert_eq!(hi.cmp(&lo), std::cmp::Ordering::Greater);
    }

    #[test]
    fn gate_window_construct() {
        let gw = GateWindow {
            offset_ps: 1_000,
            duration_ps: 500,
            allowed_cos_mask: 0b1000_0000,
        };
        assert_eq!(gw.offset_ps, 1_000);
        assert_eq!(gw.duration_ps, 500);
        assert_eq!(gw.allowed_cos_mask, 0b1000_0000);
        // Cloneable + equatable for downstream containers.
        let gw2 = gw.clone();
        assert_eq!(gw, gw2);
    }

    #[test]
    fn credit_pool_construct() {
        let cp = CreditPool {
            max_credit_bytes: 12_000,
            idle_slope_bps: 100_000_000,
            send_slope_bps: 900_000_000,
        };
        assert_eq!(cp.max_credit_bytes, 12_000);
        assert_eq!(cp.idle_slope_bps, 100_000_000);
        assert_eq!(cp.send_slope_bps, 900_000_000);
    }

    #[test]
    fn read_stream_id_from_property_map() {
        let props = make_props(
            SPAR_TSN,
            "Stream_ID",
            "42",
            Some(PropertyExpr::Integer(42, None)),
        );
        assert_eq!(get_stream_id(&props), Some(42));

        // String fallback path.
        let props_str = make_props(SPAR_TSN, "Stream_ID", "100", None);
        assert_eq!(get_stream_id(&props_str), Some(100));

        // Missing returns None.
        let empty = PropertyMap::new();
        assert_eq!(get_stream_id(&empty), None);
    }

    #[test]
    fn read_class_of_service_from_property_map() {
        let props = make_props(
            SPAR_TSN,
            "Class_of_Service",
            "3",
            Some(PropertyExpr::Integer(3, None)),
        );
        assert_eq!(get_class_of_service(&props), Some(ClassOfService(3)));

        // Out-of-range typed value returns None.
        let bad = make_props(
            SPAR_TSN,
            "Class_of_Service",
            "9",
            Some(PropertyExpr::Integer(9, None)),
        );
        assert_eq!(get_class_of_service(&bad), None);

        // String fallback path.
        let props_str = make_props(SPAR_TSN, "Class_of_Service", "5", None);
        assert_eq!(get_class_of_service(&props_str), Some(ClassOfService(5)));
    }
}
