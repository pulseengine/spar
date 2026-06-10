//! Network Calculus primitives: arrival curves, service curves, and the
//! min-plus operations that combine them.
//!
//! This is the math kernel that the `wctt.rs` analysis pass (Track D
//! commit 4) will compose. The primitives here are AADL-agnostic — they
//! are pure Network Calculus on the affine "leaky bucket" arrival model
//! and the rate-latency service model. Mapping AADL property values into
//! these types is the responsibility of downstream code.
//!
//! # Scope (v0.8.0 Track D commit 3)
//!
//! Curves are restricted to the closed-form cases of Le Boudec & Thiran,
//! "Network Calculus" (chapter 1):
//!
//! - **Arrival curve** (leaky bucket): α(t) = σ + ρ·t, optionally
//!   capped by a peak rate p so α(t) = min(σ + ρ·t, p·t). σ is a burst
//!   in bytes, ρ a sustained rate in bits per second, p an optional
//!   peak rate in bits per second.
//! - **Service curve** (rate-latency): β(t) = R · max(0, t − T) where
//!   R is the rate in bits per second and T is the latency in
//!   picoseconds.
//!
//! The four operators (`backlog_bound`, `delay_bound`, `residual_service`,
//! `output_bound`) implement the closed-form bounds for this case.
//!
//! # Piecewise-affine extension (v0.9.3)
//!
//! The single-bucket affine form pays the full burst on the peak rate
//! over arbitrarily long windows. Real ADAS / TSN traffic is often
//! described by T-SPEC-style multi-bucket constraints:
//!
//! ```text
//! α(t) = min_i (σ_i + ρ_i · t)
//! ```
//!
//! where each `(σ_i, ρ_i)` is a leaky bucket and the overall curve is
//! the minimum of the family — capturing both short-horizon burst (small
//! σ, high ρ) and long-horizon sustained behaviour (large σ, low ρ).
//! This form typically *halves* delay/backlog bounds on real traffic
//! because the small-burst bucket binds the short-window readouts that
//! the worst-case delay computation actually exercises.
//!
//! [`piecewise::PiecewiseAffineArrivalCurve`] implements this
//! generalisation alongside the single-bucket [`ArrivalCurve`]. The
//! `wctt.rs` consumers continue to use the single-bucket form for
//! v0.9.3 — switching the WCTT pass to piecewise (with a propagation
//! strategy that keeps the min-bucket structure across hops) is a
//! follow-up commit. The Lean theorems in
//! `proofs/Proofs/Network/MinPlus.lean` still target the single-bucket
//! form; piecewise theorem statements are skeletoned in
//! `proofs/Proofs/Network/MinPlusPwa.lean` for a future v1.0.0 sweep.
//!
//! # Units
//!
//! All time values are `u64` picoseconds, all data sizes are `u64`
//! bytes, and all rates are `u64` bits per second. Internally we widen
//! to `u128` for the rate × time products to avoid overflow on
//! realistic inputs (e.g. 100 Gbps × 1 ms ≈ 10⁸ bytes).
//!
//! Conversion identities used throughout:
//!
//! ```text
//! bytes_in_window(rate_bps, t_ps) = rate_bps * t_ps / (8 * 10^12)
//! time_to_send(bytes, rate_bps)   = bytes * 8 * 10^12 / rate_bps
//! ```
//!
//! Both use truncating integer division. Pessimism direction: arrival
//! curve readouts round *down* and time-to-send computations round *up*
//! as noted on the individual functions, so the WCTT pass that consumes
//! these operators stays on the safe side of the bound.

/// Picoseconds per second. Defined out as a constant so the unit
/// conversion is auditable in one place.
pub(crate) const PS_PER_SECOND: u64 = 1_000_000_000_000;

/// Bits per byte.
pub(crate) const BITS_PER_BYTE: u64 = 8;

/// Compute `rate_bps * t_ps / (8 * 10^12)` using u128 to avoid overflow,
/// saturating to `u64::MAX` if the result still exceeds u64.
///
/// This is the number of bytes that can flow at `rate_bps` during a
/// window of `t_ps` picoseconds. Truncates downward — the natural
/// rounding for arrival-curve readouts (gives a non-loose count of
/// bytes that *must* have arrived) and for service-curve readouts
/// (gives a lower bound on bytes drained, which is what β represents).
fn bits_to_bytes_in_window(rate_bps: u64, t_ps: u64) -> u64 {
    let product = (rate_bps as u128) * (t_ps as u128);
    let denom = (BITS_PER_BYTE as u128) * (PS_PER_SECOND as u128);
    let bytes = product / denom;
    if bytes > u64::MAX as u128 {
        u64::MAX
    } else {
        bytes as u64
    }
}

/// Compute the time in picoseconds needed to transmit `bytes` at
/// `rate_bps` (`bytes * 8 * 10^12 / rate_bps`), rounding *up* so that
/// the returned duration is never an under-estimate.
///
/// Returns `u64::MAX` on saturation (e.g. when `rate_bps == 0` we treat
/// the time as effectively unbounded; callers should screen for that
/// case before invoking this helper).
fn time_to_send_ps(bytes: u64, rate_bps: u64) -> u64 {
    if rate_bps == 0 {
        return u64::MAX;
    }
    let numer = (bytes as u128) * (BITS_PER_BYTE as u128) * (PS_PER_SECOND as u128);
    let denom = rate_bps as u128;
    // Round up so the duration is never an under-estimate.
    let ps = numer.div_ceil(denom);
    if ps > u64::MAX as u128 {
        u64::MAX
    } else {
        ps as u64
    }
}

// ── Arrival curve ────────────────────────────────────────────────────

/// An arrival curve α(t): the maximum number of bytes that can arrive
/// in any window of length `t`.
///
/// Restricted to the affine "leaky bucket" form
///
/// ```text
/// α(t) = min(burst_bytes + sustained_rate · t, peak_rate · t)
/// ```
///
/// when `peak_rate_bps` is `Some`, or simply
///
/// ```text
/// α(t) = burst_bytes + sustained_rate · t
/// ```
///
/// when it is `None`.
///
/// **Causality at t = 0**: α(0) = 0 by convention, regardless of the
/// burst σ. A window of length zero contains zero bytes — the burst is
/// only realized as soon as t > 0 (instantaneously). For the
/// peak-capped form this falls out of `min(σ + 0, p · 0) = min(σ, 0) =
/// 0`; for the affine-only form we special-case it. Aligned with the
/// Lean spec in `proofs/Proofs/Network/MinPlus.lean` (v0.9.2 fix).
///
/// Note: the Network Calculus *bounds* (`backlog_bound`, `delay_bound`,
/// `output_bound`, `residual_service`) all use the closed-form
/// expressions in σ and ρ directly — they do **not** call `at(0)` —
/// so this convention only affects readouts of α at t = 0 and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrivalCurve {
    /// σ — the maximum instantaneous burst in bytes (the y-intercept).
    pub burst_bytes: u64,
    /// ρ — the long-run sustained rate in bits per second.
    pub sustained_rate_bps: u64,
    /// p — an optional ceiling on the short-run rate, in bits per
    /// second. When `Some`, α(t) is capped at `p · t`.
    pub peak_rate_bps: Option<u64>,
}

impl ArrivalCurve {
    /// Construct an affine arrival curve `σ + ρ·t` with no peak cap.
    pub const fn affine(burst_bytes: u64, sustained_rate_bps: u64) -> Self {
        Self {
            burst_bytes,
            sustained_rate_bps,
            peak_rate_bps: None,
        }
    }

    /// Construct an arrival curve with a peak rate cap, giving
    /// α(t) = min(σ + ρ·t, p·t).
    pub const fn with_peak(burst_bytes: u64, sustained_rate_bps: u64, peak_rate_bps: u64) -> Self {
        Self {
            burst_bytes,
            sustained_rate_bps,
            peak_rate_bps: Some(peak_rate_bps),
        }
    }

    /// Compute α(t) at a given time in picoseconds.
    ///
    /// Returns the number of bytes. Saturates to `u64::MAX` if the
    /// underlying arithmetic would overflow (which only happens for
    /// astronomical rate × time products).
    ///
    /// **Causality**: α(0) = 0 — a zero-length window admits no bytes
    /// even when σ > 0. The peak-capped branch enforces this naturally
    /// via `min(σ + 0, p · 0) = 0`; the affine-only branch needs the
    /// explicit `t_ps == 0` short-circuit since `σ + ρ · 0 = σ`. This
    /// matches the Lean spec in `proofs/Proofs/Network/MinPlus.lean`
    /// (v0.9.2: aligned away from the prior pre-mature-optimisation
    /// short-circuit that returned σ at t = 0).
    pub fn at(&self, t_ps: u64) -> u64 {
        if t_ps == 0 {
            // Causal: zero-length window admits zero bytes. The peak
            // branch would give 0 by `min(σ, 0)` anyway; the affine-only
            // branch needs this explicit case because `σ + ρ·0 = σ`.
            return 0;
        }
        let sustained = self
            .burst_bytes
            .saturating_add(bits_to_bytes_in_window(self.sustained_rate_bps, t_ps));
        match self.peak_rate_bps {
            Some(peak) => {
                let peak_term = bits_to_bytes_in_window(peak, t_ps);
                sustained.min(peak_term)
            }
            None => sustained,
        }
    }
}

// ── Service curve ────────────────────────────────────────────────────

/// A service curve β(t): the minimum number of bytes a server is
/// guaranteed to drain in any backlogged window of length `t`.
///
/// Rate-latency form:
///
/// ```text
/// β(t) = rate · max(0, t − latency)
/// ```
///
/// `rate_bps` is in bits per second, `latency_ps` in picoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCurve {
    /// R — the long-run service rate in bits per second.
    pub rate_bps: u64,
    /// T — the latency before service begins, in picoseconds.
    pub latency_ps: u64,
}

impl ServiceCurve {
    /// Construct a rate-latency service curve.
    pub const fn rate_latency(rate_bps: u64, latency_ps: u64) -> Self {
        Self {
            rate_bps,
            latency_ps,
        }
    }

    /// Compute β(t) at a given time in picoseconds.
    ///
    /// Returns the minimum bytes guaranteed served in `t`. Below the
    /// latency, β returns 0 (the server has not started serving yet).
    pub fn at(&self, t_ps: u64) -> u64 {
        if t_ps <= self.latency_ps {
            return 0;
        }
        let delta = t_ps - self.latency_ps;
        bits_to_bytes_in_window(self.rate_bps, delta)
    }
}

// ── Errors ───────────────────────────────────────────────────────────

/// Errors returned by Network Calculus operators when the closed-form
/// is not defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcError {
    /// The arrival rate exceeds the service rate, so backlog and delay
    /// are unbounded (`ρ > R`). No finite Network Calculus bound exists.
    UnstableServer,
    /// The competing flow's sustained rate equals or exceeds the
    /// server's rate, so no service is left for the tagged flow.
    UnservableFlow,
}

impl core::fmt::Display for NcError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnstableServer => {
                write!(f, "arrival rate exceeds service rate; bound is unbounded")
            }
            Self::UnservableFlow => write!(
                f,
                "competing flow consumes all service; tagged flow is unservable"
            ),
        }
    }
}

impl core::error::Error for NcError {}

// ── Bounds and composition ───────────────────────────────────────────

/// Maximum backlog (bytes) at a server with arrival α and service β,
/// computed as the closed-form `sup_t { α(t) − β(t) }`.
///
/// For the affine + rate-latency case, the supremum is reached at
/// `t = latency` (the moment the server starts draining), giving
///
/// ```text
/// B = σ + ρ · latency
/// ```
///
/// when `ρ ≤ R`. When `ρ > R` the queue grows without bound and we
/// return [`NcError::UnstableServer`].
pub fn backlog_bound(alpha: &ArrivalCurve, beta: &ServiceCurve) -> Result<u64, NcError> {
    if alpha.sustained_rate_bps > beta.rate_bps {
        return Err(NcError::UnstableServer);
    }
    let inflation = bits_to_bytes_in_window(alpha.sustained_rate_bps, beta.latency_ps);
    Ok(alpha.burst_bytes.saturating_add(inflation))
}

/// Maximum delay (picoseconds) experienced by a flow with arrival α at
/// a server with service β, computed as the horizontal distance
/// between α and β.
///
/// For the affine + rate-latency case:
///
/// ```text
/// D = latency + σ / R
/// ```
///
/// where `σ / R` is converted to picoseconds as `σ · 8 · 10^12 / R`.
/// Returns [`NcError::UnstableServer`] when `ρ > R`.
///
/// The `σ / R` term is rounded *up* (via [`time_to_send_ps`]) so the
/// returned bound is always a valid upper bound, never an
/// underestimate.
pub fn delay_bound(alpha: &ArrivalCurve, beta: &ServiceCurve) -> Result<u64, NcError> {
    if alpha.sustained_rate_bps > beta.rate_bps {
        return Err(NcError::UnstableServer);
    }
    if beta.rate_bps == 0 {
        return Err(NcError::UnstableServer);
    }
    let burst_drain_ps = time_to_send_ps(alpha.burst_bytes, beta.rate_bps);
    Ok(beta.latency_ps.saturating_add(burst_drain_ps))
}

/// Residual service curve seen by a tagged flow when a competing flow
/// shares the same FIFO server.
///
/// For rate-latency β and affine competing α_c:
///
/// ```text
/// β'(t) = max(0, (R − ρ_c) · (t − T − σ_c / (R − ρ_c)))
/// ```
///
/// i.e. the residual rate is `R − ρ_c` and the residual latency is
/// `T + σ_c / (R − ρ_c)`. Returns [`NcError::UnservableFlow`] when
/// `ρ_c ≥ R`.
///
/// The `σ_c / (R − ρ_c)` term is rounded up so the residual latency is
/// never under-estimated (pessimism direction matches `delay_bound`).
pub fn residual_service(
    beta: &ServiceCurve,
    alpha_competing: &ArrivalCurve,
) -> Result<ServiceCurve, NcError> {
    if alpha_competing.sustained_rate_bps >= beta.rate_bps {
        return Err(NcError::UnservableFlow);
    }
    let residual_rate = beta.rate_bps - alpha_competing.sustained_rate_bps;
    let extra_latency = time_to_send_ps(alpha_competing.burst_bytes, residual_rate);
    Ok(ServiceCurve {
        rate_bps: residual_rate,
        latency_ps: beta.latency_ps.saturating_add(extra_latency),
    })
}

/// Output (departure) arrival curve of a flow with arrival α through a
/// server with rate-latency service β.
///
/// Standard Network Calculus output bound theorem (Le Boudec & Thiran,
/// theorem 1.4.3): the rate is preserved, the burst grows by `ρ · T`:
///
/// ```text
/// α*(t) = (σ + ρ · T) + ρ · t
/// ```
///
/// The peak rate (if any) is preserved unchanged — service-induced
/// burstiness does not affect the short-term ceiling. Returns
/// [`NcError::UnstableServer`] if the flow would not be served
/// (`ρ > R`).
pub fn output_bound(alpha: &ArrivalCurve, beta: &ServiceCurve) -> Result<ArrivalCurve, NcError> {
    if alpha.sustained_rate_bps > beta.rate_bps {
        return Err(NcError::UnstableServer);
    }
    let inflation = bits_to_bytes_in_window(alpha.sustained_rate_bps, beta.latency_ps);
    Ok(ArrivalCurve {
        burst_bytes: alpha.burst_bytes.saturating_add(inflation),
        sustained_rate_bps: alpha.sustained_rate_bps,
        peak_rate_bps: alpha.peak_rate_bps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1 Gbps in bits/second.
    const GBPS: u64 = 1_000_000_000;
    /// 100 Mbps in bits/second.
    const HUNDRED_MBPS: u64 = 100_000_000;
    /// 400 Mbps in bits/second.
    const FOUR_HUNDRED_MBPS: u64 = 400_000_000;
    /// 600 Mbps in bits/second.
    const SIX_HUNDRED_MBPS: u64 = 600_000_000;
    /// 10 microseconds in picoseconds.
    const TEN_US_PS: u64 = 10_000_000;
    /// 1 microsecond in picoseconds.
    const ONE_US_PS: u64 = 1_000_000;

    #[test]
    fn arrival_curve_at_zero_is_zero() {
        // Causality: α(0) = 0 for all arrival curves — a zero-length
        // window admits zero bytes even when σ > 0. v0.9.2 alignment
        // with the Lean spec; the prior short-circuit that returned σ
        // was a pre-mature optimisation that violated causality.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        assert_eq!(alpha.at(0), 0);

        let alpha = ArrivalCurve::with_peak(1500, HUNDRED_MBPS, GBPS);
        assert_eq!(alpha.at(0), 0);

        // The burst is still the y-intercept of the affine line and is
        // realised as t grows beyond zero — this is captured by other
        // tests (`affine_arrival_eval`, `affine_arrival_with_peak`).
    }

    #[test]
    fn service_curve_at_latency_is_zero() {
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        // Below the latency: β = 0.
        assert_eq!(beta.at(0), 0);
        assert_eq!(beta.at(1), 0);
        assert_eq!(beta.at(TEN_US_PS / 2), 0);
        // At exactly the latency: β = 0 (served = 0 bytes so far).
        assert_eq!(beta.at(TEN_US_PS), 0);
    }

    #[test]
    fn service_curve_above_latency_grows_linearly() {
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        // At latency + 1 us: served = 1 Gbps · 1 us
        //                  = 10^9 · 10^-6 bits = 1000 bits = 125 bytes.
        let bytes_in_one_us_at_gbps = bits_to_bytes_in_window(GBPS, ONE_US_PS);
        assert_eq!(bytes_in_one_us_at_gbps, 125);
        assert_eq!(beta.at(TEN_US_PS + ONE_US_PS), 125);
        // At latency + 8 us: 8 × 125 = 1000 bytes.
        assert_eq!(beta.at(TEN_US_PS + 8 * ONE_US_PS), 1000);
        // Larger ms-scale window: 1 Gbps · 1 ms = 10^6 bits = 125_000 bytes.
        let one_ms_ps: u64 = 1_000_000_000;
        assert_eq!(beta.at(TEN_US_PS + one_ms_ps), 125_000);
    }

    #[test]
    fn affine_arrival_eval() {
        // σ = 1500, ρ = 100 Mbps. At t = 1 ms (10^9 ps):
        // bytes = 1500 + 100_000_000 · 10^9 / (8 · 10^12)
        //       = 1500 + 12_500
        //       = 14_000.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        assert_eq!(alpha.at(1_000_000_000), 14_000);
    }

    #[test]
    fn affine_arrival_with_peak() {
        // For very small t the peak rate dominates. With σ=1500,
        // ρ=100Mbps, peak=1Gbps, the crossover (where σ + ρ·t equals
        // peak·t) is at t = σ·8·10^12 / (peak − ρ) ≈ 13_333 ps.
        let alpha = ArrivalCurve::with_peak(1500, HUNDRED_MBPS, GBPS);

        // At t = 10_000 ps (10 ns), still under the 13.3 ns crossover:
        //   sustained_term = 1500 + 100Mbps · 10000 / (8·10^12)
        //                  = 1500 + 0 (truncated)         = 1500
        //   peak_term      = 1Gbps · 10000 / (8·10^12)    = 1 byte
        // So α saturates at 1 byte. That demonstrates the peak cap is
        // active; the cap dominates for small t.
        let small = alpha.at(10_000);
        let peak_only = bits_to_bytes_in_window(GBPS, 10_000);
        assert_eq!(small, peak_only);
        assert!(small < 1500); // Capped well below the burst.

        // At a large t, sustained dominates.
        let large = alpha.at(1_000_000_000); // 1 ms
        // sustained_term = 1500 + 12_500 = 14_000
        // peak_term      = 1Gbps · 10^9 / (8·10^12) = 125_000
        assert_eq!(large, 14_000);
    }

    #[test]
    fn backlog_bound_classical() {
        // σ = 1500, ρ = 100 Mbps, β: rate = 1 Gbps, latency = 10 us.
        // Closed form: σ + ρ · latency = 1500 + 100Mbps·10us/(8·10^12)
        //                              = 1500 + 125 = 1625 bytes.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        assert_eq!(backlog_bound(&alpha, &beta).unwrap(), 1625);
    }

    #[test]
    fn backlog_bound_unstable_when_rate_exceeds_service() {
        let alpha = ArrivalCurve::affine(1500, GBPS); // 1 Gbps
        let beta = ServiceCurve::rate_latency(HUNDRED_MBPS, TEN_US_PS); // 100 Mbps
        assert_eq!(backlog_bound(&alpha, &beta), Err(NcError::UnstableServer));
    }

    #[test]
    fn delay_bound_classical() {
        // σ = 1500, ρ = 100 Mbps, β: rate = 1 Gbps, latency = 10 us.
        // Closed form: latency + σ/R
        //   σ/R = 1500 · 8 / 1Gbps seconds = 12_000 / 10^9 s = 12 us
        //       = 12_000_000 ps.
        // → D = 10 us + 12 us = 22 us = 22_000_000 ps.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        assert_eq!(delay_bound(&alpha, &beta).unwrap(), TEN_US_PS + 12_000_000);
    }

    #[test]
    fn delay_bound_unstable_when_rate_exceeds_service() {
        let alpha = ArrivalCurve::affine(1500, GBPS);
        let beta = ServiceCurve::rate_latency(HUNDRED_MBPS, TEN_US_PS);
        assert_eq!(delay_bound(&alpha, &beta), Err(NcError::UnstableServer));
    }

    #[test]
    fn residual_service_two_flows() {
        // 1 Gbps server, two flows each at 400 Mbps sustained.
        // Each flow's residual service rate should be 1Gbps − 400Mbps
        // = 600 Mbps.
        let beta = ServiceCurve::rate_latency(GBPS, 0);
        let competing = ArrivalCurve::affine(1500, FOUR_HUNDRED_MBPS);

        let residual = residual_service(&beta, &competing).unwrap();
        assert_eq!(residual.rate_bps, SIX_HUNDRED_MBPS);

        // Symmetric: from the other flow's perspective, same residual.
        let residual_b = residual_service(&beta, &competing).unwrap();
        assert_eq!(residual_b.rate_bps, SIX_HUNDRED_MBPS);

        // Latency inflation: σ_c / (R − ρ_c) = 1500·8·10^12 / 600Mbps
        //                                   = 12_000·10^12 / 6·10^8
        //                                   = 20_000_000 ps = 20 us.
        // Original β had latency = 0, so residual latency = 20 us.
        assert_eq!(residual.latency_ps, 20_000_000);
    }

    #[test]
    fn residual_service_unservable() {
        // Competing flow at 1 Gbps on a 1 Gbps server: nothing left.
        let beta = ServiceCurve::rate_latency(GBPS, 0);
        let competing = ArrivalCurve::affine(0, GBPS);
        assert_eq!(
            residual_service(&beta, &competing),
            Err(NcError::UnservableFlow)
        );

        // Competing flow at 1.1 Gbps: also unservable.
        let over = ArrivalCurve::affine(0, GBPS + HUNDRED_MBPS);
        assert_eq!(residual_service(&beta, &over), Err(NcError::UnservableFlow));
    }

    #[test]
    fn output_bound_burst_inflation() {
        // σ = 1500, ρ = 100 Mbps, β: rate = 1 Gbps, latency = 10 us.
        // Output burst = σ + ρ · latency = 1500 + 125 = 1625 bytes.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        let out = output_bound(&alpha, &beta).unwrap();
        assert_eq!(out.burst_bytes, 1625);
    }

    #[test]
    fn output_bound_rate_preserved() {
        // The sustained rate must pass through unchanged.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        let out = output_bound(&alpha, &beta).unwrap();
        assert_eq!(out.sustained_rate_bps, HUNDRED_MBPS);

        // And the peak rate, when present.
        let alpha_peak = ArrivalCurve::with_peak(1500, HUNDRED_MBPS, GBPS);
        let out_peak = output_bound(&alpha_peak, &beta).unwrap();
        assert_eq!(out_peak.peak_rate_bps, Some(GBPS));
        assert_eq!(out_peak.sustained_rate_bps, HUNDRED_MBPS);
    }

    #[test]
    fn compose_two_servers_classical() {
        // Two rate-latency servers in series for one flow.
        // β_a: 1 Gbps, latency 10 us; β_b: 1 Gbps, latency 5 us.
        // α: σ=1500, ρ=100 Mbps.
        //
        // Naive (per-hop) end-to-end delay:
        //   D_a = T_a + σ/R = 10us + 12us = 22us
        //   σ_a (output of first) = σ + ρ·T_a = 1500 + 125 = 1625
        //   D_b = T_b + σ_a/R = 5us + 1625·8·10^12/1Gbps
        //       = 5us + 13us = 18us
        //   Total = 40us.
        //
        // Classical "pay burst only once" sum (which our naive
        // composition approximates without the PBOO concatenation):
        //   T_a + T_b + σ / min(R_a, R_b) = 10us + 5us + 12us = 27us.
        //
        // For this commit we don't yet implement PBOO concatenation,
        // so we verify the naive composition: feed the output of the
        // first server's output_bound into the second server's
        // delay_bound and accumulate.
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        let beta_a = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
        let beta_b = ServiceCurve::rate_latency(GBPS, 5 * ONE_US_PS);

        let d_a = delay_bound(&alpha, &beta_a).unwrap();
        let alpha_out = output_bound(&alpha, &beta_a).unwrap();
        let d_b = delay_bound(&alpha_out, &beta_b).unwrap();

        // d_a = 10us + 12us = 22us.
        assert_eq!(d_a, 22_000_000);
        // alpha_out: burst = 1500 + 125 = 1625 bytes.
        assert_eq!(alpha_out.burst_bytes, 1625);
        // d_b = 5us + (1625·8·10^12/1Gbps rounded up) = 5us + 13us
        //     = 18_000_000 ps. (1625·8 = 13_000 ns = 13 us exactly.)
        assert_eq!(d_b, 5_000_000 + 13_000_000);

        // End-to-end naive sum = 22us + 18us = 40us.
        assert_eq!(d_a + d_b, 40_000_000);
    }
}

// ── Piecewise-affine arrival curves (v0.9.3) ─────────────────────────

/// Piecewise-affine arrival curves: `α(t) = min_i (σ_i + ρ_i · t)`.
///
/// The math kernel for the v0.9.3 NC tightness item #1: a generalisation
/// of the single-bucket [`ArrivalCurve`] that captures T-SPEC-style
/// multi-bucket constraints. The single-bucket form is the special case
/// `[(σ, ρ)]` and round-trips through `From<ArrivalCurve>`.
///
/// The wctt.rs analysis pass continues to consume single-bucket
/// [`ArrivalCurve`] in v0.9.3; switching the pass to piecewise is a
/// follow-up commit (it needs a min-bucket-preserving propagation
/// strategy that respects the AADL property surface for declaring
/// per-stream T-SPEC inputs).
pub mod piecewise {
    use super::{ArrivalCurve, NcError, ServiceCurve, bits_to_bytes_in_window, time_to_send_ps};

    /// Piecewise-affine arrival curve: `α(t) = min_i (σ_i + ρ_i · t)`.
    ///
    /// Each `(σ_i, ρ_i)` is a leaky bucket; the overall curve is the
    /// pointwise minimum of the family. The min-of-affines structure
    /// captures both short-horizon burst (small σ, high ρ) and
    /// long-horizon sustained behaviour (large σ, low ρ) — the workhorse
    /// model of T-SPEC and IntServ-style traffic descriptors.
    ///
    /// `ArrivalCurve::affine(σ, ρ)` is the special case `[(σ, ρ)]`;
    /// `From<ArrivalCurve>` round-trips through it. Peak-rate caps from
    /// the single-bucket form are encoded as a second bucket
    /// `(0, peak)` so all callers see a single representation here.
    ///
    /// # Canonicalisation
    ///
    /// On construction the bucket list is sorted by σ (ascending) and
    /// duplicate (σ, ρ) pairs are removed. Equality (`PartialEq`,
    /// `Eq`) compares the canonical form so two curves that describe
    /// the same min-of-affines are equal regardless of input order.
    /// The constructor rejects an empty bucket list — at least one
    /// bucket is required for the min to be defined.
    ///
    /// # Causality at t = 0
    ///
    /// `at(0) = 0` for all piecewise curves, matching the single-bucket
    /// convention pinned by the Lean spec in `MinPlus.lean`. A
    /// zero-length window admits zero bytes regardless of the bucket
    /// bursts.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PiecewiseAffineArrivalCurve {
        /// Buckets `(σ_i bytes, ρ_i bps)` in canonical form: sorted by
        /// σ ascending (ρ used as secondary key for stability), with
        /// duplicate pairs removed. Always non-empty.
        buckets: Vec<(u64, u64)>,
    }

    /// Errors returned when constructing a [`PiecewiseAffineArrivalCurve`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PwaError {
        /// The bucket list was empty. Piecewise α requires at least
        /// one bucket — the min over an empty family is undefined and
        /// would mean "infinite arrivals", which is not the intent.
        EmptyBuckets,
    }

    impl core::fmt::Display for PwaError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::EmptyBuckets => write!(
                    f,
                    "piecewise-affine arrival curve requires at least one bucket"
                ),
            }
        }
    }

    impl core::error::Error for PwaError {}

    impl PiecewiseAffineArrivalCurve {
        /// Construct a piecewise-affine arrival curve from a bucket
        /// list. The list is canonicalised in place: sorted by σ
        /// ascending (ρ ties broken by ρ ascending) and de-duplicated.
        ///
        /// Returns [`PwaError::EmptyBuckets`] when `buckets` is empty.
        ///
        /// Note: this constructor does **not** prune buckets that are
        /// dominated by others (e.g. a `(σ_i, ρ_i)` strictly above the
        /// min envelope of the others). Dominated buckets are sound
        /// but redundant and can be stripped for compactness; we keep
        /// the simple-and-correct form here and leave domination
        /// pruning to a future helper.
        pub fn new(mut buckets: Vec<(u64, u64)>) -> Result<Self, PwaError> {
            if buckets.is_empty() {
                return Err(PwaError::EmptyBuckets);
            }
            // Canonicalise: sort by (σ, ρ) ascending, dedup exact pairs.
            buckets.sort_unstable();
            buckets.dedup();
            Ok(Self { buckets })
        }

        /// Borrow the canonical bucket list.
        pub fn buckets(&self) -> &[(u64, u64)] {
            &self.buckets
        }

        /// Compute `α(t) = min_i (σ_i + ρ_i · t)` at the given time
        /// `t_ps` in picoseconds.
        ///
        /// Returns the minimum over per-bucket readouts. Saturates to
        /// `u64::MAX` only if every bucket saturates, which is
        /// astronomical input territory.
        ///
        /// **Causality**: `at(0) = 0` for all piecewise curves —
        /// matches the single-bucket convention; a zero-length window
        /// admits zero bytes regardless of σ.
        pub fn at(&self, t_ps: u64) -> u64 {
            if t_ps == 0 {
                return 0;
            }
            self.buckets
                .iter()
                .map(|&(sigma, rho)| sigma.saturating_add(bits_to_bytes_in_window(rho, t_ps)))
                .min()
                .expect("buckets is non-empty by construction")
        }

        /// Asymptotic long-run sustained rate of the curve.
        ///
        /// As `t → ∞` the smallest `ρ_i` dominates. This is the
        /// long-run rate seen by stability checks and by the bound
        /// operators when they compose with a service curve.
        pub fn sustained_rate_bps(&self) -> u64 {
            self.buckets
                .iter()
                .map(|&(_, rho)| rho)
                .min()
                .expect("buckets is non-empty by construction")
        }

        /// Largest sustained rate across all buckets — the conservative
        /// rate used by [`residual_service`] when a single-bucket
        /// rate-latency residual must absorb a piecewise competitor.
        pub fn max_sustained_rate_bps(&self) -> u64 {
            self.buckets
                .iter()
                .map(|&(_, rho)| rho)
                .max()
                .expect("buckets is non-empty by construction")
        }

        /// Largest burst across all buckets — the conservative σ used
        /// by [`residual_service`] when a single-bucket rate-latency
        /// residual must absorb a piecewise competitor.
        pub fn max_burst_bytes(&self) -> u64 {
            self.buckets
                .iter()
                .map(|&(sigma, _)| sigma)
                .max()
                .expect("buckets is non-empty by construction")
        }
    }

    /// `ArrivalCurve::affine(σ, ρ)` round-trips into the single-bucket
    /// piecewise form `[(σ, ρ)]`. A peak-rate cap (if present) is
    /// encoded as a second bucket `(0, peak)`: `α(t) = min(σ + ρ·t,
    /// peak·t)` is the canonical 2-bucket form of the peak-capped
    /// affine curve.
    impl From<ArrivalCurve> for PiecewiseAffineArrivalCurve {
        fn from(alpha: ArrivalCurve) -> Self {
            let mut buckets = vec![(alpha.burst_bytes, alpha.sustained_rate_bps)];
            if let Some(peak) = alpha.peak_rate_bps {
                // The peak cap `min(σ + ρ·t, p·t)` is `p·t` at large t
                // when ρ < p, so the second bucket has σ = 0 burst and
                // rate p. For p ≤ ρ the peak cap is degenerate (the
                // sustained line dominates) and we'd dedup naturally.
                buckets.push((0, peak));
            }
            // Constructor canonicalises and never fails on a
            // non-empty list.
            Self::new(buckets).expect("buckets is non-empty")
        }
    }

    /// Maximum backlog (bytes) at a server with piecewise arrival α
    /// and rate-latency service β.
    ///
    /// Each bucket `(σ_i, ρ_i)` independently dominates α (since
    /// α = min, α(t) ≤ σ_i + ρ_i·t for every i), so the per-bucket
    /// backlog `σ_i + ρ_i·T` is a valid bound. The tight composite
    /// bound is the **minimum** over per-bucket bounds — the bucket
    /// that gives the smallest backlog is the one that actually binds
    /// at the supremum.
    ///
    /// Stability: every bucket must satisfy `ρ_i ≤ R`. With strict
    /// piecewise α the asymptotic rate is `min ρ_i`, so as long as
    /// one bucket is stable the system is stable; but we require
    /// **all** buckets stable here so every per-bucket bound is
    /// well-defined and the min is taken over a uniformly valid family.
    /// This matches the conservative single-bucket convention.
    /// Returns [`NcError::UnstableServer`] if any bucket has `ρ_i > R`.
    pub fn backlog_bound(
        alpha: &PiecewiseAffineArrivalCurve,
        beta: &ServiceCurve,
    ) -> Result<u64, NcError> {
        for &(_, rho) in &alpha.buckets {
            if rho > beta.rate_bps {
                return Err(NcError::UnstableServer);
            }
        }
        let mut best: Option<u64> = None;
        for &(sigma, rho) in &alpha.buckets {
            let inflation = bits_to_bytes_in_window(rho, beta.latency_ps);
            let candidate = sigma.saturating_add(inflation);
            best = Some(match best {
                Some(b) => b.min(candidate),
                None => candidate,
            });
        }
        Ok(best.expect("buckets is non-empty by construction"))
    }

    /// Maximum delay (picoseconds) experienced by a flow with
    /// piecewise arrival α at a server with rate-latency service β.
    ///
    /// Each bucket gives a per-bucket delay bound `T + σ_i / R`; the
    /// tight composite bound is the **minimum** across buckets — the
    /// horizontal distance `h(α, β)` for `α = min_i α_i` is at most
    /// `min_i h(α_i, β)` because `D` is monotone non-decreasing in α.
    /// At any operating point one bucket is binding (the one whose
    /// `σ + ρ·t` is smallest there); its `T + σ/R` is the delay.
    ///
    /// The `σ_i / R` term is rounded up via [`time_to_send_ps`] so
    /// every per-bucket bound is a valid upper bound, and so is the
    /// minimum.
    ///
    /// Stability: requires every `ρ_i ≤ R` so every per-bucket bound
    /// is well-defined. Returns [`NcError::UnstableServer`] otherwise.
    pub fn delay_bound(
        alpha: &PiecewiseAffineArrivalCurve,
        beta: &ServiceCurve,
    ) -> Result<u64, NcError> {
        if beta.rate_bps == 0 {
            return Err(NcError::UnstableServer);
        }
        for &(_, rho) in &alpha.buckets {
            if rho > beta.rate_bps {
                return Err(NcError::UnstableServer);
            }
        }
        let mut best: Option<u64> = None;
        for &(sigma, _) in &alpha.buckets {
            let burst_drain_ps = time_to_send_ps(sigma, beta.rate_bps);
            let candidate = beta.latency_ps.saturating_add(burst_drain_ps);
            best = Some(match best {
                Some(b) => b.min(candidate),
                None => candidate,
            });
        }
        Ok(best.expect("buckets is non-empty by construction"))
    }

    /// Output (departure) arrival curve of a flow with piecewise
    /// arrival α through a rate-latency server β.
    ///
    /// Each bucket `(σ_i, ρ_i)` of α produces an output bucket
    /// `(σ_i + ρ_i · T, ρ_i)` — rate is preserved, burst grows by
    /// `ρ_i · T` (Le Boudec & Thiran theorem 1.4.3 applied
    /// per-bucket). Since α(t) ≤ σ_i + ρ_i·t for every i, the output
    /// satisfies `α*(t) ≤ (σ_i + ρ_i·T) + ρ_i·t` for every i, so the
    /// output is again the **minimum** of the inflated per-bucket
    /// affine curves: a piecewise-affine output curve with the same
    /// number of buckets, each independently inflated.
    ///
    /// Stability: every `ρ_i ≤ R` is required. Returns
    /// [`NcError::UnstableServer`] otherwise.
    pub fn output_bound(
        alpha: &PiecewiseAffineArrivalCurve,
        beta: &ServiceCurve,
    ) -> Result<PiecewiseAffineArrivalCurve, NcError> {
        for &(_, rho) in &alpha.buckets {
            if rho > beta.rate_bps {
                return Err(NcError::UnstableServer);
            }
        }
        let new_buckets: Vec<(u64, u64)> = alpha
            .buckets
            .iter()
            .map(|&(sigma, rho)| {
                let inflation = bits_to_bytes_in_window(rho, beta.latency_ps);
                (sigma.saturating_add(inflation), rho)
            })
            .collect();
        // Re-canonicalise: inflation can change σ ordering relative to
        // the input (a small-σ-high-ρ bucket may inflate past a
        // large-σ-low-ρ bucket). Constructor only fails on an empty
        // list, which is impossible here because `alpha.buckets` is
        // non-empty by construction.
        Ok(PiecewiseAffineArrivalCurve::new(new_buckets)
            .expect("buckets is non-empty by construction"))
    }

    /// Residual service curve seen by a tagged flow when a piecewise
    /// competing flow shares the same FIFO server.
    ///
    /// **Conservative single-bucket residual.** Each competing bucket
    /// independently bounds the competitor, so for soundness we must
    /// satisfy *every* bucket constraint. Lowering to a single
    /// rate-latency residual we take:
    ///
    /// ```text
    /// R_residual = R - max ρ_i
    /// T_residual = T + max_i ( σ_i / (R - max ρ_j) )
    /// ```
    ///
    /// i.e. the residual rate is `R − max ρ_i` (the most pessimistic,
    /// since the worst-case bucket dominates the rate budget) and the
    /// residual latency adds the worst per-bucket burst-drain time
    /// at that rate. This is sound but not maximally tight — a
    /// piecewise residual service curve (preserving the multi-bucket
    /// structure) is a possible v0.9.x extension once the analysis
    /// pass downstream can consume it.
    ///
    /// Returns [`NcError::UnservableFlow`] when `max ρ_i ≥ R` (no
    /// residual rate available for the tagged flow). The
    /// `σ_i / (R − max ρ_j)` term is rounded up for the same
    /// pessimism reason as the single-bucket version.
    pub fn residual_service(
        beta: &ServiceCurve,
        alpha_competing: &PiecewiseAffineArrivalCurve,
    ) -> Result<ServiceCurve, NcError> {
        let max_rho = alpha_competing.max_sustained_rate_bps();
        if max_rho >= beta.rate_bps {
            return Err(NcError::UnservableFlow);
        }
        let residual_rate = beta.rate_bps - max_rho;
        // For each bucket compute the latency inflation σ_i / R_residual
        // (rounded up) and take the worst one.
        let extra_latency = alpha_competing
            .buckets
            .iter()
            .map(|&(sigma, _)| time_to_send_ps(sigma, residual_rate))
            .max()
            .expect("buckets is non-empty by construction");
        Ok(ServiceCurve {
            rate_bps: residual_rate,
            latency_ps: beta.latency_ps.saturating_add(extra_latency),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::super::{ArrivalCurve, NcError, ServiceCurve};
        use super::*;

        const GBPS: u64 = 1_000_000_000;
        const HUNDRED_MBPS: u64 = 100_000_000;
        const TEN_MBPS: u64 = 10_000_000;
        const TEN_US_PS: u64 = 10_000_000;
        const ONE_US_PS: u64 = 1_000_000;

        #[test]
        fn empty_bucket_list_is_rejected() {
            // Constructor refuses an empty list — α with no buckets
            // would mean "min over empty family", which is undefined.
            let err = PiecewiseAffineArrivalCurve::new(vec![]).unwrap_err();
            assert_eq!(err, PwaError::EmptyBuckets);
        }

        #[test]
        fn single_bucket_matches_single_affine_at_readouts() {
            // The single-bucket case must reproduce the affine
            // ArrivalCurve numerically. σ=1500, ρ=100 Mbps.
            let pwa = PiecewiseAffineArrivalCurve::new(vec![(1500, HUNDRED_MBPS)]).unwrap();
            let affine = ArrivalCurve::affine(1500, HUNDRED_MBPS);
            // Spot a few times: at 0 (causality), at 1 us, at 1 ms.
            assert_eq!(pwa.at(0), affine.at(0));
            assert_eq!(pwa.at(ONE_US_PS), affine.at(ONE_US_PS));
            assert_eq!(pwa.at(1_000_000_000), affine.at(1_000_000_000));
        }

        #[test]
        fn buckets_are_sorted_by_sigma_for_canonical_equality() {
            // Two curves with the same (σ,ρ) set in different input
            // orders should compare equal after canonicalisation.
            let a =
                PiecewiseAffineArrivalCurve::new(vec![(1500, HUNDRED_MBPS), (100, GBPS)]).unwrap();
            let b =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            assert_eq!(a, b);
            // And the canonical layout is σ-ascending.
            assert_eq!(a.buckets(), &[(100, GBPS), (1500, HUNDRED_MBPS)]);
        }

        #[test]
        fn duplicate_buckets_are_deduped() {
            let pwa = PiecewiseAffineArrivalCurve::new(vec![
                (1500, HUNDRED_MBPS),
                (1500, HUNDRED_MBPS),
                (100, GBPS),
            ])
            .unwrap();
            // After dedup: two distinct buckets.
            assert_eq!(pwa.buckets().len(), 2);
            assert_eq!(pwa.buckets(), &[(100, GBPS), (1500, HUNDRED_MBPS)]);
        }

        #[test]
        fn at_takes_min_across_buckets() {
            // Two-bucket curve: short-burst high-rate + large-burst
            // low-rate. At small t the small-σ bucket dominates
            // (small σ + large ρ·t still small); at large t the
            // small-ρ bucket wins because ρ·t is dwarfed by the σ of
            // the high-rate bucket.
            //
            //   B1: σ=100,   ρ=1 Gbps   (peak)
            //   B2: σ=1500,  ρ=100 Mbps (sustained)
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            // at(0) = 0 (causality).
            assert_eq!(pwa.at(0), 0);
            // At small t (1 ns = 1000 ps):
            //   B1 = 100 + 1Gbps · 1000ps / 8e12 = 100 + 0 = 100
            //   B2 = 1500 + 100Mbps · 1000ps / 8e12 = 1500 + 0 = 1500
            //   min = 100 (B1 binds).
            assert_eq!(pwa.at(1_000), 100);
            // At large t (1 ms = 1e9 ps):
            //   B1 = 100 + 1Gbps · 1e9 / 8e12 = 100 + 125_000 = 125_100
            //   B2 = 1500 + 100Mbps · 1e9 / 8e12 = 1500 + 12_500 = 14_000
            //   min = 14_000 (B2 binds — sustained dominates).
            assert_eq!(pwa.at(1_000_000_000), 14_000);
        }

        #[test]
        fn at_zero_is_zero_for_all_buckets() {
            // Causality holds even with very large σ.
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(1_000_000, TEN_MBPS), (10, GBPS)]).unwrap();
            assert_eq!(pwa.at(0), 0);
        }

        #[test]
        fn delay_bound_min_across_buckets_each_dominates_a_regime() {
            // Two-bucket α and a single rate-latency β, choosing β
            // so that *each* bucket binds in some regime. We assert
            // the result is the **min** of per-bucket delay bounds.
            //
            // β: rate=1 Gbps, latency=10 us.
            //   B1: σ=100,  ρ=1 Gbps  → D1 = 10us + 100·8·1e12/1Gbps
            //                              = 10us + 800_000 ps
            //                              = 10_800_000 ps
            //   B2: σ=1500, ρ=100Mbps → D2 = 10us + 1500·8·1e12/1Gbps
            //                              = 10us + 12_000_000 ps
            //                              = 22_000_000 ps
            //   min(D1, D2) = 10_800_000 ps — the small-burst bucket
            //                                  binds.
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
            let d = delay_bound(&pwa, &beta).unwrap();
            assert_eq!(d, TEN_US_PS + 800_000);

            // Sanity: each per-bucket bound matches the
            // single-bucket ArrivalCurve::affine bound on its own.
            let single_b1 =
                super::super::delay_bound(&ArrivalCurve::affine(100, GBPS), &beta).unwrap();
            let single_b2 =
                super::super::delay_bound(&ArrivalCurve::affine(1500, HUNDRED_MBPS), &beta)
                    .unwrap();
            assert_eq!(d, single_b1.min(single_b2));
        }

        #[test]
        fn delay_bound_unstable_when_any_bucket_exceeds_service() {
            // β = 100 Mbps. B1 has ρ=1 Gbps (unstable on this β).
            // Even though B2 is stable (ρ=10 Mbps), we require
            // *every* bucket stable for a uniformly-valid bound.
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, TEN_MBPS)]).unwrap();
            let beta = ServiceCurve::rate_latency(HUNDRED_MBPS, TEN_US_PS);
            assert_eq!(delay_bound(&pwa, &beta), Err(NcError::UnstableServer));
        }

        #[test]
        fn output_bound_inflates_each_bucket_independently() {
            // Each bucket's burst grows by ρ_i·T; rates are preserved.
            //   B1: σ=100,  ρ=1 Gbps,  T=10us → σ' = 100 + 1Gbps·10us/8e12
            //                                       = 100 + 1250 = 1350
            //   B2: σ=1500, ρ=100Mbps, T=10us → σ' = 1500 + 100Mbps·10us/8e12
            //                                       = 1500 + 125  = 1625
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
            let out = output_bound(&pwa, &beta).unwrap();
            // Two buckets out, rates preserved, bursts inflated
            // independently. Canonical ordering by σ ascending:
            // (1350, 1Gbps), (1625, 100Mbps).
            assert_eq!(out.buckets(), &[(1350, GBPS), (1625, HUNDRED_MBPS)]);
        }

        #[test]
        fn residual_service_uses_max_rho_and_max_sigma_drain() {
            // Two competing buckets share a 1 Gbps server.
            //   B1: σ=100,  ρ=400 Mbps
            //   B2: σ=1500, ρ=600 Mbps
            // Conservative residual:
            //   R_residual = 1 Gbps − max(400, 600) Mbps = 400 Mbps
            //   max σ/R_residual = max(100, 1500) · 8·1e12 / 400Mbps
            //                    = 1500 · 8e12 / 4e8
            //                    = 30_000_000 ps = 30 us.
            // Original β has latency 0, so residual latency = 30 us.
            let four_hundred_mbps = 400_000_000u64;
            let six_hundred_mbps = 600_000_000u64;
            let pwa = PiecewiseAffineArrivalCurve::new(vec![
                (100, four_hundred_mbps),
                (1500, six_hundred_mbps),
            ])
            .unwrap();
            let beta = ServiceCurve::rate_latency(GBPS, 0);
            let residual = residual_service(&beta, &pwa).unwrap();
            assert_eq!(residual.rate_bps, four_hundred_mbps);
            assert_eq!(residual.latency_ps, 30_000_000);
        }

        #[test]
        fn residual_service_unservable_when_any_bucket_saturates_rate() {
            // Single competing bucket at 1 Gbps on a 1 Gbps server:
            // even one saturating bucket makes the conservative
            // residual unservable (max ρ = 1 Gbps ≥ R).
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(0, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            let beta = ServiceCurve::rate_latency(GBPS, 0);
            assert_eq!(residual_service(&beta, &pwa), Err(NcError::UnservableFlow));
        }

        #[test]
        fn from_arrival_curve_round_trips_affine() {
            // The single-bucket affine round-trips through `From`.
            let affine = ArrivalCurve::affine(1500, HUNDRED_MBPS);
            let pwa: PiecewiseAffineArrivalCurve = affine.into();
            assert_eq!(pwa.buckets(), &[(1500, HUNDRED_MBPS)]);

            // Spot-check that readouts agree numerically over a few
            // probe times — the round-trip is lossless on the affine
            // single-bucket case.
            for &t in &[1_u64, ONE_US_PS, 1_000_000_000_u64] {
                assert_eq!(pwa.at(t), affine.at(t));
            }
        }

        #[test]
        fn from_arrival_curve_with_peak_encodes_two_buckets() {
            // The peak-capped single-bucket form
            // `min(σ + ρ·t, peak·t)` becomes a 2-bucket
            // PWA: (σ, ρ) and (0, peak).
            let affine = ArrivalCurve::with_peak(1500, HUNDRED_MBPS, GBPS);
            let pwa: PiecewiseAffineArrivalCurve = affine.into();
            // After canonicalisation by σ ascending: (0, GBPS),
            // (1500, 100 Mbps).
            assert_eq!(pwa.buckets(), &[(0, GBPS), (1500, HUNDRED_MBPS)]);

            // Readouts must agree with the peak-capped affine form
            // numerically. The peak dominates at small t (matches
            // affine_arrival_with_peak in the parent test module).
            for &t in &[10_000_u64, ONE_US_PS, 1_000_000_000_u64] {
                assert_eq!(pwa.at(t), affine.at(t));
            }
        }

        #[test]
        fn backlog_bound_min_across_buckets() {
            // Two-bucket α; backlog at a 1 Gbps / 10 us-latency server.
            //   B1: σ=100,  ρ=1 Gbps    → B1_b = 100  + 1Gbps·10us/8e12  = 100  + 1250 = 1350
            //   B2: σ=1500, ρ=100 Mbps  → B2_b = 1500 + 100Mbps·10us/8e12 = 1500 + 125  = 1625
            // min(B1_b, B2_b) = 1350. The small-burst, high-rate
            // bucket binds the worst-case backlog tighter than the
            // sustained bucket.
            let pwa =
                PiecewiseAffineArrivalCurve::new(vec![(100, GBPS), (1500, HUNDRED_MBPS)]).unwrap();
            let beta = ServiceCurve::rate_latency(GBPS, TEN_US_PS);
            let b = backlog_bound(&pwa, &beta).unwrap();
            assert_eq!(b, 1350);
        }
    }
}
