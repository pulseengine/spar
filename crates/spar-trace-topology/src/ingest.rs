//! Placeholder trait surface for runtime-artefact parsers.
//!
//! Real parsers land in v0.10.x sibling commits — PCAPNG (`pcap-parser`
//! crate or hand-rolled), LLDP (LLDP TLVs from frames or
//! lldpd-style YAML), Qcc YANG (`ieee802-dot1q-bridge`,
//! `ieee802-dot1q-tsn-types` schema), gPTP (linuxptp's `ptp4l` /
//! `pmc` JSON or CTF events).
//!
//! The trait shapes are minimal — concrete return types are
//! deliberately deferred (returning `()` rather than typed
//! envelopes) so the parsers can negotiate their own data
//! structures without churning this surface. v0.11.0 widens these
//! traits once the reconciliation engine settles on its working set.
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
/// TODO(v0.10.0+): real parser — IEEE 802.1AB-2016 (LLDP) TLV
/// decoding. See design doc §"Input artefact set" §LLDP.
pub trait TopologySource {
    /// Open the topology source at `path`. v0.10.0 placeholder.
    fn open(path: &Path) -> Result<Self, IngestError>
    where
        Self: Sized;
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

/// Errors surfaced from a runtime-artefact parser.
///
/// v0.10.0 ships an `Unimplemented` variant only — the foundation
/// crate carries no real I/O. v0.10.x parsers extend this enum with
/// the concrete I/O / format-decode kinds.
#[derive(Debug)]
pub enum IngestError {
    /// The requested parser surface is not implemented in this
    /// build of spar-trace-topology. v0.10.0 returns this from every
    /// `open` call; v0.10.x parsers replace it with concrete kinds.
    Unimplemented,
}

impl core::fmt::Display for IngestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unimplemented => write!(
                f,
                "parser not implemented in v0.10.0 foundation; see \
                 docs/designs/v0.10.0-trace-topology.md"
            ),
        }
    }
}

impl std::error::Error for IngestError {}
