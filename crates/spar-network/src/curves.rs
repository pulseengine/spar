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
//! `output_bound`) implement the closed-form bounds for this case. A
//! piecewise-affine extension is deferred to v0.8.x.
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
/// when it is `None`. For `t = 0`, α(0) = `burst_bytes` regardless of
/// the peak (the burst is the y-intercept).
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
    pub fn at(&self, t_ps: u64) -> u64 {
        if t_ps == 0 {
            return self.burst_bytes;
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
    fn arrival_curve_at_zero_is_burst() {
        let alpha = ArrivalCurve::affine(1500, HUNDRED_MBPS);
        assert_eq!(alpha.at(0), 1500);

        let alpha = ArrivalCurve::with_peak(1500, HUNDRED_MBPS, GBPS);
        assert_eq!(alpha.at(0), 1500);
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
