//! Pure transformer functions: `tc -j qdisc show` JSON → Qcc YANG JSON,
//! and `pmc` text → gPTP JSON.
//!
//! These functions have no I/O and no network-namespace involvement, so they
//! can be unit-tested on any platform (including macOS dev boxes).
//!
//! # Determinism for gPTP timestamps
//!
//! Real gPTP captures produce jittery `timestamp_ns` values that change on
//! every run, making fixture diffs noisy.  The [`pmc_to_gptp_json`] function
//! replaces every `timestamp_ns` with a value derived from a fixed epoch
//! (`TIMESTAMP_EPOCH_NS`) plus a per-sample step (`TIMESTAMP_STEP_NS`).
//! The meaningful field `sync_error_ns` (the absolute master offset) is kept
//! as captured.
//!
//! # Qci stream-filter synthesis
//!
//! Linux's `sch_taprio` has no direct Qci (IEEE 802.1Qci) stream-filter
//! equivalent.  The [`STATIC_STREAM_FILTERS`] constant provides a small
//! synthesised list; the Qcc YANG fixture documents clearly in comments that
//! these entries are synthesised, not observed from the kernel.

use serde_json::{Value, json};

use super::FixtureError;

// ── Constants ─────────────────────────────────────────────────────────────

/// Fixed epoch for synthetic gPTP `timestamp_ns` values (2024-01-01 00:00:00 UTC).
pub const TIMESTAMP_EPOCH_NS: u64 = 1_704_067_200_000_000_000;

/// Per-sample step for synthetic timestamps (100 ms).
pub const TIMESTAMP_STEP_NS: u64 = 100_000_000;

/// Static Qci stream filters injected into the Qcc YANG fixture.
///
/// There is no Linux kernel equivalent for Qci stream filters; these entries
/// are synthesised from the 3-node topology's stream-handle / priority pairs
/// and are clearly marked as synthesised in the generated fixture.
pub const STATIC_STREAM_FILTERS: &[(u32, u8)] = &[
    (1, 6), // critical control traffic, PCP 6
    (2, 5), // audio-video bridging, PCP 5
    (3, 0), // best-effort, PCP 0
];

// ── tc → Qcc YANG ─────────────────────────────────────────────────────────

/// Transform `tc -j qdisc show dev <dev>` JSON into the Qcc YANG shape
/// expected by [`crate::ingest::QccYangSwitchConfigSource`].
///
/// Input: the JSON array produced by `tc -j qdisc show`.
///
/// Output shape:
/// ```json
/// { "interfaces": [
///     { "name": "<port>",
///       "tsn": {
///         "gate-control-list": [ {"gate-states-value": u8, "time-interval-value": u64} ],
///         "bandwidth-reservation-permille": u32,
///         "max-frame-size": 1518,
///         "stream-filters": [ {"stream-handle": u32, "priority-spec": u8} ]
///       }
///     }
/// ]}
/// ```
///
/// Only `taprio` qdiscs are processed for the gate-control list; `cbs` qdiscs
/// supply the bandwidth-reservation.  A port that has neither taprio nor cbs
/// yields a bare `{"name": "..."}` entry (no `tsn` block).
///
/// The `stream-filters` field is always populated from [`STATIC_STREAM_FILTERS`]
/// for taprio ports and is absent on non-taprio ports.
pub fn tc_qdisc_json_to_qcc(port_name: &str, tc_json: &str) -> Result<Value, FixtureError> {
    let arr: Value = serde_json::from_str(tc_json)
        .map_err(|e| FixtureError::Transform(format!("tc JSON parse error: {e}")))?;
    let qdiscs = arr
        .as_array()
        .ok_or_else(|| FixtureError::Transform("tc output is not a JSON array".to_string()))?;

    let mut gcl: Option<Vec<Value>> = None;
    let mut bandwidth_permille: Option<u32> = None;
    let max_frame_size: u16 = 1518;

    for qdisc in qdiscs {
        let kind = qdisc
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if kind == "taprio" {
            gcl = Some(extract_taprio_gcl(qdisc)?);
        } else if kind == "cbs" {
            bandwidth_permille = Some(extract_cbs_permille(qdisc)?);
        }
    }

    let tsn_block = build_tsn_block(gcl, bandwidth_permille, max_frame_size);
    let iface = if let Some(tsn) = tsn_block {
        json!({ "name": port_name, "tsn": tsn })
    } else {
        json!({ "name": port_name })
    };

    Ok(json!({ "interfaces": [iface] }))
}

/// Lists the keys of a JSON object for an error message, so a shape mismatch
/// reports what was actually there instead of only what was missing.
///
/// A parser that says "key X is missing" tells you nothing you did not already
/// know from the fact that it failed. The keys it *did* see are the evidence
/// that identifies the real shape, and in a fixture generated inside a VM this
/// is the only chance to capture them.
fn describe_keys(value: Option<&Value>) -> String {
    match value.and_then(Value::as_object) {
        Some(map) => map
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        None if value.is_some() => "<not a JSON object>".to_string(),
        None => "<absent>".to_string(),
    }
}

/// Pull the gate-control list out of one taprio qdisc of `tc -j qdisc show`.
///
/// The shape below is taken from iproute2's own printer, not from the man page
/// and not from the kernel's netlink vocabulary — those are the *input* grammar
/// (`tc ... sched-entry S ff 400000`) and they do not name the output keys.
/// In `tc/q_taprio.c`, `print_sched_list` opens the array as
///
/// ```text
/// open_json_array(PRINT_JSON, "schedule");
/// ```
///
/// and each entry is `index` / `cmd` / `gatemask` / `interval` (`print_uint`,
/// `print_string`, `print_0xhex`, `print_uint`). `tc/tc_qdisc.c` wraps every
/// qdisc-specific block in `open_json_object("options")`, which is where the
/// `options.` prefix comes from:
///
/// ```text
/// { "kind": "taprio", "options": { "schedule": [
///     { "index": 0, "cmd": "S", "gatemask": "0xff", "interval": 400000 }
/// ]}}
/// ```
///
/// Verified identical across iproute2 v4.20.0 (which introduced taprio), v5.10,
/// v6.1 and v6.17 — the array has been named `schedule` for the tool's whole
/// taprio history, so there is no older spelling to stay compatible with.
/// v6.17 is what this repo's pinned nixos-25.11 channel builds into the guest.
///
/// `taprio_print_opt` prints the active (*oper*) schedule at `options.schedule`
/// and, when a not-yet-active schedule is pending, a second copy nested under
/// `options.admin`. A schedule installed with a base-time in the future is
/// admin-only until that time arrives, so both are accepted — oper first,
/// because when both exist the oper one is what the port is enforcing.
fn extract_taprio_gcl(qdisc: &Value) -> Result<Vec<Value>, FixtureError> {
    let options = qdisc.get("options");
    let admin = options.and_then(|o| o.get("admin"));

    let entries = options
        .and_then(|o| o.get("schedule"))
        .or_else(|| admin.and_then(|a| a.get("schedule")))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            FixtureError::Transform(format!(
                "taprio qdisc has neither options.schedule nor options.admin.schedule; \
                 options keys: [{}]; options.admin keys: [{}]",
                describe_keys(options),
                describe_keys(admin),
            ))
        })?;

    let mut gcl = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        // iproute2 prints gatemask via `print_0xhex`, which in JSON context
        // snprintf's "%#llx" and emits it as a *string* — always, in every
        // release. So the string arm is the real one, and note it is not
        // zero-padded: mask 1 renders "0x1". The numeric arm below is
        // defensive tolerance for a hypothetical other producer, not a shape
        // any released `tc` emits; see `tc_gatemask_as_number_parsed`.
        let gate_states_value: u8 = match entry.get("gatemask") {
            Some(Value::String(s)) => {
                let s = s.trim_start_matches("0x").trim_start_matches("0X");
                u8::from_str_radix(s, 16).map_err(|e| {
                    FixtureError::Transform(format!(
                        "schedule[{i}] gatemask {s:?} is not a hex u8: {e}"
                    ))
                })?
            }
            Some(Value::Number(n)) => {
                let v = n.as_u64().ok_or_else(|| {
                    FixtureError::Transform(format!(
                        "schedule[{i}] gatemask is not a non-negative integer"
                    ))
                })?;
                if v > u64::from(u8::MAX) {
                    return Err(FixtureError::Transform(format!(
                        "schedule[{i}] gatemask {v} exceeds u8::MAX"
                    )));
                }
                v as u8
            }
            _ => {
                return Err(FixtureError::Transform(format!(
                    "schedule[{i}] missing or non-string gatemask"
                )));
            }
        };

        let time_interval_value =
            entry
                .get("interval")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    FixtureError::Transform(format!("schedule[{i}] missing `interval` u64"))
                })?;

        gcl.push(json!({
            "gate-states-value": gate_states_value,
            "time-interval-value": time_interval_value,
        }));
    }
    Ok(gcl)
}

fn extract_cbs_permille(qdisc: &Value) -> Result<u32, FixtureError> {
    // `tc -j qdisc show` cbs output:
    //   { "kind": "cbs", "options": { "idleslope": <i32 kbps>,
    //                                  "sendslope": ..., "hicredit": ..., ... } }
    // We approximate bandwidth reservation as idleslope / port_speed_kbps * 1000.
    // For a 1 Gbps (1_000_000 kbps) port with idleslope = 750000 → 750 permille.
    // If idleslope is absent, fall back to 0.
    const PORT_SPEED_KBPS: i64 = 1_000_000; // 1 Gbps in kbps
    let idleslope = qdisc
        .get("options")
        .and_then(|o| o.get("idleslope"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let idleslope = idleslope.max(0);
    let permille = (idleslope * 1000 / PORT_SPEED_KBPS).clamp(0, 1000) as u32;
    Ok(permille)
}

fn build_tsn_block(
    gcl: Option<Vec<Value>>,
    bandwidth_permille: Option<u32>,
    max_frame_size: u16,
) -> Option<Value> {
    if gcl.is_none() && bandwidth_permille.is_none() {
        return None;
    }

    let mut tsn = serde_json::Map::new();

    if let Some(gcl_list) = gcl {
        tsn.insert("gate-control-list".to_string(), Value::Array(gcl_list));
        // Inject synthesised Qci stream-filters for taprio ports.
        // NOTE: Linux sch_taprio has no Qci equivalent; these entries are
        // synthesised from the 3-node topology config and do NOT reflect
        // observed kernel state.
        let filters: Vec<Value> = STATIC_STREAM_FILTERS
            .iter()
            .map(|(handle, prio)| json!({ "stream-handle": handle, "priority-spec": prio }))
            .collect();
        tsn.insert("stream-filters".to_string(), Value::Array(filters));
        tsn.insert("max-frame-size".to_string(), json!(max_frame_size));
    }

    if let Some(bw) = bandwidth_permille {
        tsn.insert("bandwidth-reservation-permille".to_string(), json!(bw));
    }

    Some(Value::Object(tsn))
}

// ── pmc text → gPTP JSON ──────────────────────────────────────────────────

/// A single parsed pmc sample (before deterministic-timestamp substitution).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PmcRawSample {
    sync_error_ns: u64,
}

/// Transform multiple rounds of `pmc -u -b 0 'GET TIME_STATUS_NP'` text
/// output into the gPTP JSON shape expected by
/// [`crate::ingest::GptpJsonPtpTimeSource`].
///
/// Each `pmc_rounds` entry is the stdout of one `pmc` invocation;
/// successive rounds produce successive samples for the port.
///
/// The `timestamp_ns` for each sample is replaced by a deterministic counter
/// starting at [`TIMESTAMP_EPOCH_NS`] with step [`TIMESTAMP_STEP_NS`].
/// This prevents noisy diffs while still preserving the relative ordering.
///
/// The `sync_error_ns` value is taken as `abs(masterOffset)` from the pmc
/// output.
///
/// Output shape matches [`crate::ingest::GptpJsonPtpTimeSource`]:
/// ```json
/// { "gptp": { "grandmaster": "...", "domain": 0,
///             "ports": [ { "name": "...", "samples": [
///               {"timestamp_ns": u64, "sync_error_ns": u64} ] } ] } }
/// ```
pub fn pmc_to_gptp_json(
    port_name: &str,
    grandmaster: Option<&str>,
    domain: u8,
    pmc_rounds: &[&str],
) -> Result<Value, FixtureError> {
    let mut samples: Vec<Value> = Vec::with_capacity(pmc_rounds.len());

    for (idx, &round_text) in pmc_rounds.iter().enumerate() {
        let raw = parse_pmc_round(round_text)
            .map_err(|e| FixtureError::Transform(format!("pmc round {idx}: {e}")))?;
        let timestamp_ns = TIMESTAMP_EPOCH_NS + (idx as u64) * TIMESTAMP_STEP_NS;
        samples.push(json!({
            "timestamp_ns": timestamp_ns,
            "sync_error_ns": raw.sync_error_ns,
        }));
    }

    let port = json!({ "name": port_name, "samples": samples });

    let gptp_inner = match grandmaster {
        Some(gm) => json!({
            "grandmaster": gm,
            "domain": domain,
            "ports": [port],
        }),
        None => json!({
            "domain": domain,
            "ports": [port],
        }),
    };

    Ok(json!({ "gptp": gptp_inner }))
}

/// Parse a single `pmc -u -b 0 'GET TIME_STATUS_NP'` text block.
///
/// The relevant line has the form:
/// ```text
///     masterOffset              -42
/// ```
/// We take the absolute value to satisfy the schema's requirement for
/// non-negative `sync_error_ns`.
fn parse_pmc_round(text: &str) -> Result<PmcRawSample, String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("masterOffset") {
            let val_str = rest.trim();
            let val: i64 = val_str
                .parse()
                .map_err(|e| format!("cannot parse masterOffset {val_str:?}: {e}"))?;
            return Ok(PmcRawSample {
                sync_error_ns: val.unsigned_abs(),
            });
        }
    }
    Err("pmc output missing `masterOffset` line".to_string())
}

// ── LLDP JSON reshaping ───────────────────────────────────────────────────

/// Reshape `lldpctl -f json` output into a normalised form.
///
/// lldpd already emits the canonical shape; this function is a pass-through
/// that validates the top-level `lldp.interface` key exists, then returns
/// the value unchanged.  It exists as a named function so callers can pipe
/// through it and so that unit tests can assert against known shapes.
pub fn validate_lldp_json(lldp_raw: &str) -> Result<Value, FixtureError> {
    let v: Value = serde_json::from_str(lldp_raw)
        .map_err(|e| FixtureError::Transform(format!("lldp JSON parse error: {e}")))?;
    // Validate top-level structure so errors surface here, not in ingest.
    v.get("lldp")
        .and_then(|l| l.get("interface"))
        .ok_or_else(|| {
            FixtureError::Transform(
                "lldpctl JSON missing `lldp.interface` key — \
                 is lldpd running and has it observed any neighbors?"
                    .to_string(),
            )
        })?;
    Ok(v)
}

// ── Unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{
        GptpJsonPtpTimeSource, PtpTimeSource, QccYangSwitchConfigSource, SwitchConfigSource,
    };

    // ── tc → Qcc YANG ──────────────────────────────────────────────────────

    /// `tc -j qdisc show` output for the taprio + cbs pair that
    /// `bin/gen-fixtures.rs` installs on the switch veth.
    ///
    /// Every key here is derived from iproute2's printer source (v6.17.0,
    /// `tc/q_taprio.c` + `tc/q_cbs.c`, wrapped in `options` by
    /// `tc/tc_qdisc.c:315`), NOT from the man page. That distinction is the
    /// whole reason this constant is being rewritten: the previous version was
    /// written from the *input* grammar and claimed in a comment to be "from
    /// iproute2 6.1 manual examples". It used `sched-entry-list` (the netlink
    /// attribute `TCA_TAPRIO_ATTR_SCHED_ENTRY_LIST`, lowercased) and `command`
    /// (the man page's argument name), neither of which iproute2 has ever
    /// emitted in JSON — checked against v4.20.0, v5.10, v6.1 and v6.17. The
    /// parser was written to match that invented shape, so this test passed for
    /// as long as it existed while the real readback in the VM could only fail.
    ///
    /// Note `"0x1"`, not `"0x01"`: gatemask goes through `print_0xhex`, which
    /// formats with `%#llx` and does not zero-pad.
    ///
    /// Faithfulness is claimed for the structure and for the fields the
    /// transform reads (`kind`, `options`, `schedule`, `gatemask`, `interval`,
    /// `idleslope`). The surrounding cosmetic fields are representative.
    const TC_TAPRIO_CBS_JSON: &str = r#"[
        {
            "kind": "taprio",
            "handle": "100:",
            "parent": "root",
            "options": {
                "tc": 4,
                "map": [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "queues": [
                    { "offset": 0, "count": 1 },
                    { "offset": 1, "count": 1 },
                    { "offset": 2, "count": 1 },
                    { "offset": 3, "count": 1 }
                ],
                "clockid": "TAI",
                "flags": "0x0",
                "base_time": 0,
                "cycle_time": 500000,
                "schedule": [
                    { "index": 0, "cmd": "S", "gatemask": "0xff", "interval": 400000 },
                    { "index": 1, "cmd": "S", "gatemask": "0x1", "interval": 100000 }
                ]
            }
        },
        {
            "kind": "cbs",
            "handle": "200:",
            "parent": "100:1",
            "options": {
                "idleslope": 750000,
                "sendslope": -250000,
                "hicredit": 34,
                "locredit": -15
            }
        }
    ]"#;

    #[test]
    fn tc_taprio_cbs_round_trips_qcc() {
        let out = tc_qdisc_json_to_qcc("swp1", TC_TAPRIO_CBS_JSON).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();

        // Must parse cleanly through QccYangSwitchConfigSource.
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let ports = src.ports();
        assert_eq!(ports.len(), 1);

        let p = &ports[0];
        assert_eq!(p.port_name, "swp1");

        // Two GCL entries: 0xff / 400000 ns, 0x01 / 100000 ns.
        let gcl = p.gate_control_list.as_ref().unwrap();
        assert_eq!(gcl.len(), 2);
        assert_eq!(gcl[0].gate_states_value, 0xff);
        assert_eq!(gcl[0].time_interval_value, 400_000);
        assert_eq!(gcl[1].gate_states_value, 0x01);
        assert_eq!(gcl[1].time_interval_value, 100_000);

        // CBS: 750000 kbps / 1000000 kbps * 1000 = 750 permille.
        assert_eq!(p.bandwidth_reservation_permille, Some(750));
        assert_eq!(p.max_frame_size, Some(1518));

        // Synthesised Qci stream filters.
        let filters = p.streams.as_ref().unwrap();
        assert_eq!(filters.len(), STATIC_STREAM_FILTERS.len());
        for (filter, &(expected_handle, expected_prio)) in
            filters.iter().zip(STATIC_STREAM_FILTERS.iter())
        {
            assert_eq!(filter.stream_handle, expected_handle);
            assert_eq!(filter.priority_spec, expected_prio);
        }
    }

    #[test]
    fn tc_no_taprio_no_cbs_yields_bare_port() {
        let tc_json =
            r#"[{"kind": "pfifo_fast", "handle": "0:", "parent": "root", "options": {}}]"#;
        let out = tc_qdisc_json_to_qcc("swp2", tc_json).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let p = &src.ports()[0];
        assert_eq!(p.port_name, "swp2");
        assert!(p.gate_control_list.is_none());
        assert!(p.bandwidth_reservation_permille.is_none());
        assert!(p.streams.is_none());
    }

    /// iproute2 renders gatemask through `print_0xhex`, which in JSON context
    /// always emits a string. The numeric arm of the parser is therefore
    /// defensive, not a shape any released `tc` produces — this test pins that
    /// tolerance so a future refactor does not drop it, but passing it is NOT
    /// evidence about real `tc` output.
    #[test]
    fn tc_gatemask_as_number_parsed() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": {
                "schedule": [
                    { "index": 0, "cmd": "S", "gatemask": 255, "interval": 200000 }
                ]
            }
        }]"#;
        let out = tc_qdisc_json_to_qcc("swp3", tc_json).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let gcl = src.ports()[0].gate_control_list.as_ref().unwrap();
        assert_eq!(gcl[0].gate_states_value, 0xff);
    }

    /// The shape this parser used to accept must now be REJECTED.
    ///
    /// This is the guard against the defect that produced this whole fix. The
    /// old fixture and the old parser encoded the same invented shape, so they
    /// agreed with each other and the test passed while the only real `tc` in
    /// the project — the one in the fixture VM — could never satisfy it. Two
    /// sides sharing one misconception is not a round trip.
    ///
    /// Pinning the *rejection* is what makes the accept-test mean something:
    /// the two shapes now produce different outcomes, so the suite can tell
    /// them apart. A parser lenient enough to take both would pass its accept
    /// test either way and prove nothing about which one `tc` emits.
    ///
    /// Rejecting costs no compatibility: `sched-entry-list` has never appeared
    /// in iproute2's JSON output in any release since taprio landed in v4.20.0.
    #[test]
    fn tc_rejects_the_netlink_attribute_shape() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": {
                "sched-entry-list": [
                    { "command": "S", "gatemask": "0xff", "interval": 400000 }
                ]
            }
        }]"#;
        let err = tc_qdisc_json_to_qcc("swp1", tc_json)
            .expect_err("the netlink-attribute spelling must not be accepted as tc output");

        // And the message must carry the evidence: the keys actually present.
        // Without this, a VM-side failure reports only what it wanted, which is
        // exactly how the real shape stayed unknown through six dispatches.
        let msg = err.to_string();
        assert!(
            msg.contains("sched-entry-list"),
            "error must name the keys it saw, got: {msg}"
        );
    }

    /// A schedule whose base-time has not yet arrived is reported by `tc` only
    /// under `options.admin`; `options.schedule` carries the *oper* schedule and
    /// is absent until the kernel promotes it. Reading back immediately after
    /// configuring can land in that window.
    #[test]
    fn tc_taprio_admin_only_schedule_is_read() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": {
                "tc": 4,
                "clockid": "TAI",
                "admin": {
                    "base_time": 9999999999,
                    "cycle_time": 500000,
                    "schedule": [
                        { "index": 0, "cmd": "S", "gatemask": "0xff", "interval": 400000 }
                    ]
                }
            }
        }]"#;
        let out = tc_qdisc_json_to_qcc("swp1", tc_json).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let gcl = src.ports()[0].gate_control_list.as_ref().unwrap();
        assert_eq!(gcl.len(), 1);
        assert_eq!(gcl[0].gate_states_value, 0xff);
        assert_eq!(gcl[0].time_interval_value, 400_000);
    }

    /// When both are present the *oper* schedule wins: it is what the port is
    /// enforcing right now, while `admin` is a pending replacement.
    #[test]
    fn tc_taprio_prefers_oper_schedule_over_admin() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": {
                "schedule": [
                    { "index": 0, "cmd": "S", "gatemask": "0x0f", "interval": 111000 }
                ],
                "admin": {
                    "schedule": [
                        { "index": 0, "cmd": "S", "gatemask": "0xf0", "interval": 222000 }
                    ]
                }
            }
        }]"#;
        let out = tc_qdisc_json_to_qcc("swp1", tc_json).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let gcl = src.ports()[0].gate_control_list.as_ref().unwrap();
        assert_eq!(gcl.len(), 1);
        assert_eq!(gcl[0].gate_states_value, 0x0f);
        assert_eq!(gcl[0].time_interval_value, 111_000);
    }

    /// A shape mismatch must report the keys it *did* see.
    ///
    /// The failing dispatch said only "taprio qdisc missing
    /// options.sched-entry-list array", which restates the assumption instead of
    /// reporting the observation — the run therefore cost a full VM dispatch and
    /// returned no information about the real shape. `describe_keys` exists so
    /// the next unknown shape is identified by the run that hits it.
    #[test]
    fn taprio_shape_error_reports_observed_keys() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": { "tc": 4, "clockid": "TAI", "some-future-spelling": [] }
        }]"#;
        let msg = tc_qdisc_json_to_qcc("swp1", tc_json)
            .expect_err("unknown schedule spelling must fail")
            .to_string();

        for expected in ["tc", "clockid", "some-future-spelling"] {
            assert!(
                msg.contains(expected),
                "error must list observed key {expected:?}, got: {msg}"
            );
        }
        assert!(
            msg.contains("<absent>"),
            "error must say options.admin was absent, got: {msg}"
        );
    }

    // ── pmc → gPTP JSON ───────────────────────────────────────────────────

    /// Realistic `pmc -u -b 0 'GET TIME_STATUS_NP'` output fragment.
    /// Format from linuxptp 4.x.
    const PMC_ROUND_1: &str = r#"
sending: GET TIME_STATUS_NP
    7cfe90.fffe.000001-0 seq 0 RESPONSE MANAGEMENT TIME_STATUS_NP
        master_offset              -42
        ingress_time               1704067200100000000
        cumulativeScaledRateOffset +0.000000000
        scaledLastGmPhaseChange    0
        gmTimeBaseIndicator        0
        lastGmPhaseChange          0x0000'0000000000000000.0000
        gmPresent                  true
        gmIdentity                 00:1b:21:ff:fe:01:02:03
    masterOffset              -42
    ingress_time               1704067200100000000
"#;

    const PMC_ROUND_2: &str = r#"
sending: GET TIME_STATUS_NP
    7cfe90.fffe.000001-0 seq 1 RESPONSE MANAGEMENT TIME_STATUS_NP
    masterOffset              15
    ingress_time               1704067200200000000
"#;

    #[test]
    fn pmc_parses_negative_master_offset_as_abs() {
        let result =
            pmc_to_gptp_json("eth0", Some("00:1b:21:ff:fe:01:02:03"), 0, &[PMC_ROUND_1]).unwrap();
        let json_str = serde_json::to_string(&result).unwrap();
        let src = GptpJsonPtpTimeSource::from_json_str(&json_str).unwrap();
        assert_eq!(src.grandmaster(), Some("00:1b:21:ff:fe:01:02:03"));
        assert_eq!(src.domain(), Some(0));
        let samples = &src.ports()[0].samples;
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].sync_error_ns, 42); // abs(-42)
    }

    #[test]
    fn pmc_positive_master_offset_unchanged() {
        let result = pmc_to_gptp_json("eth0", None, 20, &[PMC_ROUND_2]).unwrap();
        let json_str = serde_json::to_string(&result).unwrap();
        let src = GptpJsonPtpTimeSource::from_json_str(&json_str).unwrap();
        assert_eq!(src.ports()[0].samples[0].sync_error_ns, 15);
    }

    #[test]
    fn pmc_timestamp_determinism_two_rounds() {
        // Same input twice → identical timestamp_ns sequence both times.
        let rounds = &[PMC_ROUND_1, PMC_ROUND_2];
        let result1 = pmc_to_gptp_json("eth0", None, 0, rounds).unwrap();
        let result2 = pmc_to_gptp_json("eth0", None, 0, rounds).unwrap();

        // The timestamps are derived from a fixed epoch, not wall-clock.
        assert_eq!(result1, result2);

        // Verify exact values.
        let samples = result1["gptp"]["ports"][0]["samples"].as_array().unwrap();
        assert_eq!(
            samples[0]["timestamp_ns"].as_u64().unwrap(),
            TIMESTAMP_EPOCH_NS
        );
        assert_eq!(
            samples[1]["timestamp_ns"].as_u64().unwrap(),
            TIMESTAMP_EPOCH_NS + TIMESTAMP_STEP_NS
        );
    }

    #[test]
    fn pmc_round_trips_gptp_ingest() {
        let result = pmc_to_gptp_json(
            "eth0",
            Some("00:1b:21:ff:fe:01:02:03"),
            0,
            &[PMC_ROUND_1, PMC_ROUND_2],
        )
        .unwrap();
        let json_str = serde_json::to_string(&result).unwrap();
        let src = GptpJsonPtpTimeSource::from_json_str(&json_str).unwrap();
        assert_eq!(src.ports().len(), 1);
        assert_eq!(src.ports()[0].name, "eth0");
        assert_eq!(src.ports()[0].samples.len(), 2);
        // First sample: abs(-42) = 42.
        assert_eq!(src.ports()[0].samples[0].sync_error_ns, 42);
        // Second sample: abs(15) = 15.
        assert_eq!(src.ports()[0].samples[1].sync_error_ns, 15);
    }

    // ── LLDP passthrough ──────────────────────────────────────────────────

    const LLDP_JSON: &str = r#"{
        "lldp": {
            "interface": [
                {
                    "name": "veth0",
                    "chassis": {
                        "switch": {
                            "id": {"type": "mac", "value": "aa:bb:cc:dd:ee:01"},
                            "name": "grandmaster"
                        }
                    },
                    "port": {
                        "id": {"type": "ifname", "value": "veth1"}
                    }
                }
            ]
        }
    }"#;

    #[test]
    fn lldp_validate_passthrough() {
        let v = validate_lldp_json(LLDP_JSON).unwrap();
        assert!(v.get("lldp").is_some());
    }

    #[test]
    fn lldp_validate_rejects_missing_interface() {
        let bad = r#"{"lldp": {}}"#;
        let err = validate_lldp_json(bad).unwrap_err();
        assert!(matches!(err, FixtureError::Transform(_)));
    }
}
