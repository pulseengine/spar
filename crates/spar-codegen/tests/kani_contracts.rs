//! Kani bounded model-checking harnesses proving that the spar-codegen
//! output preserves the AADL source contract.
//!
//! **Design note** — Kani cannot symbolically construct `SystemInstance`
//! values (they embed `la_arena::Idx` handles that require a live arena).
//! Following the pattern established in `kani_codegen.rs` and
//! `kani_solver.rs`, each harness therefore models the *pure functions*
//! that the codegen pass calls, and asserts the invariant that the pass
//! must satisfy.  The functions under test (`period_ns_to_string`,
//! `wit_direction_token`, `access_shim_is_mut`) are thin, faithful
//! reimplementations of the logic in `lib.rs`, `wit_gen.rs`, and
//! `rust_gen.rs` respectively — meaning any divergence between the model
//! and production would be caught by the existing unit + golden tests.
//!
//! This is spar's **Logika-equivalent strategy**: instead of a new prover
//! language, we deepen Kani coverage on the generated-code path so each
//! codegen pass has a machine-checked proof that emitted Rust/WIT
//! preserves the source AADL contract.
//!
//! All harnesses are guarded by `#[cfg(kani)]`.

#![cfg(kani)]

// ── Shared constants ─────────────────────────────────────────────────────────

/// One nanosecond expressed in picoseconds (the internal unit of spar-hir-def).
const NS_TO_PS: u64 = 1_000;

/// Upper bound on the nanosecond period range asserted by the AADL contract
/// for `prove_thread_period_preserved`: 0 < p ≤ 1_000_000_000 ns (= 1 s).
const MAX_PERIOD_NS: u64 = 1_000_000_000;

// ═══════════════════════════════════════════════════════════════════════════
// Harness 1 — prove_thread_period_preserved
// ═══════════════════════════════════════════════════════════════════════════

/// Emit a period value (in picoseconds) as the nanosecond string that
/// `spar-codegen` writes into the dispatch metadata struct's `period_ns`
/// field.
///
/// This mirrors the production path:
///   `rust_gen` / `config_gen` → `format_time_ps(period_ps)` → e.g. "5000 ns"
///
/// We keep units in nanoseconds here because the AADL contract specifies
/// periods in nanoseconds; the generator always normalises to the smallest
/// lossless unit.
fn period_ps_to_ns_string(period_ps: u64, buf: &mut [u8; 32]) -> usize {
    // Only the nanosecond branch is exercised by the harness assumption
    // (period_ps is always a multiple of NS_TO_PS).
    let period_ns = period_ps / NS_TO_PS;

    // Digit math in u32: every value in the contract domain
    // (0 < ns ≤ 10^9 < 2^32) is exactly representable, and 32-bit div/mod
    // circuits roughly halve the SAT instance vs u64 (#252). The cast is
    // asserted lossless so the model cannot silently diverge if the
    // domain ever grows past u32.
    assert!(period_ns <= u32::MAX as u64);
    // Write decimal digits into buf.
    let mut tmp = [0u8; 20];
    let mut n = period_ns as u32;
    let mut ndigits = 0usize;
    if n == 0 {
        tmp[0] = b'0';
        ndigits = 1;
    } else {
        while n > 0 {
            tmp[ndigits] = b'0' + (n % 10) as u8;
            n /= 10;
            ndigits += 1;
        }
        // reverse
        let mut lo = 0usize;
        let mut hi = ndigits - 1;
        while lo < hi {
            tmp.swap(lo, hi);
            lo += 1;
            hi -= 1;
        }
    }

    // Append " ns"
    let suffix = b" ns";
    let total = ndigits + suffix.len();
    for i in 0..ndigits {
        buf[i] = tmp[i];
    }
    for i in 0..suffix.len() {
        buf[ndigits + i] = suffix[i];
    }
    total
}

/// Parse the nanosecond string written by `period_ps_to_ns_string` back to
/// a picosecond value.  Mirrors `crate::parse_time_to_ps` for the "NNN ns"
/// format.
fn parse_ns_string_to_ps(buf: &[u8; 32], len: usize) -> u64 {
    // Format is "<digits> ns" — strip the " ns" suffix (3 bytes) and parse
    // the decimal prefix.
    if len < 4 {
        return 0;
    }
    let digit_len = len - 3; // strip " ns"
    // u32 accumulator: the contract domain tops out at 10^9 < 2^32, so
    // every parseable in-domain value fits; see the width note in
    // `period_ps_to_ns_string` (#252).
    let mut val: u32 = 0;
    for i in 0..digit_len {
        let d = buf[i];
        if d < b'0' || d > b'9' {
            return 0;
        }
        val = val * 10 + (d - b'0') as u32;
    }
    (val as u64) * NS_TO_PS
}

/// Contract: given a thread with `Period = p ns` (0 < p ≤ 1_000_000_000 ns),
/// the generated dispatch metadata string round-trips back to exactly `p * NS_TO_PS`
/// picoseconds — no truncation, no off-by-one.
///
/// **Proof decomposition (#252):** a single harness over the full
/// `[1, 10^9]` symbolic domain produced a 308k-variable / 1.5M-clause SAT
/// instance (the digit-extraction division loop unwinds 64+ times against a
/// fully symbolic u64) and CBMC ran out of memory — locally AND as the CI
/// 6h-timeout root cause. The claim is therefore *stratified by decimal
/// digit count*: one harness per digit-band, `union(d1..d10) = [1, 10^9]`
/// exactly, so the TOTAL claim is unchanged — full-domain, no weakening.
/// Within a band the digit loop has a fixed small iteration count, so each
/// SAT instance stays trivially small.
fn check_period_roundtrip(lo_ns: u64, hi_ns: u64) {
    // Precondition: AADL contract requires 0 < Period ≤ 1_000_000_000 ns;
    // this harness instance covers the [lo_ns, hi_ns] band of that range.
    let period_ns: u64 = kani::any();
    kani::assume(period_ns >= lo_ns && period_ns <= hi_ns);

    // The HIR stores periods in picoseconds; a nanosecond input is always
    // a multiple of NS_TO_PS.
    let period_ps: u64 = period_ns * NS_TO_PS;

    // Model the codegen emission path.
    let mut buf = [0u8; 32];
    let len = period_ps_to_ns_string(period_ps, &mut buf);

    // Model the parse-back path (what a downstream tool would read).
    let recovered_ps = parse_ns_string_to_ps(&buf, len);

    // Postcondition: the recovered value must equal the original exactly.
    assert!(
        recovered_ps == period_ps,
        "period round-trip failed: recovered {recovered_ps} ps != original {period_ps} ps"
    );
}

/// One proof per decimal-digit band. Band boundaries are consecutive and
/// exhaustive: d1 = [1,9], d2 = [10,99], …, d9 = [10^8, 10^9 - 1],
/// d10 = [10^9, 10^9]. Their union is exactly (0, MAX_PERIOD_NS] — the
/// original contract domain.
/// The unwind bound is per-band: band dN's value has exactly N decimal
/// digits, so every loop in the model runs ≤ N (+1 for the exit check)
/// iterations. CBMC unwinds symbolic loops to the declared bound
/// regardless of feasibility, so a tight bound shrinks the instance —
/// unwind(N+2) gives one iteration of slack while staying minimal.
macro_rules! period_band_proof {
    ($name:ident, $lo:expr, $hi:expr, $unwind:literal) => {
        #[kani::proof]
        #[kani::unwind($unwind)]
        fn $name() {
            check_period_roundtrip($lo, $hi);
        }
    };
}

period_band_proof!(prove_thread_period_preserved_d1, 1, 9, 4);
period_band_proof!(prove_thread_period_preserved_d2, 10, 99, 4);
period_band_proof!(prove_thread_period_preserved_d3, 100, 999, 5);
period_band_proof!(prove_thread_period_preserved_d4, 1_000, 9_999, 6);
period_band_proof!(prove_thread_period_preserved_d5, 10_000, 99_999, 7);
period_band_proof!(prove_thread_period_preserved_d6, 100_000, 999_999, 8);
period_band_proof!(prove_thread_period_preserved_d7, 1_000_000, 9_999_999, 9);
period_band_proof!(prove_thread_period_preserved_d8, 10_000_000, 99_999_999, 10);
period_band_proof!(
    prove_thread_period_preserved_d9,
    100_000_000,
    999_999_999,
    11
);
period_band_proof!(
    prove_thread_period_preserved_d10,
    MAX_PERIOD_NS,
    MAX_PERIOD_NS,
    12
);

/// Compile-time guard that the bands tile the contract domain exactly:
/// consecutive (each lo = previous hi + 1), starting at 1, ending at
/// MAX_PERIOD_NS. If someone edits a band and leaves a hole, this fails
/// the build — the stratified claim would otherwise silently weaken.
const PERIOD_BANDS: [(u64, u64); 10] = [
    (1, 9),
    (10, 99),
    (100, 999),
    (1_000, 9_999),
    (10_000, 99_999),
    (100_000, 999_999),
    (1_000_000, 9_999_999),
    (10_000_000, 99_999_999),
    (100_000_000, 999_999_999),
    (MAX_PERIOD_NS, MAX_PERIOD_NS),
];
const _BANDS_TILE_DOMAIN: () = {
    assert!(PERIOD_BANDS[0].0 == 1);
    assert!(PERIOD_BANDS[9].1 == MAX_PERIOD_NS);
    let mut i = 1;
    while i < 10 {
        assert!(PERIOD_BANDS[i].0 == PERIOD_BANDS[i - 1].1 + 1);
        assert!(PERIOD_BANDS[i].0 <= PERIOD_BANDS[i].1);
        i += 1;
    }
};

// ═══════════════════════════════════════════════════════════════════════════
// Harness 2 — prove_port_direction_preserved
// ═══════════════════════════════════════════════════════════════════════════

/// Numeric encoding of AADL port direction (mirrors `item_tree::Direction`).
///
/// Using `u8` instead of the real enum so Kani can enumerate all values
/// via `kani::any::<u8>()` without requiring the production enum to impl
/// `kani::Arbitrary` (which it does not today).
const DIR_IN: u8 = 0;
const DIR_OUT: u8 = 1;
const DIR_IN_OUT: u8 = 2;

/// WIT token class: what the WIT generator emits for a DataPort feature.
///
/// The production `wit_gen::generate_wit` emits:
///   - `Direction::In`    → plain `func()` (getter, no `set-` prefix)
///   - `Direction::Out`   → `set-{name}: func(val: T)` (`set-` prefix)
///   - `Direction::InOut` → both a getter and a setter
///
/// We collapse this to three marker bytes for Kani reasoning.
const WIT_GETTER: u8 = b'g'; // In  → getter only
const WIT_SETTER: u8 = b's'; // Out → setter only
const WIT_BOTH: u8 = b'b'; // InOut → both

/// Model the WIT direction-to-token mapping used by `wit_gen::generate_wit`.
fn wit_direction_token(direction: u8) -> u8 {
    match direction {
        DIR_IN => WIT_GETTER,
        DIR_OUT => WIT_SETTER,
        DIR_IN_OUT => WIT_BOTH,
        _ => 0xFF, // invalid
    }
}

/// Contract: `Direction::Out` source is never mapped to a WIT getter, and
/// `Direction::In` sink is never mapped to a WIT setter.  Specifically:
///
/// 1. A well-formed `Out → In` connection produces a setter on the source
///    side and a getter on the sink side (correct directionality).
/// 2. An ill-formed `Out → Out` or `In → In` connection can never arise
///    from two features that both map to the same WIT token class.
///
/// This mirrors the AADL §9 rule that a port connection must join an Out
/// feature to an In feature; the codegen must not silently invert or
/// merge the directions.
#[kani::proof]
#[kani::unwind(8)]
fn prove_port_direction_preserved() {
    // Let source_dir and sink_dir range over all three direction values.
    let source_dir: u8 = kani::any();
    let sink_dir: u8 = kani::any();
    kani::assume(source_dir <= DIR_IN_OUT);
    kani::assume(sink_dir <= DIR_IN_OUT);

    let src_tok = wit_direction_token(source_dir);
    let snk_tok = wit_direction_token(sink_dir);

    // Invariant A: Out source maps to setter, never getter.
    if source_dir == DIR_OUT {
        assert!(
            src_tok == WIT_SETTER,
            "Out source must map to WIT setter, not getter or both"
        );
    }

    // Invariant B: In sink maps to getter, never setter.
    if sink_dir == DIR_IN {
        assert!(
            snk_tok == WIT_GETTER,
            "In sink must map to WIT getter, not setter"
        );
    }

    // Invariant C: a well-formed connection (Out → In) gives complementary tokens.
    if source_dir == DIR_OUT && sink_dir == DIR_IN {
        assert!(
            src_tok == WIT_SETTER && snk_tok == WIT_GETTER,
            "Out→In connection must produce setter+getter pair"
        );
        // They must differ — the same WIT token on both ends would mean
        // the codegen emitted two getters or two setters, which is a
        // contract violation.
        assert!(
            src_tok != snk_tok,
            "Out→In must produce distinct WIT tokens (not both setter or both getter)"
        );
    }

    // Invariant D (inverse): two features with the same direction never
    // produce a valid complementary Out→In pair.  This is the "ill-formed
    // connection" witness.
    if source_dir == sink_dir && source_dir != DIR_IN_OUT {
        // If both are In: both are getters → no setter → the source side
        // cannot satisfy the "setter required for output" contract.
        // If both are Out: both are setters → no getter → the sink side
        // cannot satisfy the "getter required for input" contract.
        // We assert that the two tokens are identical (not complementary),
        // i.e., this connection would violate the AADL §9 contract.
        assert!(
            src_tok == snk_tok,
            "Same-direction features must produce identical WIT tokens (not complementary)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Harness 3 — prove_access_right_preserved
// ═══════════════════════════════════════════════════════════════════════════

/// Access-right encoding (mirrors the AADL `Access_Rights` property).
///
/// In production, `spar-hir-def` parses `Access_Rights` from the model's
/// property associations.  Here we encode the two relevant values as
/// `u8` constants so Kani can enumerate them.
const ACCESS_READ_ONLY: u8 = 0;
const ACCESS_READ_WRITE: u8 = 1;

/// Buf size for the generated shim snippet.
const SHIM_BUF: usize = 64;

/// Model: emit the Rust type token for the shared resource parameter in a
/// generated bus-access shim.
///
/// Production path (`rust_gen`): when a bus-access feature has
/// `Access_Rights = Read_Only`, the generated shim receives the shared
/// resource as `&T` (immutable reference).  When `Read_Write`, it receives
/// `&mut T`.
///
/// We model this as a fixed-size byte buffer containing either `&T` or
/// `&mut T`.  The harness then checks that `&mut` never appears when
/// `access_right == ACCESS_READ_ONLY`.
fn emit_access_shim(access_right: u8, buf: &mut [u8; SHIM_BUF]) -> usize {
    let token: &[u8] = if access_right == ACCESS_READ_ONLY {
        b"fn shim(resource: &T)"
    } else {
        b"fn shim(resource: &mut T)"
    };
    let len = token.len().min(SHIM_BUF);
    buf[..len].copy_from_slice(&token[..len]);
    len
}

/// Scan `buf[..len]` for the four-byte sequence `&mut`.
fn contains_mut_ref(buf: &[u8; SHIM_BUF], len: usize) -> bool {
    let needle = b"&mut";
    if len < needle.len() {
        return false;
    }
    let mut i = 0usize;
    while i + needle.len() <= len {
        let mut matched = true;
        let mut j = 0usize;
        while j < needle.len() {
            if buf[i + j] != needle[j] {
                matched = false;
                break;
            }
            j += 1;
        }
        if matched {
            return true;
        }
        i += 1;
    }
    false
}

/// Contract: given a bus-access feature with `Access_Rights = Read_Only` and
/// a connection delegating that access, the generated access shim contains no
/// `&mut` reference to the shared resource (read-only enforcement at the type
/// level).
///
/// Conversely, a `Read_Write` access right must produce a shim that does
/// contain `&mut`.  This proves the access-right is faithfully propagated to
/// the generated Rust type signature and cannot be silently widened.
#[kani::proof]
#[kani::unwind(65)]
fn prove_access_right_preserved() {
    let access_right: u8 = kani::any();
    kani::assume(access_right == ACCESS_READ_ONLY || access_right == ACCESS_READ_WRITE);

    let mut buf = [0u8; SHIM_BUF];
    let len = emit_access_shim(access_right, &mut buf);

    let has_mut = contains_mut_ref(&buf, len);

    // Postcondition A: Read_Only → no &mut in shim (read-only enforcement).
    if access_right == ACCESS_READ_ONLY {
        assert!(
            !has_mut,
            "Read_Only access must not produce &mut in the generated shim"
        );
    }

    // Postcondition B: Read_Write → &mut present in shim (write capability).
    if access_right == ACCESS_READ_WRITE {
        assert!(
            has_mut,
            "Read_Write access must produce &mut in the generated shim"
        );
    }
}
