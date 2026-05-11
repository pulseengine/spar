//! Runtime-artefact parser surfaces.
//!
//! v0.10.0 shipped placeholder traits only. v0.10.x sibling commits
//! land the real parsers — PCAPNG (FrameSource), LLDP
//! (TopologySource), Qcc YANG (SwitchConfigSource), gPTP
//! (PtpTimeSource).
//!
//! v0.10.x B-3 (this commit): real LLDP JSON `TopologySource`
//! implementation backed by `lldpctl -f json` output (see
//! <https://lldpd.github.io/>). The other three traits remain
//! placeholders and are filled in by sibling commits.
//!
//! See `docs/designs/v0.10.0-trace-topology.md` §"Implementation
//! phasing" for the per-source roadmap.

use std::path::Path;

/// Source of L2 frames captured at runtime — typically a PCAPNG file
/// recorded with `tcpdump`, `tshark`, or a TAP/SPAN port.
///
/// TODO(v0.10.0+): real parser — PCAPNG (RFC pcapng-draft / IETF
/// opsawg-pcapng). See design doc §"Input artefact set" for the full
/// list of supported link types and capture-options.
pub trait FrameSource {
    /// Open the frame source at `path`. v0.10.0 placeholder — the
    /// real parser returns an iterator of typed frames.
    fn open(path: &Path) -> Result<Self, IngestError>
    where
        Self: Sized;
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
/// `open()` calls that haven't been replaced yet.
#[derive(Debug)]
pub enum IngestError {
    /// The requested parser surface is not implemented in this
    /// build of spar-trace-topology. v0.10.0 returned this from
    /// every `open` call; v0.10.x parsers replace it with concrete
    /// kinds as they land.
    Unimplemented,
    /// Underlying I/O error opening the artefact file.
    Io(std::io::Error),
    /// LLDP JSON dump did not match the `lldpctl -f json` schema.
    MalformedLldpJson(String),
}

impl core::fmt::Display for IngestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unimplemented => write!(
                f,
                "parser not implemented in v0.10.0 foundation; see \
                 docs/designs/v0.10.0-trace-topology.md"
            ),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::MalformedLldpJson(msg) => {
                write!(f, "malformed lldpctl JSON: {msg}")
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
}
