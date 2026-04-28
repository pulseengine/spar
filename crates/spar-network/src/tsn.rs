//! Time-Sensitive Networking (TSN) primitives.
//!
//! Phase 2 (v0.8.x) implements TAS gate-window service curves
//! (802.1Qbv), CBS credit-pool tracking (802.1Qav), and frame preemption
//! (802.1Qbu). v0.8.1 commit 1 shipped the type surface and
//! `Spar_TSN::*` property readers; commit 2 (this commit) adds the TAS
//! service-curve math (parser for [`Gate_Control_List`], the open-fraction
//! and worst-case gate-latency derivation, and [`tas_residual_service`]).
//!
//! See `docs/designs/track-d-tsn-wctt-research.md` §5.1 (property-set
//! design), §5.2 (switch modeling), and §5.3 (TAS / 802.1Qbv shaping) for
//! the design rationale.
//!
//! [`Gate_Control_List`]: get_gate_control_list_raw

use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::PropertyMap;

use crate::curves::ServiceCurve;

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

// ── Frame preemption (802.1Qbu) ──────────────────────────────────────
//
// 802.1Qbu allows an "express" traffic class to interrupt a
// "preemptable" frame mid-flight, dramatically shrinking the worst-case
// blocking term seen by express frames at a port. Without preemption,
// an express frame may have to wait for an entire preemptable MTU to
// drain before it can be transmitted (B_no_pmt = max_preemptable_frame
// / link_rate). With preemption, that blocking shrinks to a single
// minimum fragment plus the preemption header
// (B_pmt = (MIN_FRAGMENT_BYTES + PREEMPTION_HEADER_BYTES) / link_rate).
//
// References:
// - IEEE Std 802.1Qbu-2016 (Frame Preemption), §6.7.2.
// - IEEE Std 802.3br-2016 (Interspersed Express Traffic), §99.4.7
//   (mPacket and the 4-byte mCRC / SMD-S preemption header).
// - Le Boudec & Thiran, *Network Calculus* (2001), §1.5 (FIFO blocking
//   bounds — the "max_p_size / R" envelope that preemption attacks).
// - docs/designs/track-d-tsn-wctt-research.md §5.2-5.3.

/// Minimum Ethernet frame payload size, in bytes (IEEE 802.3 §3.2.7
/// "minFrameSize" — 64 bytes including FCS, the smallest legal
/// Ethernet frame). When 802.1Qbu preemption is enabled this is the
/// minimum size of an in-flight preemptable fragment, and so it
/// dominates the residual blocking seen by an express frame.
pub const MIN_FRAGMENT_BYTES: u64 = 64;

/// Preemption header overhead per fragment, in bytes (IEEE 802.3br
/// §99.4.7 — the SMD-S/SMD-C start-of-mPacket delimiter plus the
/// 4-byte mCRC tail). Charged once per blocking event because the
/// express frame waits at most one fragment to start transmitting.
pub const PREEMPTION_HEADER_BYTES: u64 = 4;

/// Picoseconds per second; mirrors `crates/spar-network/src/curves.rs`
/// to keep the unit conversion auditable in one place.
const PS_PER_SECOND: u128 = 1_000_000_000_000;
/// Bits per byte.
const BITS_PER_BYTE: u128 = 8;

/// Compute the per-hop blocking term, in picoseconds, that an express
/// frame may encounter at a TSN port before it can begin transmission.
///
/// - `link_rate_bps` — egress link rate in bits per second.
/// - `max_competing_frame_bytes` — the largest preemptable frame size
///   that could be in flight when the express frame arrives (typically
///   the link MTU, e.g. 1518 bytes including the 4-byte VLAN tag).
/// - `preemption_enabled` — whether IEEE 802.1Qbu preemption is active
///   on both the express stream and the bus. When `true`, the
///   blocking term shrinks to
///   `(MIN_FRAGMENT_BYTES + PREEMPTION_HEADER_BYTES) / link_rate`;
///   when `false`, the legacy
///   `max_competing_frame_bytes / link_rate` term is returned.
///
/// Returns `0` when `link_rate_bps == 0` (the caller is responsible
/// for screening that pathological case — a port with zero rate has
/// already failed the bus-bandwidth check).
///
/// The returned value is rounded *up* (ceiling division) so the
/// blocking term is never an under-estimate; this matches the
/// pessimism direction used elsewhere in `spar-network::curves`
/// (`time_to_send_ps` is also a ceiling). See
/// `docs/designs/track-d-tsn-wctt-research.md` §5.2-5.3 for the
/// mathematical derivation.
pub fn preemption_blocking_term_ps(
    link_rate_bps: u64,
    max_competing_frame_bytes: u64,
    preemption_enabled: bool,
) -> u64 {
    if link_rate_bps == 0 {
        return 0;
    }
    let bytes = if preemption_enabled {
        MIN_FRAGMENT_BYTES.saturating_add(PREEMPTION_HEADER_BYTES)
    } else {
        max_competing_frame_bytes
    };
    let numer = (bytes as u128) * BITS_PER_BYTE * PS_PER_SECOND;
    let denom = link_rate_bps as u128;
    let ps = numer.div_ceil(denom);
    if ps > u64::MAX as u128 {
        u64::MAX
    } else {
        ps as u64
    }
}

/// Decide whether a stream is "express" — entitled to preempt other
/// traffic — given its source-side properties.
///
/// Resolution order, per IEEE 802.1Qbu typical mappings and the v0.8.1
/// c4 spec:
///
/// 1. If the stream has an explicit `Spar_TSN::Frame_Preemption` set,
///    use it (`true` ⇒ express).
/// 2. Otherwise default by class-of-service: a stream is express iff
///    `Class_of_Service >= 6` (the two highest 802.1Q PCP values are
///    conventionally reserved for network control / time-sensitive
///    express traffic).
/// 3. With neither property declared, the stream is *not* express.
///
/// The "explicit-then-default" order matches the c4 design spec: an
/// explicit `Frame_Preemption => false` on a high-priority stream
/// overrides the CoS heuristic, and an explicit `Frame_Preemption =>
/// true` on a low-priority stream forces express semantics.
pub fn is_express_stream(props: &PropertyMap) -> bool {
    if let Some(b) = get_frame_preemption(props) {
        return b;
    }
    if let Some(cos) = get_class_of_service(props) {
        return cos.0 >= 6;
    }
    false
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

// ── TAS (802.1Qbv) gate-window service curve ─────────────────────────
//
// Time-Aware Shaper math kernel (v0.8.1 commit 2). Derives the
// rate-latency [`ServiceCurve`] offered to a single class-of-service by
// a TAS-gated egress port from its gate-control list.
//
// Per Le Boudec & Thiran "Network Calculus" (Springer 2001) chapter 1
// and the design discussion in
// `docs/designs/track-d-tsn-wctt-research.md` §5.3:
//
// Let cycle_period = ∑ window.duration over the GCL, and let the open
// time for class K be sum_K_open = ∑ window.duration over windows whose
// cos_mask has bit K set. Then:
//
//   ρ_K  = sum_K_open / cycle_period            (average open fraction)
//   T_K  = max contiguous closed duration       (worst-case gate latency)
//   β_K(t) = (R_link · ρ_K) · max(0, t − T_K)   (rate-latency form)
//
// The "max contiguous closed duration" includes wrap-around across the
// cycle boundary so the bound is correct for arbitrary GCL phasing
// (single-window, multi-window, or gap-only schedules).

/// A parsed TAS gate-control list — a periodic schedule of
/// [`GateWindow`] entries that tile the cycle period without gaps or
/// overlaps.
///
/// Constructed by [`GateSchedule::parse`]. The `cycle_ps` field is the
/// sum of all window durations and is also the GCL cycle period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSchedule {
    /// Windows in declaration order. Successive windows abut: the
    /// `(offset_ps, duration_ps)` pairs tile `[0, cycle_ps)`.
    pub windows: Vec<GateWindow>,
    /// Total cycle period, picoseconds. Equal to `sum(windows.duration_ps)`.
    pub cycle_ps: u64,
}

/// Errors returned by [`GateSchedule::parse`] when the
/// `Gate_Control_List` blob is structurally invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateScheduleError {
    /// The string parsed to zero windows.
    Empty,
    /// A window entry could not be split into the three required
    /// `offset:duration:cos_mask` fields.
    Malformed(String),
    /// The numeric component (`offset`, `duration`, or `cos_mask`) of an
    /// entry could not be parsed.
    ParseInt(String),
    /// A window's `[offset, offset+duration)` range overlaps the next
    /// window's range.
    Overlap {
        /// Index of the first window in the overlapping pair.
        index: usize,
    },
    /// A window's `[offset, offset+duration)` range leaves a gap before
    /// the next window's `offset`.
    Gap {
        /// Index of the window before the gap.
        index: usize,
    },
    /// A window's `duration_ns` is zero (would make ρ_K division
    /// degenerate and the schedule meaningless).
    ZeroDuration {
        /// Index of the offending window.
        index: usize,
    },
}

impl core::fmt::Display for GateScheduleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty gate-control list"),
            Self::Malformed(s) => write!(f, "malformed gate window entry: {:?}", s),
            Self::ParseInt(s) => write!(f, "could not parse integer in gate window entry: {:?}", s),
            Self::Overlap { index } => {
                write!(f, "gate window {} overlaps the next window", index)
            }
            Self::Gap { index } => {
                write!(f, "gap between gate window {} and the next window", index)
            }
            Self::ZeroDuration { index } => {
                write!(f, "gate window {} has zero duration", index)
            }
        }
    }
}

impl core::error::Error for GateScheduleError {}

impl GateSchedule {
    /// Parse a `Gate_Control_List` blob into a structured
    /// [`GateSchedule`].
    ///
    /// Format: `"offset_ns:duration_ns:cos_mask;offset_ns:duration_ns:cos_mask;..."`
    /// where each field is a non-negative decimal integer (the
    /// `cos_mask` may also be expressed in hex with a `0x`/`0X` prefix
    /// so the canonical 802.1Q PCP bitmask form `0x80` is accepted).
    /// Window time units are nanoseconds; the parser converts to
    /// picoseconds (matching the [`GateWindow::offset_ps`] /
    /// [`duration_ps`](GateWindow::duration_ps) representation).
    ///
    /// Trailing semicolons and surrounding whitespace are tolerated.
    /// Validates that the windows tile the cycle period without gaps
    /// or overlaps, returning a [`GateScheduleError`] otherwise.
    pub fn parse(blob: &str) -> Result<Self, GateScheduleError> {
        let trimmed = blob.trim();
        if trimmed.is_empty() {
            return Err(GateScheduleError::Empty);
        }

        let mut windows: Vec<GateWindow> = Vec::new();
        for entry in trimmed.split(';') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.split(':').collect();
            if parts.len() != 3 {
                return Err(GateScheduleError::Malformed(entry.to_string()));
            }
            let offset_ns = parse_decimal_u64(parts[0].trim())
                .ok_or_else(|| GateScheduleError::ParseInt(entry.to_string()))?;
            let duration_ns = parse_decimal_u64(parts[1].trim())
                .ok_or_else(|| GateScheduleError::ParseInt(entry.to_string()))?;
            let cos_mask = parse_cos_mask(parts[2].trim())
                .ok_or_else(|| GateScheduleError::ParseInt(entry.to_string()))?;
            windows.push(GateWindow {
                offset_ps: offset_ns.saturating_mul(1_000),
                duration_ps: duration_ns.saturating_mul(1_000),
                allowed_cos_mask: cos_mask,
            });
        }
        if windows.is_empty() {
            return Err(GateScheduleError::Empty);
        }

        // Validate: windows must tile [0, cycle_ps) in declaration order.
        // We deliberately rely on declaration order rather than sorting:
        // the GCL semantics are sequential and a misordered list is a
        // configuration error (not silently fixable).
        let mut expected_offset: u64 = 0;
        for (i, w) in windows.iter().enumerate() {
            if w.duration_ps == 0 {
                return Err(GateScheduleError::ZeroDuration { index: i });
            }
            match w.offset_ps.cmp(&expected_offset) {
                core::cmp::Ordering::Less => return Err(GateScheduleError::Overlap { index: i }),
                core::cmp::Ordering::Greater => return Err(GateScheduleError::Gap { index: i }),
                core::cmp::Ordering::Equal => {}
            }
            expected_offset = w.offset_ps.saturating_add(w.duration_ps);
        }

        Ok(GateSchedule {
            windows,
            cycle_ps: expected_offset,
        })
    }

    /// Average open fraction for class `cos`, expressed as a numerator
    /// and denominator in picoseconds.
    ///
    /// Returns `(open_time_ps, cycle_ps)`. The fraction is
    /// `open_time_ps / cycle_ps`. Computing the raw ratio is left to
    /// callers so they can carry it in `u128` accumulators where
    /// needed; [`tas_residual_service`] uses this to derive the
    /// `R_link · ρ_K` rate without dropping precision.
    pub fn open_fraction(&self, cos: ClassOfService) -> (u64, u64) {
        let bit = 1u8 << cos.0;
        let mut open_ps: u64 = 0;
        for w in &self.windows {
            if w.allowed_cos_mask & bit != 0 {
                open_ps = open_ps.saturating_add(w.duration_ps);
            }
        }
        (open_ps, self.cycle_ps)
    }

    /// Worst-case gate latency for class `cos`, picoseconds.
    ///
    /// Defined as the maximum contiguous closed (gate-shut-for-`cos`)
    /// duration in the cycle, taken with wrap-around. Equivalently, the
    /// longest stretch of time during which a frame waiting at the
    /// queue cannot egress because no window in the GCL has bit
    /// `cos.0` set.
    ///
    /// Returns `cycle_ps` if no window opens for `cos` (the gate is
    /// permanently closed; all of `cycle_ps` is the closed gap, which
    /// drives `tas_residual_service` to the unservable `(rate=0,
    /// latency=cycle_ps)` form).
    pub fn worst_case_latency(&self, cos: ClassOfService) -> u64 {
        let bit = 1u8 << cos.0;
        // Walk windows once and accumulate the longest run of closed
        // duration. Wrap-around is handled by walking *twice* and
        // recording the longest run that does not exceed `cycle_ps` —
        // this captures a closed run that straddles the cycle boundary.
        let mut max_closed: u64 = 0;
        let mut current_closed: u64 = 0;
        let any_open = self.windows.iter().any(|w| w.allowed_cos_mask & bit != 0);
        if !any_open {
            return self.cycle_ps;
        }
        // Two passes so a closed run that straddles the cycle boundary
        // is captured. Cap at `cycle_ps` so we never report a latency
        // greater than the period.
        for _ in 0..2 {
            for w in &self.windows {
                if w.allowed_cos_mask & bit != 0 {
                    if current_closed > max_closed {
                        max_closed = current_closed;
                    }
                    current_closed = 0;
                } else {
                    current_closed = current_closed.saturating_add(w.duration_ps);
                }
            }
        }
        if current_closed > max_closed {
            max_closed = current_closed;
        }
        max_closed.min(self.cycle_ps)
    }
}

/// Derive the rate-latency [`ServiceCurve`] offered to a single
/// class-of-service by a TAS-gated egress port.
///
/// Inputs:
/// - `schedule` — the parsed gate-control list.
/// - `cos` — the class-of-service whose service curve is requested.
/// - `link_rate_bps` — the underlying link rate (`R_link`, in bits per
///   second), typically read from `Spar_Network::Output_Rate` on the
///   bus.
///
/// Output: `β_K(t) = (R_link · ρ_K) · max(0, t − T_K)` where ρ_K is
/// [`GateSchedule::open_fraction`] and T_K is
/// [`GateSchedule::worst_case_latency`].
///
/// `link_rate_bps · open_time_ps / cycle_ps` is computed in `u128` to
/// avoid overflow (a 100 Gbps link yields a u128 product of ~10²² for
/// millisecond-scale cycles, which still fits). The result is
/// truncated to `u64` (saturating to `u64::MAX`), matching the
/// rounding convention in [`crate::curves`].
///
/// When `cos` never opens in the schedule, the returned curve is
/// `(rate=0, latency=cycle_ps)` — semantically "no service" — which
/// the WCTT pass surfaces via the existing
/// [`crate::curves::NcError::UnservableFlow`] path.
pub fn tas_residual_service(
    schedule: &GateSchedule,
    cos: ClassOfService,
    link_rate_bps: u64,
) -> ServiceCurve {
    let (open_ps, cycle_ps) = schedule.open_fraction(cos);
    let latency_ps = schedule.worst_case_latency(cos);

    // R_link · ρ_K = link_rate_bps · open_ps / cycle_ps, in u128 to
    // avoid overflow on realistic inputs.
    let rate_bps = if cycle_ps == 0 || open_ps == 0 {
        0
    } else {
        let product = (link_rate_bps as u128).saturating_mul(open_ps as u128);
        let r = product / (cycle_ps as u128);
        if r > u64::MAX as u128 {
            u64::MAX
        } else {
            r as u64
        }
    };
    ServiceCurve::rate_latency(rate_bps, latency_ps)
}

/// Parse [`Spar_TSN::Gate_Control_List`] from a [`PropertyMap`] into a
/// structured [`GateSchedule`].
///
/// Returns `None` when the property is unset or the value cannot be
/// parsed (the latter case is converted to `None` for callers that
/// already emit a model-level diagnostic at the WCTT pass; deeper
/// diagnostic surfacing through [`GateScheduleError`] lands in a
/// follow-up commit alongside the TAS-aware diagnostic kind).
///
/// [`Spar_TSN::Gate_Control_List`]: get_gate_control_list_raw
pub fn get_gate_schedule(props: &PropertyMap) -> Option<GateSchedule> {
    let raw = get_gate_control_list_raw(props)?;
    GateSchedule::parse(&raw).ok()
}

fn parse_decimal_u64(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

fn parse_cos_mask(s: &str) -> Option<u8> {
    if let Some(stripped) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u8::from_str_radix(stripped, 16).ok()
    } else {
        s.parse::<u8>().ok()
    }
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

    // ── TAS gate-window service curve tests (v0.8.1 commit 2) ────────

    /// 1 Gbps in bits per second.
    const TAS_GBPS: u64 = 1_000_000_000;

    #[test]
    fn parse_single_window_gcl() {
        // One window covering the whole cycle, mask=0xFF (all classes
        // open). Each unit is ns; offset 0, 10 us cycle.
        let s = GateSchedule::parse("0:10000:0xFF").expect("valid GCL");
        assert_eq!(s.windows.len(), 1);
        assert_eq!(s.cycle_ps, 10_000 * 1_000); // 10 us in ps
        assert_eq!(s.windows[0].offset_ps, 0);
        assert_eq!(s.windows[0].duration_ps, 10_000_000);
        assert_eq!(s.windows[0].allowed_cos_mask, 0xFF);
    }

    #[test]
    fn parse_two_window_gcl_50_50() {
        // 5 us only CoS 7 open; 5 us all other CoS open. Standard
        // motivating example from the commit brief.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").expect("valid GCL");
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.cycle_ps, 10_000_000); // 10 us
        assert_eq!(s.windows[0].offset_ps, 0);
        assert_eq!(s.windows[0].duration_ps, 5_000_000);
        assert_eq!(s.windows[0].allowed_cos_mask, 0x80);
        assert_eq!(s.windows[1].offset_ps, 5_000_000);
        assert_eq!(s.windows[1].duration_ps, 5_000_000);
        assert_eq!(s.windows[1].allowed_cos_mask, 0x7F);
    }

    #[test]
    fn parse_eight_window_gcl_round_trip_preserves_order() {
        // 8 successive 1 us windows opening one CoS at a time, in
        // 7,6,5,4,3,2,1,0 order. The parser must preserve order.
        let blob = "0:1000:0x80;1000:1000:0x40;2000:1000:0x20;3000:1000:0x10;\
                    4000:1000:0x08;5000:1000:0x04;6000:1000:0x02;7000:1000:0x01";
        let s = GateSchedule::parse(blob).expect("valid 8-window GCL");
        assert_eq!(s.windows.len(), 8);
        assert_eq!(s.cycle_ps, 8_000_000); // 8 us
        let masks: Vec<u8> = s.windows.iter().map(|w| w.allowed_cos_mask).collect();
        assert_eq!(masks, vec![0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01]);
    }

    #[test]
    fn parse_overlap_rejected() {
        // Second window starts at 4000 ns but first runs to 5000 ns —
        // overlap by 1 us.
        let err = GateSchedule::parse("0:5000:0x80;4000:5000:0x7F").unwrap_err();
        assert!(matches!(err, GateScheduleError::Overlap { index: 1 }));
    }

    #[test]
    fn parse_gap_rejected() {
        // Second window starts at 6000 ns, first ends at 5000 ns —
        // 1 us gap.
        let err = GateSchedule::parse("0:5000:0x80;6000:5000:0x7F").unwrap_err();
        assert!(matches!(err, GateScheduleError::Gap { index: 1 }));
    }

    #[test]
    fn parse_malformed_rejected() {
        // Missing the third field.
        assert!(matches!(
            GateSchedule::parse("0:5000"),
            Err(GateScheduleError::Malformed(_))
        ));
        // Non-numeric offset.
        assert!(matches!(
            GateSchedule::parse("xyz:5000:0x80"),
            Err(GateScheduleError::ParseInt(_))
        ));
        // Empty blob.
        assert_eq!(GateSchedule::parse(""), Err(GateScheduleError::Empty));
        assert_eq!(GateSchedule::parse("   "), Err(GateScheduleError::Empty));
        // Trailing semicolon is tolerated (does not produce an empty
        // entry in the parser); a single semicolon-only blob is empty.
        assert!(GateSchedule::parse("0:5000:0x80;").is_ok());
    }

    #[test]
    fn open_fraction_two_window_50_50() {
        // CoS 7 open in window 1 only (5 us / 10 us cycle = 50%).
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        let (open_ps, cycle_ps) = s.open_fraction(cos7);
        assert_eq!(open_ps, 5_000_000);
        assert_eq!(cycle_ps, 10_000_000);

        // CoS 0 open in window 2 only (also 50%).
        let cos0 = ClassOfService::new(0).unwrap();
        let (open0, _) = s.open_fraction(cos0);
        assert_eq!(open0, 5_000_000);
    }

    #[test]
    fn worst_case_latency_two_window() {
        // CoS 7 open for the first 5 us, closed for the next 5 us. The
        // longest closed run is 5 us — straight through window 2.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        assert_eq!(s.worst_case_latency(cos7), 5_000_000);

        // CoS 0 closed for the first 5 us, open for the next 5 us. Same
        // worst-case latency by symmetry.
        let cos0 = ClassOfService::new(0).unwrap();
        assert_eq!(s.worst_case_latency(cos0), 5_000_000);
    }

    #[test]
    fn worst_case_latency_wrap_around() {
        // Three windows: closed-open-closed for a particular CoS. The
        // closed runs straddling the cycle boundary should be combined.
        // Layout: [0..2us closed for CoS 7][2..4us open for CoS 7][4..10us closed for CoS 7]
        // Closed runs: window 0 (2 us) + window 2 (6 us). With wrap-around
        // the longest is window 2 followed by window 0 = 6 + 2 = 8 us.
        let s = GateSchedule::parse("0:2000:0x7F;2000:2000:0x80;4000:6000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        assert_eq!(s.worst_case_latency(cos7), 8_000_000);
    }

    #[test]
    fn worst_case_latency_permanently_closed() {
        // CoS 0 never opens (no mask has bit 0 set). worst_case_latency
        // returns the full cycle period — the gate is permanently shut.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0xFE").unwrap();
        let cos0 = ClassOfService::new(0).unwrap();
        assert_eq!(s.worst_case_latency(cos0), 10_000_000);
    }

    #[test]
    fn tas_residual_service_50_percent_open() {
        // 50% open, 1 Gbps link → service rate = 500 Mbps.
        // Latency = 5 us = 5_000_000 ps.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        let svc = tas_residual_service(&s, cos7, TAS_GBPS);
        assert_eq!(svc.rate_bps, 500_000_000);
        assert_eq!(svc.latency_ps, 5_000_000);
    }

    #[test]
    fn tas_residual_service_full_open() {
        // Single window covering the whole cycle, all CoS open. Service
        // rate = link rate; latency = 0 (gate is never closed).
        let s = GateSchedule::parse("0:10000:0xFF").unwrap();
        let cos3 = ClassOfService::new(3).unwrap();
        let svc = tas_residual_service(&s, cos3, TAS_GBPS);
        assert_eq!(svc.rate_bps, TAS_GBPS);
        assert_eq!(svc.latency_ps, 0);
    }

    #[test]
    fn tas_residual_service_unservable_when_class_never_opens() {
        // CoS 0 never opens → rate = 0, latency = cycle. The wctt pass
        // surfaces this as an UnservableFlow downstream.
        let s = GateSchedule::parse("0:10000:0x80").unwrap();
        let cos0 = ClassOfService::new(0).unwrap();
        let svc = tas_residual_service(&s, cos0, TAS_GBPS);
        assert_eq!(svc.rate_bps, 0);
        assert_eq!(svc.latency_ps, 10_000_000);
    }

    #[test]
    fn get_gate_schedule_reads_property_map() {
        // String-fallback path: the typed PropertyExpr is None, so the
        // accessor walks the raw blob through GateSchedule::parse.
        let props = make_props(
            SPAR_TSN,
            "Gate_Control_List",
            "0:5000:0x80;5000:5000:0x7F",
            None,
        );
        let s = get_gate_schedule(&props).expect("schedule parses");
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.cycle_ps, 10_000_000);

        // Missing property returns None.
        let empty = PropertyMap::new();
        assert!(get_gate_schedule(&empty).is_none());

        // Malformed blob returns None (full structured-error surfacing
        // is a follow-up commit; today's caller emits a model-level
        // diagnostic at the WCTT pass when this returns None).
        let bad = make_props(SPAR_TSN, "Gate_Control_List", "not a gcl", None);
        assert!(get_gate_schedule(&bad).is_none());
    }

    // ── Frame preemption (802.1Qbu) ──────────────────────────────────

    /// 100 Mbps in bits per second.
    const HUNDRED_MBPS: u64 = 100_000_000;

    #[test]
    fn preemption_constants_match_802_3br() {
        // IEEE 802.3 minFrameSize = 64 bytes (incl. FCS).
        assert_eq!(MIN_FRAGMENT_BYTES, 64);
        // IEEE 802.3br SMD-S + 4-byte mCRC = 4 bytes preemption header.
        assert_eq!(PREEMPTION_HEADER_BYTES, 4);
    }

    #[test]
    fn preemption_blocking_term_with_preemption_enabled() {
        // 100 Mbps, MTU=1518 (irrelevant when preemption is enabled).
        // Bytes blocked = 64 + 4 = 68 bytes.
        // Blocking time = 68 * 8 * 1e12 / 1e8 = 5_440_000 ps = 5.44 us.
        let t = preemption_blocking_term_ps(HUNDRED_MBPS, 1518, true);
        assert_eq!(t, 5_440_000);
    }

    #[test]
    fn preemption_blocking_term_without_preemption_falls_back_to_max_frame() {
        // 100 Mbps, MTU=1518: the legacy term.
        // Blocking time = 1518 * 8 * 1e12 / 1e8 = 121_440_000 ps ≈ 121 us.
        let t = preemption_blocking_term_ps(HUNDRED_MBPS, 1518, false);
        assert_eq!(t, 121_440_000);
    }

    #[test]
    fn preemption_blocking_term_zero_rate_returns_zero() {
        // Pathological zero-rate input is screened by bus_bandwidth
        // upstream; we just make sure we don't panic / divide by zero.
        assert_eq!(preemption_blocking_term_ps(0, 1518, true), 0);
        assert_eq!(preemption_blocking_term_ps(0, 1518, false), 0);
    }

    #[test]
    fn preemption_blocking_term_rounds_up_for_pessimism() {
        // Non-integer-byte boundary: 65 bytes at 100 Mbps with no
        // preemption enabled. 65 * 8 = 520 bits @ 100 Mbps = 5.2 us.
        // Integer-rounded numerator/denominator: 5_200_000_000_000_000
        // / 100_000_000 = 5_200_000 ps exactly. Pick a rate that is
        // *not* a clean divisor instead.
        // 1518 bytes at 1 Gbps: 1518 * 8 * 1e12 / 1e9 = 12_144_000 ps
        // exactly. Use 999_999_999 bps (1 Gbps - 1 bps) so we straddle.
        let exact = preemption_blocking_term_ps(1_000_000_000, 1518, false);
        let nearly = preemption_blocking_term_ps(999_999_999, 1518, false);
        // Nearly-Gbps (lower) takes *strictly more* time than exact Gbps.
        assert!(nearly > exact);
        // Difference is at most one ceiling round-up bit (1 ps).
        assert!(nearly - exact >= 1);
    }

    #[test]
    fn is_express_stream_explicit_overrides_cos() {
        // Explicit Frame_Preemption=>true forces express even at low CoS.
        let mut props = make_props(
            SPAR_TSN,
            "Class_of_Service",
            "0",
            Some(PropertyExpr::Integer(0, None)),
        );
        props.add(spar_hir_def::properties::PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new(SPAR_TSN)),
                property_name: Name::new("Frame_Preemption"),
            },
            value: "true".to_string(),
            typed_expr: Some(PropertyExpr::Boolean(true)),
            is_append: false,
        });
        assert!(is_express_stream(&props));

        // Explicit Frame_Preemption=>false forces preemptable even at
        // high CoS.
        let mut props = make_props(
            SPAR_TSN,
            "Class_of_Service",
            "7",
            Some(PropertyExpr::Integer(7, None)),
        );
        props.add(spar_hir_def::properties::PropertyValue {
            name: PropertyRef {
                property_set: Some(Name::new(SPAR_TSN)),
                property_name: Name::new("Frame_Preemption"),
            },
            value: "false".to_string(),
            typed_expr: Some(PropertyExpr::Boolean(false)),
            is_append: false,
        });
        assert!(!is_express_stream(&props));
    }

    #[test]
    fn is_express_stream_cos_default_threshold() {
        // No Frame_Preemption set → fall back to CoS-based default.
        // CoS 0..=5: not express.
        for cos in 0u8..=5 {
            let props = make_props(
                SPAR_TSN,
                "Class_of_Service",
                &cos.to_string(),
                Some(PropertyExpr::Integer(cos as i64, None)),
            );
            assert!(
                !is_express_stream(&props),
                "CoS {} must default to non-express",
                cos
            );
        }
        // CoS 6 and 7: express.
        for cos in 6u8..=7 {
            let props = make_props(
                SPAR_TSN,
                "Class_of_Service",
                &cos.to_string(),
                Some(PropertyExpr::Integer(cos as i64, None)),
            );
            assert!(
                is_express_stream(&props),
                "CoS {} must default to express",
                cos
            );
        }
    }

    #[test]
    fn is_express_stream_no_props_is_not_express() {
        // Neither Frame_Preemption nor Class_of_Service declared: the
        // safe default is non-express (preemptable).
        let empty = PropertyMap::new();
        assert!(!is_express_stream(&empty));
    }
}
