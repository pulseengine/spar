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

use crate::curves::{ArrivalCurve, ServiceCurve, delay_bound};

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

/// 802.1Qav CBS reservation parameters for one class.
///
/// Derived from the per-class properties (`Spar_TSN::Bandwidth_Reservation`
/// for the idle slope, plus the bus's `Spar_Network::Output_Rate` for the
/// link rate that determines the send slope and the credit bounds).
///
/// All slopes are stored in bits per second. By the standard:
/// - `idle_slope_bps > 0` (the reserved bps for this class)
/// - `send_slope_bps == idle_slope_bps - link_rate_bps` (always negative
///   for a stable reservation; we keep the signed form so the analysis
///   does not need to remember the convention)
/// - `hi_credit_bytes` and `lo_credit_bytes` bound the credit pool;
///   `lo_credit_bytes` is the magnitude of the credit floor (always a
///   non-negative byte count here, the spec's `loCredit` is its negation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CbsReservation {
    /// Reserved bandwidth for this class (`idleSlope`), bits per second.
    pub idle_slope_bps: u64,
    /// Drain rate while the class is transmitting (`sendSlope`), bits
    /// per second; conventionally negative in the standard
    /// (= idleSlope − linkRate).
    pub send_slope_bps: i64,
    /// Upper credit bound (`hiCredit`), bytes.
    pub hi_credit_bytes: u64,
    /// Magnitude of the lower credit bound (`|loCredit|`), bytes. The
    /// classical CBS service-curve closed form drains credit to this
    /// value during the worst-case other-class burst.
    pub lo_credit_bytes: u64,
}

impl CbsReservation {
    /// Construct a [`CbsReservation`] from the four primary parameters.
    ///
    /// `idle_slope_bps` is the reserved bandwidth; `link_rate_bps` is
    /// the underlying link rate. `send_slope_bps` is computed as
    /// `idleSlope − linkRate` and is therefore non-positive when the
    /// reservation is feasible (idleSlope ≤ linkRate). Returns `None`
    /// when the reservation is infeasible (idleSlope > linkRate) or
    /// the link rate is zero.
    pub fn new(
        idle_slope_bps: u64,
        link_rate_bps: u64,
        hi_credit_bytes: u64,
        lo_credit_bytes: u64,
    ) -> Option<Self> {
        if link_rate_bps == 0 || idle_slope_bps > link_rate_bps {
            return None;
        }
        // sendSlope = idleSlope − linkRate; always ≤ 0 here.
        let send_slope_bps = (idle_slope_bps as i128) - (link_rate_bps as i128);
        // Above bounds make this fit in i64 safely (link_rate ≤ u64::MAX
        // means the difference is in [-(2^63), 0]; clamp defensively).
        let send_slope_bps = if send_slope_bps < i64::MIN as i128 {
            i64::MIN
        } else {
            send_slope_bps as i64
        };
        Some(Self {
            idle_slope_bps,
            send_slope_bps,
            hi_credit_bytes,
            lo_credit_bytes,
        })
    }

    /// Reserved-bandwidth fraction `idleSlope / link_rate` as a
    /// rational `(num, den)`. Convenience for diagnostics.
    pub fn reserved_fraction(&self) -> (u64, u64) {
        // The link rate is recoverable as idle − send (since
        // sendSlope = idleSlope − linkRate ⇒ linkRate = idleSlope −
        // sendSlope; sendSlope is non-positive here).
        let link = (self.idle_slope_bps as i128) - (self.send_slope_bps as i128);
        let link = if link < 0 { 0 } else { link as u64 };
        (self.idle_slope_bps, link)
    }
}

/// 802.1Qav CBS service curve for a class.
///
/// Implements the Le Boudec/Thiran (§1.4.3) rate-latency closed-form
/// from the AVB/TSN literature (Lim et al., "Delay analysis in CBS for
/// AVB"):
///
/// ```text
/// β_class(t) = idleSlope · max(0, t − T)
/// ```
///
/// where the service latency T captures the worst-case credit drain
/// caused by other-class traffic:
///
/// ```text
/// T = max_competing_frame / link_rate
///     + lo_credit / |sendSlope|
/// ```
///
/// The first term is the maximum-frame blocking from a non-CBS class
/// (the frame that started transmitting just before the CBS class
/// became eligible, holding the link until completion). The second term
/// is the additional credit-recovery time once the CBS queue starts
/// being serviced — `lo_credit` bytes have to be replenished before
/// service can resume, at the (positive) recovery rate `|sendSlope|`.
///
/// Both terms are converted to picoseconds with rounding *up* so the
/// returned latency is never an under-estimate, matching the pessimism
/// direction of `delay_bound` and `residual_service` in `curves.rs`.
///
/// `max_competing_frame_bytes` is the maximum frame size of any
/// non-CBS competing class (in bytes). When unknown, callers default to
/// the Ethernet MTU (1518 bytes); see [`crate::curves`] for unit
/// conventions.
pub fn cbs_residual_service(
    reservation: &CbsReservation,
    link_rate_bps: u64,
    max_competing_frame_bytes: u64,
) -> ServiceCurve {
    // The reservation is the service rate. The latency T captures the
    // worst-case time the credit pool needs to refill from `loCredit`
    // back to zero, including blocking by other classes.
    let rate_bps = reservation.idle_slope_bps;

    // Term 1: maximum-frame blocking from non-CBS classes.
    //
    //   blocking_ps = max_competing_frame · 8 · 10^12 / link_rate
    //
    // Rounded up; saturates to u64::MAX if link_rate is zero (which the
    // CBS analysis path already guards against, but we keep the
    // helper safe in isolation).
    let blocking_ps = if link_rate_bps == 0 {
        0
    } else {
        cbs_time_to_send_ps_rounded_up(max_competing_frame_bytes, link_rate_bps)
    };

    // Term 2: credit recovery from loCredit. We need to replenish
    // `lo_credit_bytes` worth of credit at the recovery rate, which is
    // the *send-slope magnitude*. By the standard sendSlope ≤ 0 here;
    // we take its absolute value.
    //
    // When |sendSlope| == 0 the formula is degenerate (idleSlope ==
    // link_rate, no other class can take the link, so the CBS class is
    // never blocked). In that boundary case the credit-recovery term
    // is zero.
    let send_slope_abs = if reservation.send_slope_bps >= 0 {
        0u64
    } else {
        // sendSlope is negative; magnitude fits in u64 because sendSlope
        // = idleSlope − linkRate where both are u64-bounded.
        reservation.send_slope_bps.unsigned_abs()
    };
    let recovery_ps = if send_slope_abs == 0 {
        0
    } else {
        cbs_time_to_send_ps_rounded_up(reservation.lo_credit_bytes, send_slope_abs)
    };

    let latency_ps = blocking_ps.saturating_add(recovery_ps);
    ServiceCurve::rate_latency(rate_bps, latency_ps)
}

/// Internal helper: time in picoseconds to transmit `bytes` at
/// `rate_bps`, rounding up. Mirrors `curves::time_to_send_ps` (which is
/// crate-private) so the CBS path does not need to expose that helper.
fn cbs_time_to_send_ps_rounded_up(bytes: u64, rate_bps: u64) -> u64 {
    if rate_bps == 0 {
        return u64::MAX;
    }
    // 8 bits per byte · 10^12 ps per second.
    let numer = (bytes as u128) * 8u128 * 1_000_000_000_000u128;
    let denom = rate_bps as u128;
    let ps = numer.div_ceil(denom);
    if ps > u64::MAX as u128 {
        u64::MAX
    } else {
        ps as u64
    }
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

/// Read [`Spar_TSN::Hi_Credit`] as the CBS hiCredit upper bound in
/// bytes. Per IEEE 802.1Qav §34.5, hiCredit is the maximum positive
/// credit a class can accumulate before sending; it bounds the
/// burst-out tolerance. AADL declares the property as `aadlinteger
/// units Size_Units` so it lowers via the same machinery as
/// `Max_Frame_Size`. Returns `None` when unset (callers default to
/// the pre-v0.9.2 behaviour of `Max_Frame_Size`).
pub fn get_hi_credit_bytes(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed(props, "Hi_Credit") {
        return extract_size_bytes(expr);
    }
    let raw = get_raw(props, "Hi_Credit")?;
    parse_size_bytes(raw)
}

/// Read [`Spar_TSN::Lo_Credit`] as the CBS loCredit lower bound in
/// bytes (declared as a positive magnitude — the AVB spec uses a
/// negative value but spar stores the absolute byte count). Per
/// IEEE 802.1Qav §34.5, loCredit gives the worst-case credit
/// drain, which combined with `|sendSlope|` yields the recovery
/// time `T_recovery = loCredit / |sendSlope|` that lands in the
/// CBS rate-latency service curve. Returns `None` when unset.
pub fn get_lo_credit_bytes(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed(props, "Lo_Credit") {
        return extract_size_bytes(expr);
    }
    let raw = get_raw(props, "Lo_Credit")?;
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

/// Read [`Spar_TSN::Bandwidth_Reservation`] as the CBS idleSlope, in
/// bits per second.
///
/// This is the per-class reserved bandwidth used by the 802.1Qav
/// Credit-Based Shaper. AADL declares the property as `aadlinteger
/// units Data_Rate_Units`, so we accept the standard project units
/// (`bitsps`, `Bytesps`, `KBytesps`, `MBytesps`, `GBytesps`)
/// case-insensitively. A bare integer is interpreted as bits per
/// second, matching `Spar_Network::Output_Rate`.
pub fn get_bandwidth_reservation_bps(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed(props, "Bandwidth_Reservation")
        && let Some(bps) = extract_data_rate_bps_local(expr)
    {
        return Some(bps);
    }
    let raw = get_raw(props, "Bandwidth_Reservation")?;
    parse_data_rate_bps_local(raw)
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

/// Read [`Spar_TSN::Sync_Error`] — the per-hop gPTP (IEEE 802.1AS)
/// synchronization-error budget ε, returned in picoseconds.
///
/// In a real TSN deployment the gate-control list is enforced against
/// the local synchronized clock, which can differ from the master clock
/// by up to ε per hop. The TAS service-curve math (v0.9.1 NC soundness
/// pass) subtracts ε from the effective open time and adds ε to the
/// worst-case gate latency so the bounds remain sound under realistic
/// clock accuracy. When the property is unset this accessor returns
/// `None`, callers treat that as ε = 0 (legacy v0.8.1 behaviour) so
/// the non-regression contract holds byte-identically.
///
/// Accepts the standard AADL time units (`ps`, `ns`, `us`, `ms`, `sec`,
/// etc.) on both the typed and string-fallback paths. A bare integer
/// (or string) is interpreted as picoseconds, matching the canonical
/// unit used elsewhere in the WCTT pass.
pub fn get_sync_error_ps(props: &PropertyMap) -> Option<u64> {
    if let Some(expr) = get_typed(props, "Sync_Error")
        && let Some(ps) = extract_time_ps_local(expr)
    {
        return Some(ps);
    }
    let raw = get_raw(props, "Sync_Error")?;
    parse_time_ps_local(raw)
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

// ── Data-rate helpers (local) ────────────────────────────────────────
//
// We mirror the parsing logic from `extract.rs` instead of cross-importing
// because those helpers are crate-private to that module's call sites
// (the Output_Rate accessor) and CBS reads its own property under
// `Spar_TSN`. Keeping a small local copy avoids exposing internals.

const DATA_RATE_UNIT_FACTORS_BPS: &[(&str, u64)] = &[
    ("bitsps", 1),
    ("Bytesps", 8),
    ("KBytesps", 8 * 1024),
    ("MBytesps", 8 * 1024 * 1024),
    ("GBytesps", 8 * 1024 * 1024 * 1024),
];

fn data_rate_unit_factor_bps(unit: &str) -> Option<u64> {
    DATA_RATE_UNIT_FACTORS_BPS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

fn extract_data_rate_bps_local(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, Some(unit)) if *v >= 0 => {
            let factor = data_rate_unit_factor_bps(unit.as_str())?;
            (*v as u64).checked_mul(factor)
        }
        PropertyExpr::Integer(v, None) if *v >= 0 => Some(*v as u64),
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            if v < 0.0 {
                return None;
            }
            let factor = data_rate_unit_factor_bps(unit.as_str())?;
            Some((v * factor as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = data_rate_unit_factor_bps(unit.as_str())?;
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

fn parse_data_rate_bps_local(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit, factor) in DATA_RATE_UNIT_FACTORS_BPS {
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

// ── Time-unit helpers (local) ────────────────────────────────────────
//
// Mirrors the time-unit table in `crates/spar-network/src/extract.rs`
// (which is crate-private to that module's call sites). Used by the
// `Sync_Error` accessor so the property can be declared in any of the
// standard AADL Time_Units (ps/ns/us/ms/sec/min/hr).

const TIME_UNIT_FACTORS_PS_LOCAL: &[(&str, u64)] = &[
    ("ps", 1),
    ("ns", 1_000),
    ("us", 1_000_000),
    ("ms", 1_000_000_000),
    ("sec", 1_000_000_000_000),
    ("min", 60_000_000_000_000),
    ("hr", 3_600_000_000_000_000),
];

fn time_unit_factor_ps_local(unit: &str) -> Option<u64> {
    TIME_UNIT_FACTORS_PS_LOCAL
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(unit))
        .map(|(_, f)| *f)
}

fn extract_time_ps_local(expr: &PropertyExpr) -> Option<u64> {
    match expr {
        PropertyExpr::Integer(v, Some(unit)) if *v >= 0 => {
            let factor = time_unit_factor_ps_local(unit.as_str())?;
            (*v as u64).checked_mul(factor)
        }
        PropertyExpr::Integer(v, None) if *v >= 0 => Some(*v as u64),
        PropertyExpr::Real(s, Some(unit)) => {
            let v: f64 = s.parse().ok()?;
            if v < 0.0 {
                return None;
            }
            let factor = time_unit_factor_ps_local(unit.as_str())?;
            Some((v * factor as f64) as u64)
        }
        PropertyExpr::UnitValue(inner, unit) => {
            let factor = time_unit_factor_ps_local(unit.as_str())?;
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

fn parse_time_ps_local(s: &str) -> Option<u64> {
    let s = s.trim();
    for &(unit, factor) in TIME_UNIT_FACTORS_PS_LOCAL {
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

// ── Frame quantization (v0.9.1 NC soundness) ─────────────────────────
//
// Bytes-level NC undercounts by up to one MTU per hop because frames
// are atomic — once a frame starts transmitting, the link is held
// until the frame finishes, even if the tagged stream's α slope
// suggests the link could be released earlier. Adding the per-hop
// frame-quantization correction
//
//     q_ps = ceil(max_frame_bytes · 8 · 10^12 / link_rate_bps)
//
// turns a slightly-optimistic answer into a sound one. The correction
// is applied in the non-preemption / non-CBS arms of the TSN dispatch
// (preemption replaces this term with the much smaller fragment time;
// CBS service curve already accounts for max-frame blocking via the
// 1518-byte default in `cbs_residual_service`). See PR #186 / v0.9.1
// soundness pass.

/// Per-hop frame-quantization correction in picoseconds.
///
/// Returns `ceil(max_frame_bytes · 8 · 10^12 / link_rate_bps)` —
/// the worst-case time it takes to drain one full preemptable Ethernet
/// frame at the egress link rate, with rounding *up* so the term is
/// never an under-estimate. `link_rate_bps == 0` returns `u64::MAX`
/// (a safe pessimistic placeholder; the caller is responsible for
/// screening that pathological case).
///
/// This is the atomic-frame correction for the bytes-level NC kernel:
/// without it, the per-hop bound undercounts by up to one MTU because
/// frames cannot be split mid-transmission. Apply once per hop in the
/// TAS / FIFO arms of the WCTT dispatch; do *not* apply in the
/// preemption arm (the fragment-blocking term replaces it) or the CBS
/// arm (the closed-form latency already absorbs it).
pub fn frame_quantization_ps(max_frame_bytes: u64, link_rate_bps: u64) -> u64 {
    if link_rate_bps == 0 {
        return u64::MAX;
    }
    let numer = (max_frame_bytes as u128) * BITS_PER_BYTE * PS_PER_SECOND;
    let denom = link_rate_bps as u128;
    let ps = numer.div_ceil(denom);
    if ps > u64::MAX as u128 {
        u64::MAX
    } else {
        ps as u64
    }
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

    /// Longest contiguous closed (gate-shut-for-`cos`) run in the cycle,
    /// picoseconds, taken with wrap-around — a RAW gate-schedule metric.
    ///
    /// NOTE: this is **not** the sound rate-latency service-curve latency
    /// for multi-window schedules — it is OPTIMISTIC there (it charges only
    /// the single longest gap, ignoring the gap → sliver → gap composite).
    /// The service curves use [`GateSchedule::min_service_latency_ps`]
    /// instead (REQ-TSN-SVC-MULTIWIN-001); the two coincide only for a
    /// single open window per class and for even k-way splits. This
    /// accessor is retained for the raw "longest closed run" quantity.
    ///
    /// Returns `cycle_ps` if no window opens for `cos` (the gate is
    /// permanently closed; all of `cycle_ps` is the closed gap).
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

    /// Exact SOUND rate-latency latency `T_sound` for class `cos`
    /// (REQ-TSN-SVC-MULTIWIN-001) — the maximum horizontal deviation of the
    /// per-class minimum-service staircase below the long-term-rate line.
    ///
    /// Unlike [`GateSchedule::worst_case_latency`] (the longest single
    /// closed gap, which is OPTIMISTIC for uneven multi-window schedules),
    /// this is SOUND: the rate-latency curve `β(t) = R·ρ·(t − T_sound)₊`
    /// lower-bounds the true minimum service
    /// `S(t) = R · min over φ of (open-time for cos in [φ, φ+t])` for ALL t.
    /// For a single open window per class — or even k-way splits — it
    /// equals `worst_case_latency` (`cycle − open` and `(cycle − open)/k`);
    /// only uneven multi-window schedules are corrected upward.
    ///
    /// Method: `T_sound = maxₜ[t − S(t)/(R·ρ)] = maxᵩ maxₜ[t − open(φ,φ+t)/ρ]`.
    /// For a fixed phase φ taken at an open-window END (a gap start), the
    /// inner term rises during closed gaps and falls during open windows,
    /// so its maximum lies at an open-window START. Enumerating φ over every
    /// open-window end and t over every subsequent open-window start yields
    /// the exact maximum in O(W²) over the W open windows, entirely in
    /// integer arithmetic: maximise the candidate `t·open − o·cycle`
    /// (= `(t − o/ρ)·open`), then divide by `open` rounding UP — the
    /// pessimistic (sound) direction.
    pub fn min_service_latency_ps(&self, cos: ClassOfService) -> u64 {
        let bit = 1u8 << cos.0;
        let opens: Vec<(u64, u64)> = self
            .windows
            .iter()
            .filter(|w| w.allowed_cos_mask & bit != 0)
            .map(|w| (w.offset_ps, w.duration_ps))
            .collect();
        let open_total: u64 = opens.iter().map(|(_, d)| *d).sum();
        // Permanently closed: unservable — full-cycle latency, matching the
        // `worst_case_latency` contract `tas_residual_service` relies on.
        if open_total == 0 {
            return self.cycle_ps;
        }
        // Permanently open: zero latency.
        if open_total >= self.cycle_ps {
            return 0;
        }
        let cycle = self.cycle_ps as i128;
        let open = open_total as i128;
        let w = opens.len();
        let mut best: i128 = 0; // T_sound is ≥ 0
        for i in 0..w {
            // φ = end of open window i = start of a closed gap.
            let phi_end = opens[i].0 as i128 + opens[i].1 as i128;
            let mut o_accum: i128 = 0; // open time strictly before window j
            for k in 1..=w {
                let j = (i + k) % w;
                // Forward cyclic distance φ → start of window j, in [0, cycle).
                let t = (opens[j].0 as i128 - phi_end).rem_euclid(cycle);
                let cand = t * open - o_accum * cycle;
                best = best.max(cand);
                o_accum += opens[j].1 as i128;
            }
        }
        // ceil(best / open) — round the latency UP to stay sound.
        ((best + open - 1) / open) as u64
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
    // Sound rate-latency latency (REQ-TSN-SVC-MULTIWIN-001) — exact for
    // multi-window schedules, equal to the longest gap for single windows.
    let latency_ps = schedule.min_service_latency_ps(cos);

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

/// Derive the rate-latency [`ServiceCurve`] offered to a single
/// class-of-service by a TAS-gated egress port, accounting for the
/// per-hop gPTP synchronization-error budget ε.
///
/// This is the v0.9.1 NC soundness wrapper around
/// [`tas_residual_service`]. In a real TSN deployment the gate-control
/// list is enforced against the local synchronized clock, which differs
/// from the master clock by at most ε per hop (IEEE 802.1AS / gPTP).
/// Without accounting for ε the TAS bound is *technically unsound* — a
/// frame can miss its window by ε.
///
/// The correction is: subtract ε from the effective open time
/// (clamped at 0 — once ε exceeds the open time, the gate is
/// effectively closed for that hop) and add ε to the worst-case gate
/// latency. Both adjustments tighten the envelope in the safe (more
/// pessimistic) direction.
///
/// `sync_error_ps == 0` reproduces [`tas_residual_service`]
/// byte-identically (legacy v0.8.1 behaviour, non-regression contract).
pub fn tas_residual_service_with_sync_error(
    schedule: &GateSchedule,
    cos: ClassOfService,
    link_rate_bps: u64,
    sync_error_ps: u64,
) -> ServiceCurve {
    let (open_ps, cycle_ps) = schedule.open_fraction(cos);
    // Sound rate-latency latency (REQ-TSN-SVC-MULTIWIN-001); ε is added on
    // top below. Equals the longest gap for single-window schedules, so the
    // `sync_error_ps == 0` single-window non-regression contract holds.
    let latency_ps = schedule.min_service_latency_ps(cos);

    // Effective open time shrinks by ε (clamped at 0). Once ε ≥ open
    // time, the gate is effectively closed: rate = 0 with the worst-
    // case-latency-plus-ε falling back to the closed-gate
    // `(rate=0, latency=cycle_ps)` form via the saturating add below.
    let effective_open_ps = open_ps.saturating_sub(sync_error_ps);
    let effective_latency_ps = latency_ps.saturating_add(sync_error_ps);

    let rate_bps = if cycle_ps == 0 || effective_open_ps == 0 {
        0
    } else {
        let product = (link_rate_bps as u128).saturating_mul(effective_open_ps as u128);
        let r = product / (cycle_ps as u128);
        if r > u64::MAX as u128 {
            u64::MAX
        } else {
            r as u64
        }
    };
    ServiceCurve::rate_latency(rate_bps, effective_latency_ps)
}

impl GateSchedule {
    /// Serialize back to the `Gate_Control_List` blob format that
    /// [`GateSchedule::parse`] accepts: `offset_ns:duration_ns:0xMASK`
    /// entries joined by `;`. Window times are emitted in nanoseconds —
    /// exact only when every `offset_ps`/`duration_ps` is a whole number
    /// of nanoseconds, which holds for any schedule produced by `parse`
    /// (whose inputs are ns) or by [`synthesize_gcl`].
    pub fn to_gcl_blob(&self) -> String {
        let mut out = String::new();
        for (i, w) in self.windows.iter().enumerate() {
            if i > 0 {
                out.push(';');
            }
            out.push_str(&format!(
                "{}:{}:0x{:02X}",
                w.offset_ps / 1_000,
                w.duration_ps / 1_000,
                w.allowed_cos_mask
            ));
        }
        out
    }

    /// Serialize this gate schedule as an IEEE 802.1Qcw `ieee802-dot1q-sched`
    /// YANG instance document (NETCONF XML) for the egress port
    /// `port_name` — the deployable artifact a TSN switch consumes via
    /// `edit-config` (REQ-TSN-EXPORT-YANG-001), not a network-calculus bound.
    ///
    /// Each [`GateWindow`] becomes one `<gate-control-entry>` with
    /// `operation-name=set-gate-states`, `time-interval-value` the window
    /// duration in whole nanoseconds (`duration_ps / 1000`), and
    /// `gate-states-value` the window's `allowed_cos_mask` (the closed
    /// guard window emits mask 0). The cycle period is the rational
    /// `admin-cycle-time` = `cycle_ps / 1e12` seconds. `admin-gate-states`
    /// initializes all eight queues open (255). NO new dependency —
    /// hand-emitted XML, mirroring [`GateSchedule::to_gcl_blob`].
    ///
    /// **Precondition:** every `duration_ps` and `cycle_ps` is a whole
    /// number of nanoseconds. `time-interval-value` is integer ns, so a
    /// sub-nanosecond duration would truncate and the emitted intervals
    /// would no longer sum to `admin-cycle-time` (which is emitted from
    /// `cycle_ps` directly). Both producers — [`GateSchedule::parse`] and
    /// [`synthesize_gcl`] — guarantee whole-ns windows, so for any
    /// schedule they build `Σ time-interval-value == admin-cycle-time`;
    /// the `debug_assert` upholds that contract at this boundary.
    pub fn to_qcw_yang(&self, port_name: &str) -> String {
        // Picoseconds per second — the denominator of the `admin-cycle-time`
        // rational, since every interval in this crate is stored in ps.
        const PS_PER_SECOND: u64 = 1_000_000_000_000;
        debug_assert!(
            self.cycle_ps.is_multiple_of(1_000)
                && self
                    .windows
                    .iter()
                    .all(|w| w.duration_ps.is_multiple_of(1_000)),
            "to_qcw_yang requires whole-nanosecond windows and cycle \
             (guaranteed by GateSchedule::parse and synthesize_gcl)"
        );
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<interfaces xmlns=\"urn:ietf:params:xml:ns:yang:ietf-interfaces\">\n");
        out.push_str("  <interface>\n");
        out.push_str(&format!("    <name>{}</name>\n", xml_escape(port_name)));
        out.push_str(
            "    <gate-parameter-table xmlns=\"urn:ieee:std:802.1Q:yang:ieee802-dot1q-sched\">\n",
        );
        out.push_str("      <gate-enabled>true</gate-enabled>\n");
        out.push_str("      <admin-gate-states>255</admin-gate-states>\n");
        out.push_str("      <admin-control-list>\n");
        for (i, w) in self.windows.iter().enumerate() {
            // gate-states-value and time-interval-value are DIRECT siblings
            // of operation-name in the entry (ieee802-dot1q-sched augments
            // the dot1q-types base-gate-control-entries grouping — there is
            // no `sgs-params` wrapper, and no `admin-control-list-length`
            // leaf: length is enforced by a `must` count constraint).
            out.push_str("        <gate-control-entry>\n");
            out.push_str(&format!("          <index>{}</index>\n", i));
            out.push_str("          <operation-name>set-gate-states</operation-name>\n");
            out.push_str(&format!(
                "          <time-interval-value>{}</time-interval-value>\n",
                w.duration_ps / 1_000
            ));
            out.push_str(&format!(
                "          <gate-states-value>{}</gate-states-value>\n",
                w.allowed_cos_mask
            ));
            out.push_str("        </gate-control-entry>\n");
        }
        out.push_str("      </admin-control-list>\n");
        out.push_str("      <admin-cycle-time>\n");
        out.push_str(&format!(
            "        <numerator>{}</numerator>\n",
            self.cycle_ps
        ));
        out.push_str(&format!(
            "        <denominator>{}</denominator>\n",
            PS_PER_SECOND
        ));
        out.push_str("      </admin-cycle-time>\n");
        out.push_str("    </gate-parameter-table>\n");
        out.push_str("  </interface>\n");
        out.push_str("</interfaces>\n");
        out
    }
}

/// Minimal XML text escaping for values embedded in the
/// `ieee802-dot1q-sched` instance document emitted by
/// [`GateSchedule::to_qcw_yang`]. Ampersand is replaced first so the
/// entities introduced by the later replacements are not double-escaped.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ── 802.1Qbv GCL synthesis (REQ-TSN-SYNTH-QBV-BASE-001) ──────────────
//
// The inverse of TAS checking ([`tas_residual_service`]): given per-class
// demand at one egress port, produce a gate-control list whose residual
// service curve meets every class's deadline. The baseline allocates one
// contiguous open window per class plus a closed guard window; window
// SPLITTING (the lower-latency lever) is the follow-up REQ-TSN-SYNTH-QBV-001.

/// Per-class traffic demand at one TAS-gated egress port — the input unit
/// to [`synthesize_gcl`].
#[derive(Debug, Clone)]
pub struct ClassDemand {
    /// The 802.1Q class of service (priority) this demand occupies. Each
    /// class gets exactly one open window in the synthesized GCL.
    pub cos: ClassOfService,
    /// Aggregate arrival curve for all flows of this class at the port.
    pub arrival: ArrivalCurve,
    /// Per-hop deadline budget for this class at this gate, picoseconds
    /// (the share of the end-to-end deadline allotted to this hop).
    pub deadline_ps: u64,
}

/// Errors returned by [`synthesize_gcl`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GclSynthError {
    /// The demand set was empty.
    NoDemands,
    /// `cycle_ps` is zero or not a whole number of nanoseconds. The GCL
    /// blob format quantizes window times to nanoseconds, so the cycle
    /// must be a whole-nanosecond value.
    CycleNotWholeNanos {
        /// The rejected cycle period, picoseconds.
        cycle_ps: u64,
    },
    /// Two demands name the same class of service. A class may appear at
    /// most once (the single-window invariant the bound math relies on).
    DuplicateClass {
        /// The duplicated 802.1Q priority.
        cos: u8,
    },
    /// Even a window spanning the entire cycle cannot meet this class's
    /// deadline: the deadline is below the irreducible σ/link
    /// transmission delay of the burst at full line rate (or the class's
    /// sustained rate exceeds the whole link). Widen the deadline, raise
    /// the link rate, or shrink the burst.
    ClassInfeasible {
        /// The class that cannot be served.
        cos: u8,
        /// The deadline that could not be met, picoseconds.
        deadline_ps: u64,
        /// The smallest delay achievable for this class (a full-cycle
        /// window); `u64::MAX` if the class is unservable at any width.
        achievable_ps: u64,
    },
    /// The sum of the minimal per-class window durations exceeds the
    /// cycle — the port is oversubscribed. Lengthen the cycle, raise the
    /// link rate, or relax deadlines.
    Oversubscribed {
        /// Total window time the demands require, picoseconds.
        required_ps: u64,
        /// The available cycle period, picoseconds.
        cycle_ps: u64,
    },
    /// Self-certification failed: the synthesized GCL did not round-trip
    /// through [`GateSchedule::parse`], or a class missed its deadline on
    /// re-check with the real network-calculus bound. Indicates a
    /// synthesizer bug — surfaced as an error rather than an unsound GCL.
    SelfCheck(String),
}

impl core::fmt::Display for GclSynthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoDemands => write!(f, "no class demands supplied"),
            Self::CycleNotWholeNanos { cycle_ps } => {
                write!(f, "cycle period {} ps is not a whole nanosecond", cycle_ps)
            }
            Self::DuplicateClass { cos } => {
                write!(f, "class of service {} appears more than once", cos)
            }
            Self::ClassInfeasible {
                cos,
                deadline_ps,
                achievable_ps,
            } => write!(
                f,
                "class {} deadline {} ps is unachievable (best {} ps at full cycle)",
                cos, deadline_ps, achievable_ps
            ),
            Self::Oversubscribed {
                required_ps,
                cycle_ps,
            } => write!(
                f,
                "port oversubscribed: {} ps of window time required, {} ps cycle",
                required_ps, cycle_ps
            ),
            Self::SelfCheck(msg) => write!(f, "synthesis self-check failed: {}", msg),
        }
    }
}

impl core::error::Error for GclSynthError {}

/// Synthesize a 802.1Qbv gate-control list for one TAS-gated egress port.
///
/// Each [`ClassDemand`] receives one contiguous open window
/// (`allowed_cos_mask = 1 << cos`); the leftover cycle time becomes a
/// closed guard window (`mask = 0`). Window durations are sized by a
/// monotone search so each class's [`tas_residual_service`] curve meets
/// its deadline under the EXACT [`delay_bound`] — the same sound bound the
/// analyzer uses, run forward on the synthesizer's own output. The result
/// is self-certified through [`GateSchedule::parse`] + per-class
/// [`delay_bound`] before return.
///
/// Latency-pessimistic by construction: with one window per class the
/// worst-case gate latency is `cycle − duration`, independent of window
/// placement — so window ORDER is a no-op on the bounds. Closing the
/// residual gap needs window SPLITTING (REQ-TSN-SYNTH-QBV-001).
///
/// `cycle_ps` must be a whole number of nanoseconds (the GCL blob format
/// is nanosecond-quantized). Returns a [`GclSynthError`] distinguishing
/// per-class infeasibility from port oversubscription.
pub fn synthesize_gcl(
    demands: &[ClassDemand],
    cycle_ps: u64,
    link_rate_bps: u64,
) -> Result<GateSchedule, GclSynthError> {
    if demands.is_empty() {
        return Err(GclSynthError::NoDemands);
    }
    if cycle_ps == 0 || !cycle_ps.is_multiple_of(1_000) {
        return Err(GclSynthError::CycleNotWholeNanos { cycle_ps });
    }
    // A class may appear at most once — two windows for the same CoS
    // would break the single-window `T_K = cycle − duration` invariant.
    for (i, a) in demands.iter().enumerate() {
        for b in &demands[i + 1..] {
            if a.cos == b.cos {
                return Err(GclSynthError::DuplicateClass { cos: a.cos.0 });
            }
        }
    }

    let cycle_ns = cycle_ps / 1_000;

    // Per-class minimal window duration (ns). delay is strictly
    // decreasing in window duration, so binary-search the smallest
    // duration whose single-window schedule meets the deadline — using
    // the REAL checker as the predicate (no parallel closed-form, so
    // integer rounding cannot diverge from the oracle).
    let mut durations_ns: Vec<u64> = Vec::with_capacity(demands.len());
    for d in demands {
        // Feasibility ceiling: a full-cycle window (latency 0, rate =
        // link) is the best this class can ever get.
        let full = class_delay_ps(d, cycle_ns, cycle_ns, link_rate_bps);
        if !matches!(full, Some(delay) if delay <= d.deadline_ps) {
            return Err(GclSynthError::ClassInfeasible {
                cos: d.cos.0,
                deadline_ps: d.deadline_ps,
                achievable_ps: full.unwrap_or(u64::MAX),
            });
        }
        // Binary-search the smallest feasible duration in 1..=cycle_ns.
        let mut lo = 1u64;
        let mut hi = cycle_ns;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let ok = matches!(
                class_delay_ps(d, mid, cycle_ns, link_rate_bps),
                Some(delay) if delay <= d.deadline_ps
            );
            if ok {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        durations_ns.push(lo);
    }

    let required_ns: u64 = durations_ns.iter().sum();
    if required_ns > cycle_ns {
        return Err(GclSynthError::Oversubscribed {
            required_ps: required_ns.saturating_mul(1_000),
            cycle_ps,
        });
    }

    // Emit one open window per class (declaration order — a no-op on the
    // bounds), then a closed guard window for any leftover.
    let mut blob = String::new();
    let mut offset_ns = 0u64;
    for (d, &dur_ns) in demands.iter().zip(&durations_ns) {
        if !blob.is_empty() {
            blob.push(';');
        }
        let mask = 1u8 << d.cos.0;
        blob.push_str(&format!("{}:{}:0x{:02X}", offset_ns, dur_ns, mask));
        offset_ns += dur_ns;
    }
    if offset_ns < cycle_ns {
        blob.push(';');
        blob.push_str(&format!("{}:{}:0x00", offset_ns, cycle_ns - offset_ns));
    }

    // Self-certify against the real parser + checker. By construction
    // this always passes; a failure here is a synthesizer bug, surfaced
    // as an error rather than an unsound schedule reaching the caller.
    let sched = GateSchedule::parse(&blob)
        .map_err(|e| GclSynthError::SelfCheck(format!("re-parse failed for {:?}: {}", blob, e)))?;
    for d in demands {
        let beta = tas_residual_service(&sched, d.cos, link_rate_bps);
        match delay_bound(&d.arrival, &beta) {
            Ok(delay) if delay <= d.deadline_ps => {}
            Ok(delay) => {
                return Err(GclSynthError::SelfCheck(format!(
                    "class {} delay {} ps exceeds deadline {} ps after synthesis",
                    d.cos.0, delay, d.deadline_ps
                )));
            }
            Err(e) => {
                return Err(GclSynthError::SelfCheck(format!(
                    "class {} unservable after synthesis: {:?}",
                    d.cos.0, e
                )));
            }
        }
    }
    Ok(sched)
}

/// Worst-case delay (ps) for `demand` if its class receives a single open
/// window of `dur_ns` in a `cycle_ns` cycle on a `link_rate_bps` link,
/// computed via the real TAS checker ([`tas_residual_service`] +
/// [`delay_bound`]). `None` when the resulting server is unstable (the
/// class's sustained rate exceeds the window's residual rate).
fn class_delay_ps(
    demand: &ClassDemand,
    dur_ns: u64,
    cycle_ns: u64,
    link_rate_bps: u64,
) -> Option<u64> {
    let cycle_ps = cycle_ns.saturating_mul(1_000);
    let dur_ps = dur_ns.saturating_mul(1_000);
    let bit = 1u8 << demand.cos.0;
    let windows = if dur_ns >= cycle_ns {
        vec![GateWindow {
            offset_ps: 0,
            duration_ps: cycle_ps,
            allowed_cos_mask: bit,
        }]
    } else {
        vec![
            GateWindow {
                offset_ps: 0,
                duration_ps: dur_ps,
                allowed_cos_mask: bit,
            },
            GateWindow {
                offset_ps: dur_ps,
                duration_ps: cycle_ps - dur_ps,
                allowed_cos_mask: 0,
            },
        ]
    };
    let sched = GateSchedule { windows, cycle_ps };
    let beta = tas_residual_service(&sched, demand.cos, link_rate_bps);
    delay_bound(&demand.arrival, &beta).ok()
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

    // ── CBS (802.1Qav) — v0.8.1 commit 3 ─────────────────────────────

    /// 1 Gbps in bits/second — link rate used by the CBS unit tests.
    const GBPS: u64 = 1_000_000_000;

    #[test]
    fn cbs_reservation_construct_basic() {
        // 30% reservation on a 1 Gbps link: idleSlope = 300 Mbps,
        // sendSlope = 300 Mbps − 1 Gbps = −700 Mbps.
        let r = CbsReservation::new(300_000_000, GBPS, 12_000, 1500).unwrap();
        assert_eq!(r.idle_slope_bps, 300_000_000);
        assert_eq!(r.send_slope_bps, -700_000_000);
        assert_eq!(r.hi_credit_bytes, 12_000);
        assert_eq!(r.lo_credit_bytes, 1500);
    }

    #[test]
    fn cbs_reservation_rejects_oversubscription() {
        // Reservation > link rate is infeasible.
        assert_eq!(CbsReservation::new(GBPS + 1, GBPS, 0, 0), None);
        // Zero link rate is also rejected (guards the divisions).
        assert_eq!(CbsReservation::new(0, 0, 0, 0), None);
    }

    #[test]
    fn cbs_reservation_at_link_rate_has_zero_send_slope() {
        // idleSlope == linkRate ⇒ sendSlope = 0 (boundary, no other
        // class can use the link).
        let r = CbsReservation::new(GBPS, GBPS, 0, 0).unwrap();
        assert_eq!(r.idle_slope_bps, GBPS);
        assert_eq!(r.send_slope_bps, 0);
    }

    #[test]
    fn cbs_reservation_reserved_fraction_recovers_link_rate() {
        let r = CbsReservation::new(250_000_000, GBPS, 0, 0).unwrap();
        let (num, den) = r.reserved_fraction();
        assert_eq!(num, 250_000_000);
        assert_eq!(den, GBPS);
    }

    #[test]
    fn cbs_residual_service_rate_equals_idle_slope() {
        let r = CbsReservation::new(300_000_000, GBPS, 0, 0).unwrap();
        let beta = cbs_residual_service(&r, GBPS, 1518);
        assert_eq!(
            beta.rate_bps, 300_000_000,
            "service rate must equal idleSlope (the reserved bps)"
        );
    }

    #[test]
    fn cbs_residual_service_blocking_term_dominates_when_lo_credit_zero() {
        // With lo_credit = 0 the recovery term is 0; latency is purely
        // the worst-case blocking from the largest competing frame.
        // 1518-byte frame at 1 Gbps:
        //   blocking_ps = 1518 · 8 · 10^12 / 10^9
        //               = 12_144 · 10^3 ps
        //               = 12_144_000 ps (rounded up — exact here).
        let r = CbsReservation::new(300_000_000, GBPS, 0, 0).unwrap();
        let beta = cbs_residual_service(&r, GBPS, 1518);
        assert_eq!(beta.latency_ps, 12_144_000);
    }

    #[test]
    fn cbs_residual_service_credit_recovery_term() {
        // Closed-form latency = blocking + lo_credit / |sendSlope|.
        //
        // 1 Gbps link, idleSlope 300 Mbps ⇒ |sendSlope| = 700 Mbps.
        //
        //   blocking = 1518 · 8 · 10^12 / 10^9 = 12_144_000 ps.
        //
        //   recovery = 1500 · 8 · 10^12 / 7·10^8
        //            = 12_000 · 10^12 / 7·10^8
        //            = 17_142_857.14 ps → 17_142_858 (rounded up).
        //
        //   total = 12_144_000 + 17_142_858 = 29_286_858 ps.
        let r = CbsReservation::new(300_000_000, GBPS, 12_000, 1500).unwrap();
        let beta = cbs_residual_service(&r, GBPS, 1518);
        assert_eq!(beta.rate_bps, 300_000_000);
        // Rounding-up on the recovery term is required for safe
        // pessimism direction.
        let blocking = 12_144_000u64;
        let recovery = (1500u128 * 8u128 * 1_000_000_000_000u128).div_ceil(700_000_000u128) as u64;
        assert_eq!(beta.latency_ps, blocking + recovery);
    }

    #[test]
    fn cbs_residual_service_idle_slope_equals_link_rate_no_blocking() {
        // Boundary: idleSlope == linkRate ⇒ sendSlope == 0. The
        // closed-form's recovery term is zero, and any "competing
        // class" cannot exist (the class has the link to itself).
        // Latency is just the configured blocking term, which we
        // expect callers to set to zero in this case.
        let r = CbsReservation::new(GBPS, GBPS, 0, 0).unwrap();
        let beta = cbs_residual_service(&r, GBPS, 0);
        assert_eq!(beta.rate_bps, GBPS);
        assert_eq!(beta.latency_ps, 0);
    }

    #[test]
    fn cbs_two_classes_each_30pct_get_each_their_idle_slope() {
        // Two classes, each reserving 30% of a 1 Gbps link. Both must
        // see 300 Mbps service rate; the latency captures the same
        // worst-case blocking (the largest competing frame).
        let class_a = CbsReservation::new(300_000_000, GBPS, 12_000, 1500).unwrap();
        let class_b = CbsReservation::new(300_000_000, GBPS, 12_000, 1500).unwrap();
        let beta_a = cbs_residual_service(&class_a, GBPS, 1518);
        let beta_b = cbs_residual_service(&class_b, GBPS, 1518);
        assert_eq!(beta_a.rate_bps, 300_000_000);
        assert_eq!(beta_b.rate_bps, 300_000_000);
        assert_eq!(beta_a.latency_ps, beta_b.latency_ps);
        // 30% + 30% = 60%, leaving 40% for best-effort and the other
        // CBS class. Reservation feasibility is preserved (both
        // succeeded above).
    }

    #[test]
    fn read_bandwidth_reservation_typed_and_string_paths() {
        // Typed integer with explicit unit.
        let props = make_props(
            SPAR_TSN,
            "Bandwidth_Reservation",
            "1000000",
            Some(PropertyExpr::Integer(
                1000,
                Some(spar_hir_def::name::Name::new("KBytesps")),
            )),
        );
        // 1000 KBytesps = 1000 · 8 · 1024 = 8_192_000 bps.
        assert_eq!(get_bandwidth_reservation_bps(&props), Some(8_192_000));

        // Bare integer (interpreted as bps).
        let props_bare = make_props(
            SPAR_TSN,
            "Bandwidth_Reservation",
            "300000000",
            Some(PropertyExpr::Integer(300_000_000, None)),
        );
        assert_eq!(
            get_bandwidth_reservation_bps(&props_bare),
            Some(300_000_000)
        );

        // String fallback path — `100 MBytesps` → 100 · 8 · 1024^2.
        let props_str = make_props(SPAR_TSN, "Bandwidth_Reservation", "100 MBytesps", None);
        assert_eq!(
            get_bandwidth_reservation_bps(&props_str),
            Some(100u64 * 8 * 1024 * 1024)
        );

        // Missing returns None.
        let empty = PropertyMap::new();
        assert_eq!(get_bandwidth_reservation_bps(&empty), None);
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

    // ── REQ-TSN-SVC-MULTIWIN-001: sound TAS service-curve latency ────────

    /// INDEPENDENT min-service oracle: min over every integer-ns phase φ of
    /// the open-ps for `cos` in [φ, φ+t]. Brute force (small test cycles
    /// only) — deliberately a different algorithm from the production
    /// breakpoint method in `min_service_latency_ps`.
    fn min_open_ps(sched: &GateSchedule, cos: ClassOfService, t_ps: u64) -> u64 {
        let bit = 1u8 << cos.0;
        let cyc_ns = (sched.cycle_ps / 1000) as usize;
        let mut open = vec![false; cyc_ns];
        for w in &sched.windows {
            if w.allowed_cos_mask & bit != 0 {
                let s = (w.offset_ps / 1000) as usize;
                let e = ((w.offset_ps + w.duration_ps) / 1000) as usize;
                for slot in open.iter_mut().take(e).skip(s) {
                    *slot = true;
                }
            }
        }
        let t_ns = (t_ps / 1000) as usize;
        let mut min_o = u64::MAX;
        for phi in 0..cyc_ns {
            let mut o = 0u64;
            for k in 0..t_ns {
                if open[(phi + k) % cyc_ns] {
                    o += 1000;
                }
            }
            min_o = min_o.min(o);
        }
        min_o
    }

    /// Exact, R-free soundness predicate: β_sound(t) ≤ S(t)
    /// ⟺ open_total·(t − T_sound)₊ ≤ min_open(t)·cycle (all in ps).
    fn beta_sound_le_s(sched: &GateSchedule, cos: ClassOfService, t_ps: u64) -> bool {
        let (open_total, cycle) = sched.open_fraction(cos);
        let tsound = sched.min_service_latency_ps(cos);
        let lhs = (open_total as u128) * (t_ps.saturating_sub(tsound) as u128);
        let rhs = (min_open_ps(sched, cos, t_ps) as u128) * (cycle as u128);
        lhs <= rhs
    }

    #[test]
    fn tas_sound_holds_across_split_battery() {
        // β_sound(t) ≤ true min-service S(t) for ALL t, swept at 1ns over
        // [0, 2·cycle], across single / even / uneven / multi-class shapes.
        let cos0 = cos(0);
        let cases = [
            "0:40:0x01;40:60:0x00",                        // single 40/100
            "0:20:0x01;20:30:0x00;50:20:0x01;70:30:0x00",  // even 2-split
            "0:30:0x01;30:30:0x00;60:10:0x01;70:30:0x00",  // uneven 30+10
            "0:35:0x01;35:30:0x00;65:5:0x01;70:30:0x00",   // uneven 35+5
            "0:20:0x03;20:20:0x00;40:20:0x01;60:40:0x00",  // multi-class: cos0 uneven
        ];
        for blob in cases {
            let s = GateSchedule::parse(blob).unwrap();
            let mut t = 0u64;
            while t <= 2 * s.cycle_ps {
                assert!(
                    beta_sound_le_s(&s, cos0, t),
                    "UNSOUND β>S at t={}ps for `{}` (T_sound={})",
                    t,
                    blob,
                    s.min_service_latency_ps(cos0)
                );
                t += 1_000;
            }
        }
    }

    #[test]
    fn tas_sound_single_window_matches_longest_gap() {
        // Non-regression: for one window per class, the sound latency equals
        // the legacy longest-gap value (cycle − open), so shipped
        // single-window synthesizer output is byte-unchanged.
        let s = GateSchedule::parse("0:40:0x01;40:60:0x00").unwrap();
        assert_eq!(s.min_service_latency_ps(cos(0)), s.worst_case_latency(cos(0)));
        assert_eq!(s.min_service_latency_ps(cos(0)), 60_000);
    }

    #[test]
    fn tas_sound_even_split_is_cycle_minus_open_over_k() {
        // Even k-split → (cycle − open)/k, exactly (the tight, sound lever).
        let s2 = GateSchedule::parse("0:20:0x01;20:30:0x00;50:20:0x01;70:30:0x00").unwrap();
        assert_eq!(s2.min_service_latency_ps(cos(0)), 30_000); // (100−40)/2
        let s4 = GateSchedule::parse(
            "0:10:0x01;10:15:0x00;25:10:0x01;35:15:0x00;\
             50:10:0x01;60:15:0x00;75:10:0x01;85:15:0x00",
        )
        .unwrap();
        assert_eq!(s4.min_service_latency_ps(cos(0)), 15_000); // (100−40)/4
    }

    #[test]
    fn tas_sound_fixes_pinned_counterexamples() {
        // The two schedules where the OLD longest-gap β exceeded S. The
        // sound latency is strictly larger (the optimism removed) and
        // β_sound ≤ S now holds at the previously-failing t.
        let cos0 = cos(0);
        let c1 =
            GateSchedule::parse("0:4000:0x00;4000:500:0x01;4500:4000:0x00;8500:1000:0x01").unwrap();
        assert!(c1.min_service_latency_ps(cos0) > c1.worst_case_latency(cos0));
        assert_eq!(c1.min_service_latency_ps(cos0), 5_333_334); // ceil(8e12/1.5e6) ps
        assert!(beta_sound_le_s(&c1, cos0, 8_500_000));

        let c2 =
            GateSchedule::parse("0:200:0x01;200:5000:0x00;5200:1800:0x01;7000:5000:0x00").unwrap();
        assert!(c2.min_service_latency_ps(cos0) > c2.worst_case_latency(cos0));
        assert!(beta_sound_le_s(&c2, cos0, 10_200_000));
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

    // ── Sync_Error / gPTP ε budget (v0.9.1 NC soundness) ─────────────

    #[test]
    fn read_sync_error_typed_path_picoseconds() {
        // Bare integer — interpreted as picoseconds.
        let props = make_props(
            SPAR_TSN,
            "Sync_Error",
            "1000",
            Some(PropertyExpr::Integer(1_000, None)),
        );
        assert_eq!(get_sync_error_ps(&props), Some(1_000));
    }

    #[test]
    fn read_sync_error_typed_path_with_units() {
        // 1 us with explicit Time_Units → 1_000_000 ps.
        let props = make_props(
            SPAR_TSN,
            "Sync_Error",
            "1us",
            Some(PropertyExpr::Integer(1, Some(Name::new("us")))),
        );
        assert_eq!(get_sync_error_ps(&props), Some(1_000_000));

        // 1 ms.
        let props_ms = make_props(
            SPAR_TSN,
            "Sync_Error",
            "1ms",
            Some(PropertyExpr::Integer(1, Some(Name::new("ms")))),
        );
        assert_eq!(get_sync_error_ps(&props_ms), Some(1_000_000_000));
    }

    #[test]
    fn read_sync_error_string_fallback_matches_typed() {
        // The string-fallback path should agree with the typed-expr
        // path for the same human-readable value.
        let typed = make_props(
            SPAR_TSN,
            "Sync_Error",
            "1us",
            Some(PropertyExpr::Integer(1, Some(Name::new("us")))),
        );
        let stringy = make_props(SPAR_TSN, "Sync_Error", "1us", None);
        assert_eq!(get_sync_error_ps(&typed), get_sync_error_ps(&stringy));
        assert_eq!(get_sync_error_ps(&stringy), Some(1_000_000));
    }

    #[test]
    fn read_sync_error_missing_returns_none() {
        // Unset = legacy behaviour (callers treat as ε = 0).
        let empty = PropertyMap::new();
        assert_eq!(get_sync_error_ps(&empty), None);
    }

    // ── Frame quantization (v0.9.1 NC soundness) ─────────────────────

    #[test]
    fn frame_quantization_1518_at_100mbps() {
        // 1518-byte MTU at 100 Mbps:
        //   q_ps = ceil(1518 · 8 · 10^12 / 10^8) = 121_440_000 ps = 121.44 us.
        // This matches the legacy preemption-disabled blocking term.
        let q = frame_quantization_ps(1518, 100_000_000);
        assert_eq!(q, 121_440_000);
    }

    #[test]
    fn frame_quantization_64_at_1gbps() {
        // 64-byte minimum frame at 1 Gbps:
        //   q_ps = ceil(64 · 8 · 10^12 / 10^9) = 512_000 ps = 512 ns.
        let q = frame_quantization_ps(64, 1_000_000_000);
        assert_eq!(q, 512_000);
    }

    #[test]
    fn frame_quantization_rounds_up() {
        // Rate that does not divide evenly — ceiling rounding (pessimism)
        // must produce a strict over-estimate vs. the exact-divisor case.
        // 1518 bytes at 1 Gbps is exact (12_144_000 ps). At 999_999_999
        // bps (1 bps slower) we should round *up*.
        let exact = frame_quantization_ps(1518, 1_000_000_000);
        let nearly = frame_quantization_ps(1518, 999_999_999);
        assert_eq!(exact, 12_144_000);
        assert!(
            nearly > exact,
            "ceiling rounding must over-estimate at non-exact divisors: \
             exact={} nearly={}",
            exact,
            nearly
        );
    }

    #[test]
    fn frame_quantization_zero_rate_guard() {
        // Pathological zero-rate input: returns u64::MAX so the caller
        // can detect the unservable hop without panicking on division.
        assert_eq!(frame_quantization_ps(1518, 0), u64::MAX);
        assert_eq!(frame_quantization_ps(0, 0), u64::MAX);
    }

    #[test]
    fn frame_quantization_zero_bytes_returns_zero() {
        // Zero bytes is a degenerate but well-defined case: takes zero
        // time to transmit. Used by tests that want to verify the
        // correction is a strict additive overhead (i.e., setting it
        // to zero recovers the v0.8.1 bound).
        assert_eq!(frame_quantization_ps(0, 1_000_000_000), 0);
    }

    // ── TAS + gPTP ε budget (v0.9.1 NC soundness) ────────────────────

    #[test]
    fn tas_residual_service_with_sync_error_zero_matches_legacy() {
        // ε = 0 must reproduce the legacy `tas_residual_service`
        // byte-identically — this is the non-regression contract.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        let legacy = tas_residual_service(&s, cos7, TAS_GBPS);
        let with_eps = tas_residual_service_with_sync_error(&s, cos7, TAS_GBPS, 0);
        assert_eq!(legacy.rate_bps, with_eps.rate_bps);
        assert_eq!(legacy.latency_ps, with_eps.latency_ps);
    }

    #[test]
    fn tas_residual_service_with_sync_error_shrinks_open_grows_latency() {
        // 50% open (5 us out of 10 us), 1 Gbps. Apply ε = 1 us.
        //   effective_open  = 5 us − 1 us = 4 us
        //   rate = 1 Gbps · 4/10 = 400 Mbps
        //   effective_lat  = 5 us + 1 us = 6 us
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        let svc = tas_residual_service_with_sync_error(&s, cos7, TAS_GBPS, 1_000_000);
        assert_eq!(svc.rate_bps, 400_000_000);
        assert_eq!(svc.latency_ps, 6_000_000);
    }

    #[test]
    fn tas_residual_service_with_sync_error_clamps_when_eps_equals_open() {
        // ε equal to the open time → gate is effectively closed
        // (rate = 0); latency = open_window_latency + ε.
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        // Open time for CoS 7 is 5 us = 5_000_000 ps.
        let svc = tas_residual_service_with_sync_error(&s, cos7, TAS_GBPS, 5_000_000);
        assert_eq!(svc.rate_bps, 0, "gate must close once ε ≥ open time");
        assert_eq!(svc.latency_ps, 10_000_000, "5us legacy + 5us ε");
    }

    #[test]
    fn tas_residual_service_with_sync_error_clamps_when_eps_exceeds_open() {
        // ε strictly greater than open time → still closed; saturating
        // sub clamps at 0 (the gate cannot have "negative open time").
        let s = GateSchedule::parse("0:5000:0x80;5000:5000:0x7F").unwrap();
        let cos7 = ClassOfService::new(7).unwrap();
        let svc = tas_residual_service_with_sync_error(&s, cos7, TAS_GBPS, 10_000_000);
        assert_eq!(svc.rate_bps, 0);
        assert_eq!(svc.latency_ps, 15_000_000);
    }

    // ── QBV GCL synthesis (REQ-TSN-SYNTH-QBV-BASE-001) ───────────────
    //
    // The oracle is the existing TAS checker run forward on the
    // synthesizer's own output: synthesize_gcl → GateSchedule::parse
    // round-trip (zero error) → tas_residual_service per class →
    // delay_bound ≤ deadline. No external reference numbers — the
    // checker (TEST-TSN-TAS) is the free, exact inverse.

    fn cos(c: u8) -> ClassOfService {
        ClassOfService::new(c).unwrap()
    }

    /// Assert that `sched` round-trips through the parser unchanged and
    /// meets every demand's deadline under the exact NC delay bound.
    fn assert_gcl_sound(sched: &GateSchedule, demands: &[ClassDemand], link_bps: u64) {
        // (1) Round-trip: re-emit and re-parse with zero error.
        let blob = sched.to_gcl_blob();
        let reparsed = GateSchedule::parse(&blob)
            .unwrap_or_else(|e| panic!("synthesized GCL {:?} failed to re-parse: {}", blob, e));
        assert_eq!(&reparsed, sched, "round-trip changed the schedule");
        // (2) Windows tile [0, cycle_ps): parse already enforces this, so
        // a successful re-parse is the tiling proof.
        // (3) Every class meets its deadline under the same sound bound
        // the analyzer uses.
        for d in demands {
            let beta = tas_residual_service(sched, d.cos, link_bps);
            let delay = delay_bound(&d.arrival, &beta).unwrap_or_else(|e| {
                panic!("class {} unservable on synthesized GCL: {:?}", d.cos.0, e)
            });
            assert!(
                delay <= d.deadline_ps,
                "class {} delay {} ps exceeds deadline {} ps",
                d.cos.0,
                delay,
                d.deadline_ps
            );
        }
    }

    #[test]
    fn synth_multiclass_roundtrips_and_meets_all_deadlines() {
        // Two classes sharing a 10 µs cycle on a 1 Gbps port. Distinct
        // CoS, affine arrivals, slack deadlines — clearly feasible.
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64; // 10 µs
        let demands = vec![
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000), // 125 B burst, 100 Mbps
                deadline_ps: 8_000_000,
            },
            ClassDemand {
                cos: cos(3),
                arrival: ArrivalCurve::affine(250, 200_000_000),
                deadline_ps: 9_000_000,
            },
        ];
        let sched = synthesize_gcl(&demands, cycle_ps, link).expect("feasible");
        assert_eq!(sched.cycle_ps, cycle_ps, "cycle preserved");
        assert_gcl_sound(&sched, &demands, link);
    }

    #[test]
    fn synth_tighter_deadline_widens_window() {
        // Monotone lever: a tighter deadline forces a wider open window
        // (delay is strictly decreasing in window duration).
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64;
        let mk = |deadline_ps| {
            vec![ClassDemand {
                cos: cos(5),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps,
            }]
        };
        let loose = synthesize_gcl(&mk(8_000_000), cycle_ps, link).expect("feasible");
        let tight = synthesize_gcl(&mk(4_000_000), cycle_ps, link).expect("feasible");
        let open = |s: &GateSchedule| s.open_fraction(cos(5)).0;
        assert!(
            open(&tight) >= open(&loose),
            "tighter deadline must not shrink the open window: tight {} < loose {}",
            open(&tight),
            open(&loose)
        );
        assert_gcl_sound(&tight, &mk(4_000_000), link);
    }

    #[test]
    fn synth_leftover_becomes_closed_guard_window() {
        // A single class that does not need the whole cycle leaves a
        // closed (mask=0) guard window — which perturbs no class's
        // worst-case latency.
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64;
        let demands = vec![ClassDemand {
            cos: cos(2),
            arrival: ArrivalCurve::affine(125, 100_000_000),
            deadline_ps: 9_500_000, // generous → small window, big leftover
        }];
        let sched = synthesize_gcl(&demands, cycle_ps, link).expect("feasible");
        let guard = sched.windows.last().expect("non-empty schedule");
        assert_eq!(guard.allowed_cos_mask, 0, "leftover must be a closed guard");
        assert_gcl_sound(&sched, &demands, link);
    }

    #[test]
    fn synth_rejects_class_below_irreducible_transmission_delay() {
        // σ = 1250 B = 10000 bits takes 10 µs to send at 1 Gbps even with
        // the gate fully open (latency 0). A 5 µs deadline is physically
        // impossible → ClassInfeasible, not Oversubscribed.
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64;
        let demands = vec![ClassDemand {
            cos: cos(6),
            arrival: ArrivalCurve::affine(1250, 50_000_000),
            deadline_ps: 5_000_000,
        }];
        let err = synthesize_gcl(&demands, cycle_ps, link).expect_err("infeasible");
        assert!(
            matches!(err, GclSynthError::ClassInfeasible { cos: 6, .. }),
            "expected ClassInfeasible for cos 6, got {:?}",
            err
        );
    }

    #[test]
    fn synth_rejects_oversubscribed_port() {
        // Two classes each needing > half the cycle: individually
        // feasible, jointly oversubscribed.
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64;
        let demands = vec![
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 5_500_000, // forces a ~5.5 µs window
            },
            ClassDemand {
                cos: cos(3),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 5_500_000,
            },
        ];
        let err = synthesize_gcl(&demands, cycle_ps, link).expect_err("oversubscribed");
        assert!(
            matches!(err, GclSynthError::Oversubscribed { .. }),
            "expected Oversubscribed, got {:?}",
            err
        );
    }

    #[test]
    fn synth_input_validation() {
        let link = 1_000_000_000u64;
        // No demands.
        assert_eq!(
            synthesize_gcl(&[], 10_000_000, link),
            Err(GclSynthError::NoDemands)
        );
        // Cycle not a whole number of nanoseconds.
        let d = vec![ClassDemand {
            cos: cos(7),
            arrival: ArrivalCurve::affine(125, 100_000_000),
            deadline_ps: 8_000_000,
        }];
        assert!(matches!(
            synthesize_gcl(&d, 10_000_001, link),
            Err(GclSynthError::CycleNotWholeNanos { .. })
        ));
        // Duplicate class of service.
        let dup = vec![
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
        ];
        assert!(matches!(
            synthesize_gcl(&dup, 10_000_000, link),
            Err(GclSynthError::DuplicateClass { cos: 7 })
        ));
    }

    // ── 802.1Qcw YANG export (REQ-TSN-EXPORT-YANG-001) ───────────────

    /// A two-window schedule on a 100 ns toy cycle: one 30 ns open window
    /// for CoS 2 (mask 0x04) then a 70 ns closed guard window (mask 0).
    fn two_window_schedule() -> GateSchedule {
        GateSchedule {
            windows: vec![
                GateWindow {
                    offset_ps: 0,
                    duration_ps: 30_000,
                    allowed_cos_mask: 0x04,
                },
                GateWindow {
                    offset_ps: 30_000,
                    duration_ps: 70_000,
                    allowed_cos_mask: 0x00,
                },
            ],
            cycle_ps: 100_000,
        }
    }

    #[test]
    fn qcw_yang_golden_snapshot() {
        // Pin the EXACT emitted document. Any formatting/structural drift
        // (a renamed node, a reordered field, a changed namespace) fails
        // here — the snapshot is the strictest oracle for a serializer.
        let expected = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<interfaces xmlns=\"urn:ietf:params:xml:ns:yang:ietf-interfaces\">
  <interface>
    <name>eth0</name>
    <gate-parameter-table xmlns=\"urn:ieee:std:802.1Q:yang:ieee802-dot1q-sched\">
      <gate-enabled>true</gate-enabled>
      <admin-gate-states>255</admin-gate-states>
      <admin-control-list>
        <gate-control-entry>
          <index>0</index>
          <operation-name>set-gate-states</operation-name>
          <time-interval-value>30</time-interval-value>
          <gate-states-value>4</gate-states-value>
        </gate-control-entry>
        <gate-control-entry>
          <index>1</index>
          <operation-name>set-gate-states</operation-name>
          <time-interval-value>70</time-interval-value>
          <gate-states-value>0</gate-states-value>
        </gate-control-entry>
      </admin-control-list>
      <admin-cycle-time>
        <numerator>100000</numerator>
        <denominator>1000000000000</denominator>
      </admin-cycle-time>
    </gate-parameter-table>
  </interface>
</interfaces>
";
        assert_eq!(two_window_schedule().to_qcw_yang("eth0"), expected);
    }

    /// Parse out every `<time-interval-value>N</time-interval-value>`.
    fn interval_values(doc: &str) -> Vec<u64> {
        doc.lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("<time-interval-value>")
                    .and_then(|r| r.strip_suffix("</time-interval-value>"))
                    .map(|n| n.parse::<u64>().unwrap())
            })
            .collect()
    }

    #[test]
    fn qcw_yang_one_entry_per_window_and_cycle_conserved() {
        let sched = GateSchedule {
            windows: vec![
                GateWindow {
                    offset_ps: 0,
                    duration_ps: 20_000,
                    allowed_cos_mask: 0x02,
                },
                GateWindow {
                    offset_ps: 20_000,
                    duration_ps: 25_000,
                    allowed_cos_mask: 0x08,
                },
                GateWindow {
                    offset_ps: 45_000,
                    duration_ps: 55_000,
                    allowed_cos_mask: 0x00,
                },
            ],
            cycle_ps: 100_000,
        };
        let doc = sched.to_qcw_yang("sw1-p3");
        // exactly one gate-control-entry (and one index) per window
        assert_eq!(doc.matches("<gate-control-entry>").count(), 3);
        assert_eq!(doc.matches("<index>").count(), 3);
        // Σ time-interval-value (ns) == cycle (ns)
        let sum_ns: u64 = interval_values(&doc).iter().sum();
        assert_eq!(sum_ns, sched.cycle_ps / 1_000);
        // cycle rational: cycle_ps / 1e12 s
        assert!(doc.contains("<numerator>100000</numerator>"));
        assert!(doc.contains("<denominator>1000000000000</denominator>"));
    }

    #[test]
    fn qcw_yang_intervals_sum_to_admin_cycle_time() {
        // Internal consistency on a REAL whole-ns synthesizer output (the
        // producer contract): Σ time-interval-value (ns) == admin-cycle-time
        // numerator / 1000. Closes the gap where the cycle is emitted
        // independently of the per-window intervals (a truncating
        // sub-nanosecond duration would break this — guarded by the
        // function's debug_assert and impossible from synthesize_gcl).
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64;
        let demands = vec![
            ClassDemand {
                cos: cos(6),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
            ClassDemand {
                cos: cos(2),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
        ];
        let sched = synthesize_gcl(&demands, cycle_ps, link).expect("feasible");
        let doc = sched.to_qcw_yang("eth2");
        let sum_ns: u64 = interval_values(&doc).iter().sum();
        assert!(doc.contains(&format!("<numerator>{}</numerator>", sched.cycle_ps)));
        assert_eq!(sum_ns, sched.cycle_ps / 1_000);
    }

    #[test]
    fn qcw_yang_mask_fidelity_including_guard() {
        let sched = GateSchedule {
            windows: vec![
                GateWindow {
                    offset_ps: 0,
                    duration_ps: 40_000,
                    allowed_cos_mask: 0x80, // CoS 7 only
                },
                GateWindow {
                    offset_ps: 40_000,
                    duration_ps: 60_000,
                    allowed_cos_mask: 0x00, // closed guard
                },
            ],
            cycle_ps: 100_000,
        };
        let doc = sched.to_qcw_yang("p");
        assert!(doc.contains("<gate-states-value>128</gate-states-value>"));
        // the closed guard window must still appear, as mask 0
        assert!(doc.contains("<gate-states-value>0</gate-states-value>"));
    }

    #[test]
    fn qcw_yang_roundtrips_synthesizer_output() {
        // Export a REAL synthesize_gcl output: the document's entry count
        // and interval sum must match the source GateSchedule, closing the
        // synthesize→export loop end to end.
        let link = 1_000_000_000u64;
        let cycle_ps = 10_000_000u64; // 10 µs
        let demands = vec![
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
            ClassDemand {
                cos: cos(3),
                arrival: ArrivalCurve::affine(250, 200_000_000),
                deadline_ps: 9_000_000,
            },
        ];
        let sched = synthesize_gcl(&demands, cycle_ps, link).expect("feasible");
        let doc = sched.to_qcw_yang("eth1");
        assert_eq!(
            doc.matches("<gate-control-entry>").count(),
            sched.windows.len()
        );
        let sum_ns: u64 = interval_values(&doc).iter().sum();
        assert_eq!(sum_ns, sched.cycle_ps / 1_000);
    }

    #[test]
    fn qcw_yang_escapes_port_name() {
        // Port names flow into XML text; a `&`/`<` must be entity-escaped
        // so the document stays well-formed.
        let doc = two_window_schedule().to_qcw_yang("p&<a>\"");
        assert!(doc.contains("<name>p&amp;&lt;a&gt;&quot;</name>"));
        assert!(!doc.contains("<name>p&<a>"));
    }
}
