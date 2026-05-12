//! Runtime-artefact parsers feeding the v0.11.0 reconciliation engine.
//!
//! v0.10.x lands these incrementally:
//!
//! - **PCAPNG** ([`PcapngFrameSource`]) — implemented in v0.10.x B-2.
//!   Uses Pierre Chifflier's `pcap-parser` crate; yields typed
//!   [`CapturedFrame`] records carrying L2 identity (mac_src, mac_dst,
//!   optional 802.1Q VLAN-ID and PCP) plus a Unix-epoch nanosecond
//!   timestamp resolved via the per-IDB `ts_resol` option.
//! - **LLDP** ([`LldpJsonTopologySource`]) — implemented in v0.10.x B-3.
//!   Backed by `lldpctl -f json` output (see <https://lldpd.github.io/>);
//!   yields [`LldpNeighbor`] records carrying local_port + typed
//!   remote chassis-id / port-id / system-name.
//! - **Qcc YANG** ([`QccYangSwitchConfigSource`]) — implemented in v0.10.x B-4.
//!   Parses an `ieee802-dot1q-tsn-types`-shaped JSON dump (typically
//!   retrieved over NETCONF/RESTCONF, or extracted from a `tc qdisc`
//!   snapshot transformed to the canonical shape) into per-port
//!   [`PortConfig`] records carrying the 802.1Qbv gate-control-list,
//!   CBS bandwidth reservation (permille), max-frame-size, and Qci
//!   stream filters.
//! - **gPTP** ([`GptpJsonPtpTimeSource`]) — implemented in v0.10.x B-5.
//!   Parses a JSON sync-error dump (typically produced by a wrapper
//!   around `pmc` or `ptp4l`); yields per-port `PtpSample` streams that
//!   the reconciler compares against `Spar_TSN::Sync_Error`.
//!
//! See `docs/designs/v0.10.0-trace-topology.md` §"Implementation
//! phasing" for the per-source roadmap.

use std::path::Path;

use pcap_parser::traits::PcapReaderIterator;
use pcap_parser::{Block, Linktype, PcapBlockOwned, PcapError, PcapNGReader};

/// One captured L2 frame, distilled to the fields the v0.11.0
/// reconciler consumes. Higher-layer headers are ignored — this is
/// strictly L2 identity + timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// Source MAC address (Ethernet bytes 6..12).
    pub mac_src: [u8; 6],
    /// Destination MAC address (Ethernet bytes 0..6).
    pub mac_dst: [u8; 6],
    /// 802.1Q VLAN-ID (0..=4094) if the frame carried a 0x8100 tag.
    pub vlan_id: Option<u16>,
    /// 802.1Q PCP (0..=7) if the frame carried a 0x8100 tag.
    pub pcp: Option<u8>,
    /// Capture timestamp, normalised to nanoseconds since the
    /// Unix epoch using the IDB `ts_resol` option (defaults to 1µs
    /// per pcapng spec).
    pub timestamp_ns: u64,
}

/// Source of L2 frames captured at runtime — typically a PCAPNG file
/// recorded with `tcpdump`, `tshark`, or a TAP/SPAN port.
pub trait FrameSource {
    /// Iterate captured frames in capture order.
    fn frames(&mut self) -> Box<dyn Iterator<Item = Result<CapturedFrame, IngestError>> + '_>;
}

/// Source of LLDP topology snapshots — neighbor adjacency observed at
/// runtime via standard LLDP TLV exchange. Typical forms are
/// `lldpctl -f xml`, `lldpd`'s JSON dump, or per-frame extraction
/// from a PCAPNG that captured the LLDP multicast.
///
/// v0.10.x B-3 ships a concrete [`LldpJsonTopologySource`] backed by
/// `lldpctl -f json`. The trait surface itself is intentionally
/// minimal — it just exposes the parsed neighbor list — so that
/// alternate sources (LLDP TLVs extracted from a PCAPNG, or `lldpctl
/// -f xml`) can plug in without churning the surface.
pub trait TopologySource {
    /// Borrow the parsed list of LLDP-observed adjacencies.
    fn neighbors(&self) -> &[LldpNeighbor];
}

/// Source of switch configuration as declared by the deployed switch
/// — typically a Qcc YANG dump retrieved over NETCONF/RESTCONF or
/// `ieee802-dot1q-bridge` / `ieee802-dot1q-tsn-types`-shaped JSON.
///
/// v0.10.x B-4 ships a concrete [`QccYangSwitchConfigSource`] backed
/// by a JSON dump shaped per the `ieee802-dot1q-tsn-types` YANG module
/// (or a canonical equivalent extracted from `tc qdisc`). The trait
/// surface itself is intentionally minimal — it just exposes the
/// parsed per-port configuration list — so that alternate sources
/// (XML, vendor-specific REST APIs) can plug in without churning the
/// surface.
pub trait SwitchConfigSource {
    /// Borrow the parsed list of per-port TSN switch configurations.
    fn ports(&self) -> &[PortConfig];
}

/// One TAS gate-control-list entry — 802.1Qbv gate state for an
/// interval. The reconciler uses these to verify that the declared
/// `Spar_TSN::Schedule` matches what the switch is actually enforcing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOperation {
    /// 8-bit gate-state value; bit `i` open means traffic-class `i`
    /// may transmit during this interval.
    pub gate_states_value: u8,
    /// Interval duration in nanoseconds.
    pub time_interval_value: u64,
}

/// One Qci stream filter — links a stream-handle to a priority. The
/// reconciler matches these against the declared
/// `Spar_Identity::Stream_Handle` / `Spar_TSN::Priority` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamFilter {
    /// Stream handle (`null-stream-identification` etc.) the switch
    /// uses to identify this flow.
    pub stream_handle: u32,
    /// 3-bit priority spec the filter assigns / matches against.
    pub priority_spec: u8,
}

/// Per-port TSN switch configuration parsed from a Qcc YANG-shaped
/// JSON dump. Each field beyond the port name is optional because
/// real switch configs typically declare only the TSN features that
/// are actually enabled on a given interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortConfig {
    /// Local interface name on the switch (e.g. `swp1`, `Gi0/3`).
    pub port_name: String,
    /// 802.1Qbv Time-Aware Shaper gate-control list, in interval
    /// order. `None` if TAS is not enabled on this port.
    pub gate_control_list: Option<Vec<GateOperation>>,
    /// 802.1Qav Credit-Based Shaper bandwidth reservation, expressed
    /// as parts-per-thousand of port link speed (0..=1000). `None`
    /// if CBS is not enabled on this port.
    pub bandwidth_reservation_permille: Option<u32>,
    /// Maximum L2 frame size in bytes (typically 1518, or 9000 for
    /// jumbo frames). `None` if the dump did not declare it.
    pub max_frame_size: Option<u16>,
    /// 802.1Qci stream filters configured on this port. `None` if
    /// Qci is not enabled.
    pub streams: Option<Vec<StreamFilter>>,
}

/// Qcc YANG-shaped JSON-backed [`SwitchConfigSource`].
///
/// The expected JSON shape is
///
/// ```json
/// { "interfaces": [
///     { "name": "swp1",
///       "tsn": {
///         "gate-control-list": [
///           {"gate-states-value": 1, "time-interval-value": 500000}
///         ],
///         "bandwidth-reservation-permille": 750,
///         "max-frame-size": 1518,
///         "stream-filters": [
///           {"stream-handle": 42, "priority-spec": 5}
///         ]
///       }
///     }
/// ]}
/// ```
///
/// All four `tsn`-block fields are individually optional; an interface
/// without a `tsn` block (or with an empty one) yields a [`PortConfig`]
/// carrying just the name.
#[derive(Debug, Clone)]
pub struct QccYangSwitchConfigSource {
    ports: Vec<PortConfig>,
}

impl QccYangSwitchConfigSource {
    /// Open and parse a Qcc YANG-shaped JSON dump from `path`.
    pub fn open(path: &Path) -> Result<Self, IngestError> {
        let s = std::fs::read_to_string(path)?;
        Self::from_json_str(&s)
    }

    /// Parse a Qcc YANG-shaped JSON dump from an in-memory string.
    pub fn from_json_str(s: &str) -> Result<Self, IngestError> {
        let v: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| IngestError::MalformedQccJson(format!("invalid JSON: {e}")))?;

        let interfaces = v.get("interfaces").ok_or_else(|| {
            IngestError::MalformedQccJson("missing top-level `interfaces` key".to_string())
        })?;
        let interfaces = interfaces.as_array().ok_or_else(|| {
            IngestError::MalformedQccJson(format!(
                "`interfaces` must be an array, got {}",
                type_name(interfaces)
            ))
        })?;

        let mut ports = Vec::with_capacity(interfaces.len());
        for iface in interfaces {
            ports.push(parse_port(iface)?);
        }
        Ok(Self { ports })
    }
}

impl SwitchConfigSource for QccYangSwitchConfigSource {
    fn ports(&self) -> &[PortConfig] {
        &self.ports
    }
}

fn parse_port(iface: &serde_json::Value) -> Result<PortConfig, IngestError> {
    let port_name = iface
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedQccJson("interface entry missing `name` string".to_string())
        })?
        .to_string();

    let tsn = iface.get("tsn");

    let gate_control_list = match tsn.and_then(|t| t.get("gate-control-list")) {
        None => None,
        Some(serde_json::Value::Array(arr)) => {
            let mut gates = Vec::with_capacity(arr.len());
            for entry in arr {
                gates.push(parse_gate_operation(&port_name, entry)?);
            }
            Some(gates)
        }
        Some(other) => {
            return Err(IngestError::MalformedQccJson(format!(
                "interface `{port_name}` `gate-control-list` must be an array, got {}",
                type_name(other)
            )));
        }
    };

    let bandwidth_reservation_permille = match tsn
        .and_then(|t| t.get("bandwidth-reservation-permille"))
    {
        None => None,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| {
                IngestError::MalformedQccJson(format!(
                    "interface `{port_name}` `bandwidth-reservation-permille` must be a non-negative integer, got {}",
                    type_name(v)
                ))
            })?;
            if n > 1000 {
                return Err(IngestError::MalformedQccJson(format!(
                    "interface `{port_name}` `bandwidth-reservation-permille` = {n} is out of range (0..=1000)"
                )));
            }
            Some(n as u32)
        }
    };

    let max_frame_size = match tsn.and_then(|t| t.get("max-frame-size")) {
        None => None,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| {
                IngestError::MalformedQccJson(format!(
                    "interface `{port_name}` `max-frame-size` must be a non-negative integer, got {}",
                    type_name(v)
                ))
            })?;
            if n > u64::from(u16::MAX) {
                return Err(IngestError::MalformedQccJson(format!(
                    "interface `{port_name}` `max-frame-size` = {n} exceeds u16::MAX"
                )));
            }
            Some(n as u16)
        }
    };

    let streams = match tsn.and_then(|t| t.get("stream-filters")) {
        None => None,
        Some(serde_json::Value::Array(arr)) => {
            let mut filters = Vec::with_capacity(arr.len());
            for entry in arr {
                filters.push(parse_stream_filter(&port_name, entry)?);
            }
            Some(filters)
        }
        Some(other) => {
            return Err(IngestError::MalformedQccJson(format!(
                "interface `{port_name}` `stream-filters` must be an array, got {}",
                type_name(other)
            )));
        }
    };

    Ok(PortConfig {
        port_name,
        gate_control_list,
        bandwidth_reservation_permille,
        max_frame_size,
        streams,
    })
}

fn parse_gate_operation(
    port_name: &str,
    entry: &serde_json::Value,
) -> Result<GateOperation, IngestError> {
    let gate_states_raw = entry
        .get("gate-states-value")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            IngestError::MalformedQccJson(format!(
                "interface `{port_name}` gate entry missing `gate-states-value` u64"
            ))
        })?;
    if gate_states_raw > u64::from(u8::MAX) {
        return Err(IngestError::MalformedQccJson(format!(
            "interface `{port_name}` `gate-states-value` = {gate_states_raw} exceeds u8::MAX"
        )));
    }
    let time_interval_value = entry
        .get("time-interval-value")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            IngestError::MalformedQccJson(format!(
                "interface `{port_name}` gate entry missing `time-interval-value` u64"
            ))
        })?;
    Ok(GateOperation {
        gate_states_value: gate_states_raw as u8,
        time_interval_value,
    })
}

fn parse_stream_filter(
    port_name: &str,
    entry: &serde_json::Value,
) -> Result<StreamFilter, IngestError> {
    let stream_handle_raw = entry
        .get("stream-handle")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            IngestError::MalformedQccJson(format!(
                "interface `{port_name}` stream-filter missing `stream-handle` u64"
            ))
        })?;
    if stream_handle_raw > u64::from(u32::MAX) {
        return Err(IngestError::MalformedQccJson(format!(
            "interface `{port_name}` `stream-handle` = {stream_handle_raw} exceeds u32::MAX"
        )));
    }
    let priority_raw = entry
        .get("priority-spec")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            IngestError::MalformedQccJson(format!(
                "interface `{port_name}` stream-filter missing `priority-spec` u64"
            ))
        })?;
    if priority_raw > u64::from(u8::MAX) {
        return Err(IngestError::MalformedQccJson(format!(
            "interface `{port_name}` `priority-spec` = {priority_raw} exceeds u8::MAX"
        )));
    }
    Ok(StreamFilter {
        stream_handle: stream_handle_raw as u32,
        priority_spec: priority_raw as u8,
    })
}

/// One observed gPTP sync-error sample for a single port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtpSample {
    /// Sample timestamp in ns since Unix epoch (as reported by the
    /// gPTP stack / pmc client).
    pub timestamp_ns: u64,
    /// Magnitude of the sync error in nanoseconds (callers must
    /// pre-`abs()` signed offsets before serializing).
    pub sync_error_ns: u64,
}

/// Per-port gPTP-sample stream parsed from a JSON dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtpPortObservation {
    /// Local interface name (e.g. `eth0`).
    pub name: String,
    /// Time-ordered list of sync-error samples for this port. May be
    /// empty when the dump recorded the port but observed no samples.
    pub samples: Vec<PtpSample>,
}

/// Source of gPTP / IEEE 802.1AS synchronization-error observations
/// over the capture window — typically `ptp4l` summary logs, `pmc`
/// JSON dumps, or CTF events emitted by a Linux/Zephyr gPTP stack.
///
/// v0.10.x B-5 ships a concrete [`GptpJsonPtpTimeSource`] backed by
/// a JSON dump shaped per the design doc's §"Input artefact set" gPTP
/// entry. The reconciler uses these readings to evaluate the
/// `GptpOutOfBudget` check against `Spar_TSN::Sync_Error`.
pub trait PtpTimeSource {
    /// Grandmaster clock identity (e.g. `"00:1b:21:ff:fe:01:02:03"`)
    /// if the dump recorded it.
    fn grandmaster(&self) -> Option<&str>;
    /// PTP domain number (typically 0 or 20 for 802.1AS) if recorded.
    fn domain(&self) -> Option<u8>;
    /// Borrow the per-port sample streams.
    fn ports(&self) -> &[PtpPortObservation];
}

/// One LLDP-observed adjacency. The local-port end is identified by
/// the local interface name (typically `eth0`/`Gi0/1`/etc.); the
/// remote end carries a typed chassis-id and port-id pair, plus an
/// optional system-name hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LldpNeighbor {
    /// Local interface name as reported by lldpd (e.g. `eth0`).
    pub local_port: String,
    /// Remote chassis-id value (e.g. `aa:bb:cc:dd:ee:01`). Sourced
    /// from `chassis.<key>.id.value` — never the chassis-block child
    /// key, which is just lldpd's chosen handle.
    pub remote_chassis_id: String,
    /// Remote chassis-id type (`mac`, `ifname`, `local`, …) per
    /// IEEE 802.1AB-2016 §8.5.2.2.
    pub remote_chassis_id_type: String,
    /// Remote port-id value as advertised by the neighbor.
    pub remote_port_id: String,
    /// Remote port-id type (`ifname`, `mac`, `local`, …) per
    /// IEEE 802.1AB-2016 §8.5.3.2.
    pub remote_port_id_type: String,
    /// Optional remote system-name (`chassis.<key>.name`).
    pub remote_system_name: Option<String>,
}

/// `lldpctl -f json`-backed [`TopologySource`].
///
/// `lldpd` emits a JSON tree shaped like
///
/// ```json
/// { "lldp": { "interface": [ { "name": "...", "chassis": {...},
///   "port": {...} }, ... ] } }
/// ```
///
/// Older lldpd versions emit `"interface"` as a single object instead
/// of an array when there is exactly one neighbor; this parser
/// handles both shapes.
#[derive(Debug, Clone)]
pub struct LldpJsonTopologySource {
    neighbors: Vec<LldpNeighbor>,
}

impl LldpJsonTopologySource {
    /// Open and parse an `lldpctl -f json` dump from `path`.
    pub fn open(path: &Path) -> Result<Self, IngestError> {
        let bytes = std::fs::read(path).map_err(IngestError::Io)?;
        let s = std::str::from_utf8(&bytes)
            .map_err(|e| IngestError::MalformedLldpJson(format!("non-UTF-8 input: {e}")))?;
        Self::from_json_str(s)
    }

    /// Parse an `lldpctl -f json` dump from an in-memory string.
    pub fn from_json_str(s: &str) -> Result<Self, IngestError> {
        let v: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| IngestError::MalformedLldpJson(format!("invalid JSON: {e}")))?;

        let lldp = v.get("lldp").ok_or_else(|| {
            IngestError::MalformedLldpJson("missing top-level `lldp` key".to_string())
        })?;

        // `interface` may be absent (no neighbors observed yet),
        // a single object (older lldpd, single neighbor), or an
        // array (multi-neighbor case, or modern lldpd always).
        let interfaces: Vec<&serde_json::Value> = match lldp.get("interface") {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr.iter().collect(),
            Some(obj @ serde_json::Value::Object(_)) => vec![obj],
            Some(other) => {
                return Err(IngestError::MalformedLldpJson(format!(
                    "`lldp.interface` must be array or object, got {}",
                    type_name(other)
                )));
            }
        };

        let mut neighbors = Vec::with_capacity(interfaces.len());
        for iface in interfaces {
            neighbors.push(parse_interface(iface)?);
        }
        Ok(Self { neighbors })
    }
}

impl TopologySource for LldpJsonTopologySource {
    fn neighbors(&self) -> &[LldpNeighbor] {
        &self.neighbors
    }
}

fn parse_interface(iface: &serde_json::Value) -> Result<LldpNeighbor, IngestError> {
    let local_port = iface
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedLldpJson("interface entry missing `name` string".to_string())
        })?
        .to_string();

    // `chassis` is a dict whose single child key is the system-name
    // or MAC lldpd picked as a handle. Neither the key itself nor
    // its presence is authoritative for the chassis-id — always read
    // `id.value` from the inner block.
    let chassis_block = iface.get("chassis").ok_or_else(|| {
        IngestError::MalformedLldpJson(format!("interface `{local_port}` missing `chassis` block"))
    })?;

    let chassis_inner = chassis_block
        .as_object()
        .and_then(|m| m.values().next())
        // Some lldpd builds emit chassis directly without the
        // by-name wrapper — accept that shape too.
        .or(Some(chassis_block))
        .ok_or_else(|| {
            IngestError::MalformedLldpJson(format!(
                "interface `{local_port}` chassis block is empty"
            ))
        })?;

    // If chassis_block was already the inner shape (has `id`
    // directly), the .values().next() on it picked up the `id`
    // value rather than a wrapper — fall back to the outer block.
    let chassis_inner = if chassis_inner.get("id").is_some() {
        chassis_inner
    } else {
        chassis_block
    };

    let chassis_id_block = chassis_inner.get("id").ok_or_else(|| {
        IngestError::MalformedLldpJson(format!(
            "interface `{local_port}` chassis block missing `id`"
        ))
    })?;
    let remote_chassis_id_type = chassis_id_block
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedLldpJson(format!(
                "interface `{local_port}` chassis.id missing `type`"
            ))
        })?
        .to_string();
    let remote_chassis_id = chassis_id_block
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedLldpJson(format!(
                "interface `{local_port}` chassis.id missing `value`"
            ))
        })?
        .to_string();

    let remote_system_name = chassis_inner
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let port_block = iface.get("port").ok_or_else(|| {
        IngestError::MalformedLldpJson(format!("interface `{local_port}` missing `port` block"))
    })?;
    let port_id_block = port_block.get("id").ok_or_else(|| {
        IngestError::MalformedLldpJson(format!("interface `{local_port}` port block missing `id`"))
    })?;
    let remote_port_id_type = port_id_block
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedLldpJson(format!(
                "interface `{local_port}` port.id missing `type`"
            ))
        })?
        .to_string();
    let remote_port_id = port_id_block
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedLldpJson(format!(
                "interface `{local_port}` port.id missing `value`"
            ))
        })?
        .to_string();

    Ok(LldpNeighbor {
        local_port,
        remote_chassis_id,
        remote_chassis_id_type,
        remote_port_id,
        remote_port_id_type,
        remote_system_name,
    })
}

fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// JSON-backed [`PtpTimeSource`] carrying per-port gPTP sync-error
/// samples.
///
/// The expected JSON shape is:
///
/// ```json
/// {
///   "gptp": {
///     "grandmaster": "00:1b:21:ff:fe:01:02:03",
///     "domain": 0,
///     "ports": [
///       {
///         "name": "eth0",
///         "samples": [
///           {"timestamp_ns": 1700000000000000000, "sync_error_ns": 250}
///         ]
///       }
///     ]
///   }
/// }
/// ```
///
/// `grandmaster` and `domain` are optional; `ports` is required (but
/// may be empty). Each `samples[].sync_error_ns` is parsed as a
/// non-negative `u64` — callers must pre-`abs()` signed offsets before
/// serializing, and negative values are rejected with
/// [`IngestError::MalformedPtpJson`].
#[derive(Debug, Clone)]
pub struct GptpJsonPtpTimeSource {
    grandmaster: Option<String>,
    domain: Option<u8>,
    ports: Vec<PtpPortObservation>,
}

impl GptpJsonPtpTimeSource {
    /// Open and parse a gPTP JSON dump from `path`.
    pub fn open(path: &Path) -> Result<Self, IngestError> {
        let s = std::fs::read_to_string(path)?;
        Self::from_json_str(&s)
    }

    /// Parse a gPTP JSON dump from an in-memory string.
    pub fn from_json_str(s: &str) -> Result<Self, IngestError> {
        let v: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| IngestError::MalformedPtpJson(format!("invalid JSON: {e}")))?;

        let gptp = v.get("gptp").ok_or_else(|| {
            IngestError::MalformedPtpJson("missing top-level `gptp` key".to_string())
        })?;

        let grandmaster = gptp
            .get("grandmaster")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);

        let domain = match gptp.get("domain") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => {
                let n = v.as_u64().ok_or_else(|| {
                    IngestError::MalformedPtpJson(format!(
                        "`gptp.domain` must be a non-negative integer, got {}",
                        type_name(v)
                    ))
                })?;
                if n > u64::from(u8::MAX) {
                    return Err(IngestError::MalformedPtpJson(format!(
                        "`gptp.domain` {n} exceeds u8 range"
                    )));
                }
                Some(n as u8)
            }
        };

        let ports_value = gptp.get("ports").ok_or_else(|| {
            IngestError::MalformedPtpJson("missing `gptp.ports` array".to_string())
        })?;
        let ports_arr = ports_value.as_array().ok_or_else(|| {
            IngestError::MalformedPtpJson(format!(
                "`gptp.ports` must be an array, got {}",
                type_name(ports_value)
            ))
        })?;

        let mut ports = Vec::with_capacity(ports_arr.len());
        for (idx, port) in ports_arr.iter().enumerate() {
            ports.push(parse_ptp_port(port, idx)?);
        }

        Ok(Self {
            grandmaster,
            domain,
            ports,
        })
    }
}

impl PtpTimeSource for GptpJsonPtpTimeSource {
    fn grandmaster(&self) -> Option<&str> {
        self.grandmaster.as_deref()
    }
    fn domain(&self) -> Option<u8> {
        self.domain
    }
    fn ports(&self) -> &[PtpPortObservation] {
        &self.ports
    }
}

fn parse_ptp_port(port: &serde_json::Value, idx: usize) -> Result<PtpPortObservation, IngestError> {
    let name = port
        .get("name")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            IngestError::MalformedPtpJson(format!("`gptp.ports[{idx}]` missing `name` string"))
        })?
        .to_string();

    let samples_value = port.get("samples").ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{idx}]` ({name}) missing `samples` array"
        ))
    })?;
    let samples_arr = samples_value.as_array().ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{idx}]` ({name}) `samples` must be an array, got {}",
            type_name(samples_value)
        ))
    })?;

    let mut samples = Vec::with_capacity(samples_arr.len());
    for (s_idx, sample) in samples_arr.iter().enumerate() {
        samples.push(parse_ptp_sample(sample, &name, s_idx)?);
    }

    Ok(PtpPortObservation { name, samples })
}

fn parse_ptp_sample(
    sample: &serde_json::Value,
    port_name: &str,
    s_idx: usize,
) -> Result<PtpSample, IngestError> {
    let ts_value = sample.get("timestamp_ns").ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{port_name}].samples[{s_idx}]` missing `timestamp_ns`"
        ))
    })?;
    let timestamp_ns = ts_value.as_u64().ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{port_name}].samples[{s_idx}].timestamp_ns` must be a \
             non-negative integer, got {}",
            type_name(ts_value)
        ))
    })?;

    let err_value = sample.get("sync_error_ns").ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{port_name}].samples[{s_idx}]` missing `sync_error_ns`"
        ))
    })?;
    // Reject negative / non-integer values explicitly — schema demands
    // pre-abs'd unsigned magnitudes. `as_u64()` returns None for any
    // signed-negative or floating value.
    let sync_error_ns = err_value.as_u64().ok_or_else(|| {
        IngestError::MalformedPtpJson(format!(
            "`gptp.ports[{port_name}].samples[{s_idx}].sync_error_ns` must be a \
             non-negative integer (pre-abs() signed offsets before serializing), got {}",
            describe_number_or_type(err_value)
        ))
    })?;

    Ok(PtpSample {
        timestamp_ns,
        sync_error_ns,
    })
}

/// Helper for error messages: distinguishes a signed-negative number
/// from other type mismatches so the user gets a useful hint.
fn describe_number_or_type(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                format!("signed integer {i}")
            } else if let Some(f) = n.as_f64() {
                format!("floating-point {f}")
            } else {
                format!("number {n}")
            }
        }
        other => type_name(other).to_string(),
    }
}

/// Errors surfaced from a runtime-artefact parser.
///
/// v0.10.0 shipped only `Unimplemented`; v0.10.x parsers extend this
/// enum additively with concrete I/O / format-decode kinds. The
/// `Unimplemented` variant is preserved for the placeholder trait
/// `open()` calls that haven't been replaced yet (Qcc YANG, gPTP).
#[derive(Debug)]
pub enum IngestError {
    /// Underlying I/O error opening or reading the artefact file.
    Io(std::io::Error),
    /// Captured frame is shorter than a full L2 header (or shorter
    /// than the 16 bytes required when an 802.1Q tag is present).
    Truncated,
    /// pcap-parser reported a malformed pcapng block / record.
    MalformedPcapng(String),
    /// pcapng link type other than Ethernet (LINKTYPE_ETHERNET = 1).
    UnsupportedLinkType(i32),
    /// LLDP JSON dump did not match the `lldpctl -f json` schema.
    MalformedLldpJson(String),
    /// gPTP JSON dump did not match the expected schema.
    MalformedPtpJson(String),
    /// The requested parser surface is not implemented in this
    /// build of spar-trace-topology. v0.10.0 returned this from
    /// every `open` call; v0.10.x parsers replace it with concrete
    /// kinds as they land.
    Unimplemented,
    /// Qcc YANG-shaped JSON did not match the expected schema.
    MalformedQccJson(String),
}

impl core::fmt::Display for IngestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Truncated => write!(f, "captured frame is shorter than a full L2 header"),
            Self::MalformedPcapng(msg) => write!(f, "malformed pcapng: {msg}"),
            Self::UnsupportedLinkType(lt) => write!(
                f,
                "unsupported pcapng link type {lt}; only Ethernet (1) is supported"
            ),
            Self::MalformedLldpJson(msg) => {
                write!(f, "malformed lldpctl JSON: {msg}")
            }
            Self::MalformedPtpJson(msg) => {
                write!(f, "malformed gPTP JSON: {msg}")
            }
            Self::Unimplemented => write!(
                f,
                "parser not implemented in v0.10.0 foundation; see \
                 docs/designs/v0.10.0-trace-topology.md"
            ),
            Self::MalformedQccJson(msg) => {
                write!(f, "malformed Qcc YANG JSON: {msg}")
            }
        }
    }
}

impl std::error::Error for IngestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for IngestError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// PCAPNG-backed [`FrameSource`] using the `pcap-parser` crate.
///
/// Reads the entire `.pcapng` into memory at `open()` time — pcapng
/// captures from real deployments are bounded artefacts (typically
/// tens to hundreds of MB), not pipes, so the simpler in-memory parse
/// avoids the streaming-`consume()` lifetime gymnastics that
/// `PcapNGReader` would otherwise require us to fight.
#[derive(Debug)]
pub struct PcapngFrameSource {
    /// Raw pcapng bytes — owned so iteration can hold borrows
    /// without juggling lifetimes against the source struct.
    bytes: Vec<u8>,
    /// Pre-validated link type from the first IDB. The current
    /// implementation only accepts captures whose first IDB declares
    /// LINKTYPE_ETHERNET; multi-IDB or per-frame link-type variation
    /// is out-of-scope for v0.10.x.
    linktype: Linktype,
}

impl PcapngFrameSource {
    /// Open a pcapng file and validate the first Interface Description
    /// Block declares `LINKTYPE_ETHERNET`.
    pub fn open(path: &Path) -> Result<Self, IngestError> {
        let bytes = std::fs::read(path)?;
        let linktype = first_idb_linktype(&bytes)?;
        if linktype != Linktype::ETHERNET {
            return Err(IngestError::UnsupportedLinkType(linktype.0));
        }
        Ok(Self { bytes, linktype })
    }
}

impl FrameSource for PcapngFrameSource {
    fn frames(&mut self) -> Box<dyn Iterator<Item = Result<CapturedFrame, IngestError>> + '_> {
        Box::new(PcapngFrameIter::new(&self.bytes, self.linktype))
    }
}

/// Iterator over captured frames in a pcapng buffer.
struct PcapngFrameIter<'a> {
    reader: Option<PcapNGReader<&'a [u8]>>,
    /// IDB ts_resolution (fractions-of-a-second; 1_000_000 = µs).
    ts_resolution: u64,
    /// Set true once we surface a fatal stream-level error so we
    /// don't keep retrying the underlying parser.
    done: bool,
    /// Total input length — `PcapNGReader::reader_exhausted()` only
    /// flips when the backing reader yields zero bytes on a refill,
    /// which never happens for a `&[u8]` source that was fully
    /// preloaded. Comparing against `position()` lets us distinguish
    /// a truly-truncated tail block from clean EOF.
    total_len: usize,
    /// Pre-validated link type from `open()`. We ignore mid-stream
    /// IDB changes for v0.10.x.
    _linktype: Linktype,
}

impl<'a> PcapngFrameIter<'a> {
    fn new(bytes: &'a [u8], linktype: Linktype) -> Self {
        let reader = PcapNGReader::new(bytes.len().max(65536), bytes).ok();
        let done = reader.is_none();
        Self {
            reader,
            ts_resolution: DEFAULT_TS_RESOLUTION,
            done,
            total_len: bytes.len(),
            _linktype: linktype,
        }
    }
}

/// Default IDB ts_resolution per pcapng spec §4.2 (`if_tsresol = 6`,
/// i.e. 10^-6 seconds = µs). Means 1_000_000 ticks per second.
const DEFAULT_TS_RESOLUTION: u64 = 1_000_000;

impl Iterator for PcapngFrameIter<'_> {
    type Item = Result<CapturedFrame, IngestError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let reader = self.reader.as_mut()?;
        loop {
            match reader.next() {
                Ok((offset, block)) => {
                    let outcome = match block {
                        PcapBlockOwned::NG(Block::SectionHeader(_)) => Step::Continue,
                        PcapBlockOwned::NG(Block::InterfaceDescription(idb)) => {
                            if let Some(res) = idb.ts_resolution() {
                                self.ts_resolution = res;
                            } else {
                                self.ts_resolution = DEFAULT_TS_RESOLUTION;
                            }
                            Step::Continue
                        }
                        PcapBlockOwned::NG(Block::EnhancedPacket(epb)) => {
                            match decode_ethernet(epb.data) {
                                Ok(eth) => {
                                    let timestamp_ns = epb_timestamp_ns(
                                        epb.ts_high,
                                        epb.ts_low,
                                        self.ts_resolution,
                                    );
                                    Step::Yield(Ok(CapturedFrame {
                                        mac_src: eth.mac_src,
                                        mac_dst: eth.mac_dst,
                                        vlan_id: eth.vlan_id,
                                        pcp: eth.pcp,
                                        timestamp_ns,
                                    }))
                                }
                                Err(e) => Step::Yield(Err(e)),
                            }
                        }
                        _ => Step::Continue,
                    };
                    reader.consume(offset);
                    match outcome {
                        Step::Continue => continue,
                        Step::Yield(item) => return Some(item),
                    }
                }
                Err(PcapError::Eof) => {
                    self.done = true;
                    return None;
                }
                Err(PcapError::Incomplete(_)) => {
                    self.done = true;
                    if reader.position() >= self.total_len {
                        return None;
                    }
                    return Some(Err(IngestError::MalformedPcapng(
                        "incomplete trailing block".to_string(),
                    )));
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(IngestError::MalformedPcapng(format!("{e:?}"))));
                }
            }
        }
    }
}

enum Step {
    Continue,
    Yield(Result<CapturedFrame, IngestError>),
}

/// L2 fields extracted from a single Ethernet frame's header.
struct EthHeader {
    mac_dst: [u8; 6],
    mac_src: [u8; 6],
    vlan_id: Option<u16>,
    pcp: Option<u8>,
}

/// Decode the L2 header of an Ethernet frame.
fn decode_ethernet(data: &[u8]) -> Result<EthHeader, IngestError> {
    if data.len() < 14 {
        return Err(IngestError::Truncated);
    }
    let mut mac_dst = [0u8; 6];
    let mut mac_src = [0u8; 6];
    mac_dst.copy_from_slice(&data[0..6]);
    mac_src.copy_from_slice(&data[6..12]);
    let ethertype = u16::from_be_bytes([data[12], data[13]]);
    if ethertype == 0x8100 {
        if data.len() < 16 {
            return Err(IngestError::Truncated);
        }
        let tci = u16::from_be_bytes([data[14], data[15]]);
        let pcp = ((tci >> 13) & 0x7) as u8;
        let vlan_id = tci & 0x0FFF;
        Ok(EthHeader {
            mac_dst,
            mac_src,
            vlan_id: Some(vlan_id),
            pcp: Some(pcp),
        })
    } else {
        Ok(EthHeader {
            mac_dst,
            mac_src,
            vlan_id: None,
            pcp: None,
        })
    }
}

/// Convert an EPB ts_high/ts_low pair to ns-since-Unix-epoch using
/// the IDB's ts_resolution (fractions-of-a-second).
fn epb_timestamp_ns(ts_high: u32, ts_low: u32, resolution: u64) -> u64 {
    let ticks = (u64::from(ts_high) << 32) | u64::from(ts_low);
    if resolution == 0 {
        return 0;
    }
    ticks
        .saturating_mul(1_000_000_000)
        .saturating_div(resolution)
}

/// Pull the link type from the first Interface Description Block.
fn first_idb_linktype(bytes: &[u8]) -> Result<Linktype, IngestError> {
    let mut reader = PcapNGReader::new(bytes.len().max(65536), bytes)
        .map_err(|e| IngestError::MalformedPcapng(format!("{e:?}")))?;
    loop {
        match reader.next() {
            Ok((offset, block)) => {
                if let PcapBlockOwned::NG(Block::InterfaceDescription(idb)) = block {
                    let lt = idb.linktype;
                    return Ok(lt);
                }
                reader.consume(offset);
            }
            Err(PcapError::Eof) => {
                return Err(IngestError::MalformedPcapng(
                    "no InterfaceDescriptionBlock found".to_string(),
                ));
            }
            Err(PcapError::Incomplete(_)) => {
                return Err(IngestError::MalformedPcapng(
                    "incomplete pcapng prefix".to_string(),
                ));
            }
            Err(e) => {
                return Err(IngestError::MalformedPcapng(format!("{e:?}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANONICAL_SINGLE: &str = r#"
    {
      "lldp": {
        "interface": [
          {
            "name": "eth0",
            "via": "LLDP",
            "chassis": {
              "switch-1": {
                "id": {"type": "mac", "value": "aa:bb:cc:dd:ee:01"},
                "name": "switch-1.local",
                "descr": "TSN switch"
              }
            },
            "port": {
              "id": {"type": "ifname", "value": "Gi0/3"},
              "descr": "Eth port 3"
            }
          }
        ]
      }
    }
    "#;

    const CANONICAL_OBJECT_FORM: &str = r#"
    {
      "lldp": {
        "interface": {
          "name": "eth0",
          "via": "LLDP",
          "chassis": {
            "switch-1": {
              "id": {"type": "mac", "value": "aa:bb:cc:dd:ee:01"},
              "name": "switch-1.local",
              "descr": "TSN switch"
            }
          },
          "port": {
            "id": {"type": "ifname", "value": "Gi0/3"},
            "descr": "Eth port 3"
          }
        }
      }
    }
    "#;

    #[test]
    fn lldp_single_neighbor_array_form() {
        let src = LldpJsonTopologySource::from_json_str(CANONICAL_SINGLE).expect("parse");
        let n = src.neighbors();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].local_port, "eth0");
        assert_eq!(n[0].remote_chassis_id, "aa:bb:cc:dd:ee:01");
        assert_eq!(n[0].remote_chassis_id_type, "mac");
        assert_eq!(n[0].remote_port_id, "Gi0/3");
        assert_eq!(n[0].remote_port_id_type, "ifname");
        assert_eq!(n[0].remote_system_name, Some("switch-1.local".to_string()));
    }

    #[test]
    fn lldp_single_neighbor_object_form() {
        let src_array =
            LldpJsonTopologySource::from_json_str(CANONICAL_SINGLE).expect("parse array");
        let src_obj =
            LldpJsonTopologySource::from_json_str(CANONICAL_OBJECT_FORM).expect("parse object");
        // Both shapes must yield the same neighbor list.
        assert_eq!(src_array.neighbors(), src_obj.neighbors());
        assert_eq!(src_obj.neighbors().len(), 1);
    }

    #[test]
    fn lldp_multiple_neighbors() {
        let json = r#"
        {
          "lldp": {
            "interface": [
              {
                "name": "eth0",
                "chassis": {
                  "switch-1": {
                    "id": {"type": "mac", "value": "aa:bb:cc:dd:ee:01"},
                    "name": "switch-1.local"
                  }
                },
                "port": {"id": {"type": "ifname", "value": "Gi0/3"}}
              },
              {
                "name": "eth1",
                "chassis": {
                  "anon": {
                    "id": {"type": "mac", "value": "11:22:33:44:55:66"}
                  }
                },
                "port": {"id": {"type": "mac", "value": "11:22:33:44:55:77"}}
              }
            ]
          }
        }
        "#;
        let src = LldpJsonTopologySource::from_json_str(json).expect("parse");
        let n = src.neighbors();
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].local_port, "eth0");
        assert_eq!(n[0].remote_system_name, Some("switch-1.local".to_string()));
        assert_eq!(n[1].local_port, "eth1");
        assert_eq!(n[1].remote_system_name, None);
        assert_eq!(n[1].remote_chassis_id, "11:22:33:44:55:66");
        assert_eq!(n[1].remote_port_id_type, "mac");
    }

    #[test]
    fn lldp_chassis_id_types_preserved() {
        let json_mac = r#"
        {"lldp": {"interface": [{
          "name": "eth0",
          "chassis": {"a": {"id": {"type": "mac", "value": "aa:bb:cc:dd:ee:01"}}},
          "port": {"id": {"type": "ifname", "value": "Gi0/1"}}
        }]}}"#;
        let json_local = r#"
        {"lldp": {"interface": [{
          "name": "eth0",
          "chassis": {"a": {"id": {"type": "local", "value": "chassis-handle-7"}}},
          "port": {"id": {"type": "local", "value": "port-handle-3"}}
        }]}}"#;
        let mac = LldpJsonTopologySource::from_json_str(json_mac).expect("mac");
        let loc = LldpJsonTopologySource::from_json_str(json_local).expect("local");
        assert_eq!(mac.neighbors()[0].remote_chassis_id_type, "mac");
        assert_eq!(loc.neighbors()[0].remote_chassis_id_type, "local");
        assert_eq!(loc.neighbors()[0].remote_chassis_id, "chassis-handle-7");
        assert_eq!(loc.neighbors()[0].remote_port_id_type, "local");
    }

    #[test]
    fn lldp_malformed_yields_error() {
        // No `lldp` root.
        let json = r#"{"not_lldp": {}}"#;
        match LldpJsonTopologySource::from_json_str(json) {
            Err(IngestError::MalformedLldpJson(_)) => {}
            other => panic!("expected MalformedLldpJson, got {other:?}"),
        }
    }

    #[test]
    fn lldp_open_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lldp.json");
        std::fs::write(&path, CANONICAL_SINGLE).expect("write tempfile");
        let src = LldpJsonTopologySource::open(&path).expect("open");
        assert_eq!(src.neighbors().len(), 1);
        assert_eq!(src.neighbors()[0].local_port, "eth0");
    }

    // ── PCAPNG tests ────────────────────────────────────────────────
    use std::io::Write as _;

    /// Hand-build a minimal valid pcapng buffer:
    ///   SHB + IDB(linktype, if_tsresol) + EPB(ts_high, ts_low, frame_data).
    fn build_pcapng(
        linktype_id: u16,
        if_tsresol: u8,
        ts_high: u32,
        ts_low: u32,
        frame: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();

        // Section Header Block (block_type 0x0A0D0D0A).
        let shb_total = 4 + 4 + 4 + 4 + 8 + 4;
        out.extend_from_slice(&0x0A0D_0D0A_u32.to_le_bytes());
        out.extend_from_slice(&(shb_total as u32).to_le_bytes());
        out.extend_from_slice(&0x1A2B_3C4D_u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(-1_i64).to_le_bytes());
        out.extend_from_slice(&(shb_total as u32).to_le_bytes());

        // Interface Description Block (block_type 0x00000001).
        let mut idb_options: Vec<u8> = Vec::new();
        idb_options.extend_from_slice(&9u16.to_le_bytes());
        idb_options.extend_from_slice(&1u16.to_le_bytes());
        idb_options.push(if_tsresol);
        idb_options.extend_from_slice(&[0u8, 0, 0]);
        idb_options.extend_from_slice(&0u16.to_le_bytes());
        idb_options.extend_from_slice(&0u16.to_le_bytes());

        let idb_total = 4 + 4 + 2 + 2 + 4 + idb_options.len() + 4;
        out.extend_from_slice(&0x0000_0001_u32.to_le_bytes());
        out.extend_from_slice(&(idb_total as u32).to_le_bytes());
        out.extend_from_slice(&linktype_id.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&65535u32.to_le_bytes());
        out.extend_from_slice(&idb_options);
        out.extend_from_slice(&(idb_total as u32).to_le_bytes());

        // Enhanced Packet Block (block_type 0x00000006).
        let pad_len = (4 - (frame.len() % 4)) % 4;
        let epb_data_padded_len = frame.len() + pad_len;
        let epb_total = 4 + 4 + 4 + 4 + 4 + 4 + 4 + epb_data_padded_len + 4;
        out.extend_from_slice(&0x0000_0006_u32.to_le_bytes());
        out.extend_from_slice(&(epb_total as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&ts_high.to_le_bytes());
        out.extend_from_slice(&ts_low.to_le_bytes());
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        out.extend_from_slice(frame);
        out.extend_from_slice(&vec![0u8; pad_len]);
        out.extend_from_slice(&(epb_total as u32).to_le_bytes());

        out
    }

    fn write_to_tempfile(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    fn untagged_eth(dst: [u8; 6], src: [u8; 6], ethertype: u16) -> Vec<u8> {
        let mut v = Vec::with_capacity(14);
        v.extend_from_slice(&dst);
        v.extend_from_slice(&src);
        v.extend_from_slice(&ethertype.to_be_bytes());
        v
    }

    fn tagged_eth(
        dst: [u8; 6],
        src: [u8; 6],
        vlan_id: u16,
        pcp: u8,
        inner_ethertype: u16,
    ) -> Vec<u8> {
        let mut v = Vec::with_capacity(16);
        v.extend_from_slice(&dst);
        v.extend_from_slice(&src);
        v.extend_from_slice(&0x8100_u16.to_be_bytes());
        let tci = ((u16::from(pcp) & 0x7) << 13) | (vlan_id & 0x0FFF);
        v.extend_from_slice(&tci.to_be_bytes());
        v.extend_from_slice(&inner_ethertype.to_be_bytes());
        v
    }

    #[test]
    fn pcapng_roundtrip_untagged_frame() {
        let dst = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01];
        let src = [0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x02];
        let frame = untagged_eth(dst, src, 0x0800);
        let ts_high = 1u32;
        let ts_low = 100u32;
        let bytes = build_pcapng(1, 6, ts_high, ts_low, &frame);
        let f = write_to_tempfile(&bytes);

        let mut src_iter = PcapngFrameSource::open(f.path()).unwrap();
        let frames: Vec<_> = src_iter.frames().collect();
        assert_eq!(frames.len(), 1);
        let cf = frames.into_iter().next().unwrap().unwrap();
        assert_eq!(cf.mac_dst, dst);
        assert_eq!(cf.mac_src, src);
        assert_eq!(cf.vlan_id, None);
        assert_eq!(cf.pcp, None);
        let expected_ticks = (u64::from(ts_high) << 32) | u64::from(ts_low);
        assert_eq!(cf.timestamp_ns, expected_ticks * 1000);
    }

    #[test]
    fn pcapng_roundtrip_8021q_tagged() {
        let dst = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let src = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let frame = tagged_eth(dst, src, 100, 5, 0x0800);
        let bytes = build_pcapng(1, 6, 0, 12345, &frame);
        let f = write_to_tempfile(&bytes);

        let mut s = PcapngFrameSource::open(f.path()).unwrap();
        let frames: Vec<_> = s.frames().collect();
        assert_eq!(frames.len(), 1);
        let cf = frames.into_iter().next().unwrap().unwrap();
        assert_eq!(cf.mac_dst, dst);
        assert_eq!(cf.mac_src, src);
        assert_eq!(cf.vlan_id, Some(100));
        assert_eq!(cf.pcp, Some(5));
        assert_eq!(cf.timestamp_ns, 12345 * 1000);
    }

    #[test]
    fn pcapng_truncated_frame_yields_error() {
        let frame = vec![0u8; 8];
        let bytes = build_pcapng(1, 6, 0, 0, &frame);
        let f = write_to_tempfile(&bytes);

        let mut s = PcapngFrameSource::open(f.path()).unwrap();
        let mut it = s.frames();
        let first = it.next().expect("expected one item");
        match first {
            Err(IngestError::Truncated) => {}
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn pcapng_unsupported_linktype_errors_at_open() {
        let frame = vec![0u8; 20];
        let bytes = build_pcapng(101, 6, 0, 0, &frame);
        let f = write_to_tempfile(&bytes);

        match PcapngFrameSource::open(f.path()) {
            Err(IngestError::UnsupportedLinkType(101)) => {}
            other => panic!("expected UnsupportedLinkType(101), got {other:?}"),
        }
    }

    #[test]
    fn pcapng_ts_resol_nanoseconds() {
        let dst = [0x11; 6];
        let src = [0x22; 6];
        let frame = untagged_eth(dst, src, 0x88B5);
        let total_ticks: u64 = 9_876_543_210;
        let ts_high = (total_ticks >> 32) as u32;
        let ts_low = (total_ticks & 0xFFFF_FFFF) as u32;
        let bytes = build_pcapng(1, 9, ts_high, ts_low, &frame);
        let f = write_to_tempfile(&bytes);

        let mut s = PcapngFrameSource::open(f.path()).unwrap();
        let frames: Vec<_> = s.frames().collect();
        let cf = frames.into_iter().next().unwrap().unwrap();
        let expected = (u64::from(ts_high) << 32) | u64::from(ts_low);
        assert_eq!(cf.timestamp_ns, expected);
    }

    // ── gPTP JSON tests ─────────────────────────────────────────────

    const GPTP_CANONICAL: &str = r#"
    {
      "gptp": {
        "grandmaster": "00:1b:21:ff:fe:01:02:03",
        "domain": 0,
        "ports": [
          {
            "name": "eth0",
            "samples": [
              {"timestamp_ns": 1700000000000000000, "sync_error_ns": 250},
              {"timestamp_ns": 1700000001000000000, "sync_error_ns": 310},
              {"timestamp_ns": 1700000002000000000, "sync_error_ns": 280}
            ]
          },
          {
            "name": "eth1",
            "samples": [
              {"timestamp_ns": 1700000000000000000, "sync_error_ns": 410}
            ]
          }
        ]
      }
    }
    "#;

    // ── Qcc YANG tests ──────────────────────────────────────────────

    const QCC_FULL_SINGLE_PORT: &str = r#"
    {
      "interfaces": [
        {
          "name": "swp1",
          "tsn": {
            "gate-control-list": [
              {"gate-states-value": 1, "time-interval-value": 500000},
              {"gate-states-value": 254, "time-interval-value": 9500000}
            ],
            "bandwidth-reservation-permille": 750,
            "max-frame-size": 1518,
            "stream-filters": [
              {"stream-handle": 42, "priority-spec": 5}
            ]
          }
        }
      ]
    }
    "#;

    #[test]
    fn gptp_full_dump_two_ports() {
        let src = GptpJsonPtpTimeSource::from_json_str(GPTP_CANONICAL).expect("parse");
        assert_eq!(src.grandmaster(), Some("00:1b:21:ff:fe:01:02:03"));
        assert_eq!(src.domain(), Some(0));
        let ports = src.ports();
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].name, "eth0");
        assert_eq!(ports[0].samples.len(), 3);
        assert_eq!(
            ports[0].samples[0],
            PtpSample {
                timestamp_ns: 1_700_000_000_000_000_000,
                sync_error_ns: 250
            }
        );
        assert_eq!(ports[0].samples[1].sync_error_ns, 310);
        assert_eq!(ports[0].samples[2].sync_error_ns, 280);
        assert_eq!(ports[1].name, "eth1");
        assert_eq!(ports[1].samples.len(), 1);
        assert_eq!(ports[1].samples[0].sync_error_ns, 410);
        assert_eq!(ports[1].samples[0].timestamp_ns, 1_700_000_000_000_000_000);
    }

    #[test]
    fn gptp_port_with_no_samples() {
        let json = r#"
        {
          "gptp": {
            "domain": 20,
            "ports": [
              {"name": "eth0", "samples": []}
            ]
          }
        }
        "#;
        let src = GptpJsonPtpTimeSource::from_json_str(json).expect("parse");
        assert_eq!(src.ports().len(), 1);
        assert_eq!(src.ports()[0].name, "eth0");
        assert!(src.ports()[0].samples.is_empty());
        assert_eq!(src.domain(), Some(20));
    }

    #[test]
    fn gptp_missing_grandmaster_and_domain() {
        let json = r#"
        {
          "gptp": {
            "ports": [
              {"name": "eth0", "samples": []}
            ]
          }
        }
        "#;
        let src = GptpJsonPtpTimeSource::from_json_str(json).expect("parse");
        assert_eq!(src.grandmaster(), None);
        assert_eq!(src.domain(), None);
        assert_eq!(src.ports().len(), 1);
    }

    #[test]
    fn gptp_missing_gptp_root_yields_error() {
        let json = r#"{"not_gptp": {}}"#;
        match GptpJsonPtpTimeSource::from_json_str(json) {
            Err(IngestError::MalformedPtpJson(msg)) => {
                assert!(
                    msg.contains("gptp"),
                    "error message should mention `gptp`, got: {msg}"
                );
            }
            other => panic!("expected MalformedPtpJson, got {other:?}"),
        }
    }

    #[test]
    fn gptp_missing_ports_yields_error() {
        let json = r#"{"gptp": {}}"#;
        match GptpJsonPtpTimeSource::from_json_str(json) {
            Err(IngestError::MalformedPtpJson(msg)) => {
                assert!(
                    msg.contains("ports"),
                    "error message should mention `ports`, got: {msg}"
                );
            }
            other => panic!("expected MalformedPtpJson, got {other:?}"),
        }
    }

    #[test]
    fn gptp_open_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gptp.json");
        std::fs::write(&path, GPTP_CANONICAL).expect("write tempfile");
        let src = GptpJsonPtpTimeSource::open(&path).expect("open");
        assert_eq!(src.grandmaster(), Some("00:1b:21:ff:fe:01:02:03"));
        assert_eq!(src.domain(), Some(0));
        assert_eq!(src.ports().len(), 2);
    }

    #[test]
    fn gptp_sample_with_negative_error_clamps_to_zero_or_errors() {
        // Schema is unsigned magnitudes — negative offsets must be
        // pre-`abs()`'d by the producer. We reject negative integers
        // with MalformedPtpJson rather than silently clamping.
        let json = r#"
        {
          "gptp": {
            "ports": [
              {
                "name": "eth0",
                "samples": [
                  {"timestamp_ns": 1700000000000000000, "sync_error_ns": -100}
                ]
              }
            ]
          }
        }
        "#;
        match GptpJsonPtpTimeSource::from_json_str(json) {
            Err(IngestError::MalformedPtpJson(msg)) => {
                assert!(
                    msg.contains("sync_error_ns"),
                    "error message should mention `sync_error_ns`, got: {msg}"
                );
            }
            other => panic!("expected MalformedPtpJson, got {other:?}"),
        }
    }

    #[test]
    fn qcc_single_port_full_config() {
        let src = QccYangSwitchConfigSource::from_json_str(QCC_FULL_SINGLE_PORT).expect("parse");
        let ports = src.ports();
        assert_eq!(ports.len(), 1);
        let p = &ports[0];
        assert_eq!(p.port_name, "swp1");

        let gates = p.gate_control_list.as_ref().expect("gate list present");
        assert_eq!(gates.len(), 2);
        assert_eq!(gates[0].gate_states_value, 1);
        assert_eq!(gates[0].time_interval_value, 500_000);
        assert_eq!(gates[1].gate_states_value, 254);
        assert_eq!(gates[1].time_interval_value, 9_500_000);

        assert_eq!(p.bandwidth_reservation_permille, Some(750));
        assert_eq!(p.max_frame_size, Some(1518));

        let streams = p.streams.as_ref().expect("streams present");
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].stream_handle, 42);
        assert_eq!(streams[0].priority_spec, 5);
    }

    #[test]
    fn qcc_port_with_only_max_frame_size() {
        let json = r#"
        {
          "interfaces": [
            {
              "name": "swp2",
              "tsn": {
                "max-frame-size": 9000
              }
            }
          ]
        }
        "#;
        let src = QccYangSwitchConfigSource::from_json_str(json).expect("parse");
        let ports = src.ports();
        assert_eq!(ports.len(), 1);
        let p = &ports[0];
        assert_eq!(p.port_name, "swp2");
        assert_eq!(p.gate_control_list, None);
        assert_eq!(p.bandwidth_reservation_permille, None);
        assert_eq!(p.max_frame_size, Some(9000));
        assert_eq!(p.streams, None);
    }

    #[test]
    fn qcc_multiple_ports() {
        let json = r#"
        {
          "interfaces": [
            {
              "name": "swp1",
              "tsn": {
                "bandwidth-reservation-permille": 250,
                "max-frame-size": 1518
              }
            },
            {
              "name": "swp2",
              "tsn": {
                "gate-control-list": [
                  {"gate-states-value": 255, "time-interval-value": 1000000}
                ],
                "stream-filters": [
                  {"stream-handle": 7, "priority-spec": 3},
                  {"stream-handle": 8, "priority-spec": 4}
                ]
              }
            }
          ]
        }
        "#;
        let src = QccYangSwitchConfigSource::from_json_str(json).expect("parse");
        let ports = src.ports();
        assert_eq!(ports.len(), 2);

        assert_eq!(ports[0].port_name, "swp1");
        assert_eq!(ports[0].bandwidth_reservation_permille, Some(250));
        assert_eq!(ports[0].max_frame_size, Some(1518));
        assert_eq!(ports[0].gate_control_list, None);
        assert_eq!(ports[0].streams, None);

        assert_eq!(ports[1].port_name, "swp2");
        assert_eq!(ports[1].bandwidth_reservation_permille, None);
        assert_eq!(ports[1].max_frame_size, None);
        let gates = ports[1].gate_control_list.as_ref().expect("gates");
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].gate_states_value, 255);
        assert_eq!(gates[0].time_interval_value, 1_000_000);
        let streams = ports[1].streams.as_ref().expect("streams");
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0].stream_handle, 7);
        assert_eq!(streams[1].stream_handle, 8);
        assert_eq!(streams[1].priority_spec, 4);
    }

    #[test]
    fn qcc_empty_interfaces() {
        let src = QccYangSwitchConfigSource::from_json_str(r#"{"interfaces": []}"#).expect("parse");
        assert!(src.ports().is_empty());
    }

    #[test]
    fn qcc_missing_root_yields_error() {
        let json = r#"{"not_interfaces": []}"#;
        match QccYangSwitchConfigSource::from_json_str(json) {
            Err(IngestError::MalformedQccJson(_)) => {}
            other => panic!("expected MalformedQccJson, got {other:?}"),
        }
    }

    #[test]
    fn qcc_open_from_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("qcc.json");
        std::fs::write(&path, QCC_FULL_SINGLE_PORT).expect("write tempfile");
        let src = QccYangSwitchConfigSource::open(&path).expect("open");
        assert_eq!(src.ports().len(), 1);
        assert_eq!(src.ports()[0].port_name, "swp1");
        assert_eq!(src.ports()[0].max_frame_size, Some(1518));
    }

    #[test]
    fn qcc_invalid_bandwidth_reservation() {
        // permille > 1000 is out-of-range — we reject with MalformedQccJson.
        let json = r#"
        {
          "interfaces": [
            {
              "name": "swp1",
              "tsn": {"bandwidth-reservation-permille": 1500}
            }
          ]
        }
        "#;
        match QccYangSwitchConfigSource::from_json_str(json) {
            Err(IngestError::MalformedQccJson(msg)) => {
                assert!(
                    msg.contains("bandwidth-reservation-permille"),
                    "error message should mention the offending field, got: {msg}"
                );
            }
            other => panic!("expected MalformedQccJson, got {other:?}"),
        }
    }
}
