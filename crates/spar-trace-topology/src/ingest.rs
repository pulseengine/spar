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
//! - **Qcc YANG** ([`SwitchConfigSource`]) — placeholder, sibling commit.
//! - **gPTP** ([`PtpTimeSource`]) — placeholder, sibling commit.
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
/// TODO(v0.10.0+): real parser — IEEE 802.1Qcc-2018 plus the
/// `ieee802-dot1q-tsn-types` and `ieee802-dot1q-stream-filters-and-policing`
/// YANG modules. See design doc §"Input artefact set" §Qcc YANG.
pub trait SwitchConfigSource {
    /// Open the switch-config source at `path`. v0.10.0 placeholder.
    fn open(path: &Path) -> Result<Self, IngestError>
    where
        Self: Sized;
}

/// Source of gPTP / IEEE 802.1AS synchronization-error observations
/// over the capture window — typically `ptp4l` summary logs, `pmc`
/// JSON dumps, or CTF events emitted by a Linux/Zephyr gPTP stack.
///
/// TODO(v0.10.0+): real parser — IEEE 802.1AS-2020. The reconciler
/// uses these readings to evaluate the `GptpOutOfBudget` check
/// against `Spar_TSN::Sync_Error`. See design doc §"Input artefact
/// set" §gPTP.
pub trait PtpTimeSource {
    /// Open the gPTP-time source at `path`. v0.10.0 placeholder.
    fn open(path: &Path) -> Result<Self, IngestError>
    where
        Self: Sized;
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
    /// The requested parser surface is not implemented in this
    /// build of spar-trace-topology. v0.10.0 returned this from
    /// every `open` call; v0.10.x parsers replace it with concrete
    /// kinds as they land.
    Unimplemented,
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
            Self::Unimplemented => write!(
                f,
                "parser not implemented in v0.10.0 foundation; see \
                 docs/designs/v0.10.0-trace-topology.md"
            ),
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
}
