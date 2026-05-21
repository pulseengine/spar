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

fn extract_taprio_gcl(qdisc: &Value) -> Result<Vec<Value>, FixtureError> {
    // `tc -j qdisc show` taprio output embeds sched-entry-list under options.
    // Real `tc -j` output shape (iproute2 6.1):
    //   { "kind": "taprio", "options": { "sched-entry-list": [
    //       { "command": "S", "gatemask": "0x01", "interval": 500000 }
    //   ]}}
    let entries = qdisc
        .get("options")
        .and_then(|o| o.get("sched-entry-list"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            FixtureError::Transform(
                "taprio qdisc missing options.sched-entry-list array".to_string(),
            )
        })?;

    let mut gcl = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        // gatemask is a hex string like "0xff" or a number depending on kernel version.
        let gate_states_value: u8 = match entry.get("gatemask") {
            Some(Value::String(s)) => {
                let s = s.trim_start_matches("0x").trim_start_matches("0X");
                u8::from_str_radix(s, 16).map_err(|e| {
                    FixtureError::Transform(format!(
                        "sched-entry-list[{i}] gatemask {s:?} is not a hex u8: {e}"
                    ))
                })?
            }
            Some(Value::Number(n)) => {
                let v = n.as_u64().ok_or_else(|| {
                    FixtureError::Transform(format!(
                        "sched-entry-list[{i}] gatemask is not a non-negative integer"
                    ))
                })?;
                if v > u64::from(u8::MAX) {
                    return Err(FixtureError::Transform(format!(
                        "sched-entry-list[{i}] gatemask {v} exceeds u8::MAX"
                    )));
                }
                v as u8
            }
            _ => {
                return Err(FixtureError::Transform(format!(
                    "sched-entry-list[{i}] missing or non-string gatemask"
                )));
            }
        };

        let time_interval_value =
            entry
                .get("interval")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    FixtureError::Transform(format!("sched-entry-list[{i}] missing `interval` u64"))
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

    /// Realistic `tc -j qdisc show` output with one taprio and one cbs qdisc.
    /// Values taken from iproute2 6.1 manual examples.
    const TC_TAPRIO_CBS_JSON: &str = r#"[
        {
            "kind": "taprio",
            "handle": "100:",
            "parent": "root",
            "options": {
                "num_tc": 4,
                "map": [0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                "queues": [{ "count": 1, "offset": 0 }],
                "base-time": 0,
                "clockid": "TAI",
                "flags": "0x0",
                "sched-entry-list": [
                    { "command": "S", "gatemask": "0xff", "interval": 400000 },
                    { "command": "S", "gatemask": "0x01", "interval": 100000 }
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

    #[test]
    fn tc_gatemask_as_number_parsed() {
        let tc_json = r#"[{
            "kind": "taprio",
            "options": {
                "sched-entry-list": [
                    { "command": "S", "gatemask": 255, "interval": 200000 }
                ]
            }
        }]"#;
        let out = tc_qdisc_json_to_qcc("swp3", tc_json).unwrap();
        let json_str = serde_json::to_string(&out).unwrap();
        let src = QccYangSwitchConfigSource::from_json_str(&json_str).unwrap();
        let gcl = src.ports()[0].gate_control_list.as_ref().unwrap();
        assert_eq!(gcl[0].gate_states_value, 0xff);
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
