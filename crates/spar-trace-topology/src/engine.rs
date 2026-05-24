//! Reconciliation engine — the deterministic checks that compare a
//! declared AADL model against the observed runtime artefact set.
//!
//! The v1 design pins five checks (`docs/designs/v0.10.0-trace-topology.md`
//! §"Five deterministic checks"); v0.10.0 shipped only the
//! [`ReconcileFinding`](crate::reconcile::ReconcileFinding) type surface.
//! The v0.11.0 engine lands those checks **incrementally** — each check is
//! a self-contained, deterministic function so a partial engine is still a
//! useful, testable engine.
//!
//! ## What this module currently implements
//!
//! - [`DeclaredModel`] — the index of identities the AADL model declares,
//!   built once by walking a [`SystemInstance`].
//! - [`check_identity_unknown`] — the `IdentityUnknown` check (design
//!   §4.1), restricted in this release to the **component-borne**
//!   identities: `Spar_Identity::MAC_Address` and
//!   `Spar_Identity::LLDP_Chassis_Id`, both declared on AADL devices,
//!   processors, and buses (all of which are `ComponentInstance`s, so
//!   their property maps are reachable via
//!   [`SystemInstance::properties_for`]).
//!
//! ## Deliberately deferred
//!
//! - **Connection-borne identities** — `Stream_Handle`, `Multicast_Group`,
//!   and `VLAN_ID` are declared on AADL *connections*, not components, so
//!   they are not reachable through the component property-map surface
//!   this module uses. They land once the connection-property surfacing
//!   is settled, in a sibling commit.
//! - **The other four checks** (`TopologyMissingWiring`, `ConfigDrift`,
//!   `GptpOutOfBudget`, `BinaryMismatch`) and the orchestrating
//!   `reconcile()` entry point that composes all five.
//! - **SARIF 2.1.0 emission** and the **in-toto v1.0 attestation
//!   envelope** — design §5; later v0.11.0 / v1.0 commits.
//!
//! ## Determinism
//!
//! Per the design, two runs over byte-identical inputs must produce
//! byte-identical reports. Every check here collects observed identities
//! into ordered ([`BTreeSet`]) collections before diffing, so finding
//! order is a pure function of the input content, never of iteration or
//! hashing order.

use std::collections::BTreeSet;

use spar_hir_def::instance::SystemInstance;

use crate::identity;
use crate::ingest::{FrameSource, IngestError, TopologySource};
use crate::reconcile::ReconcileFinding;

/// An error raised while the reconciliation engine consumes its inputs.
///
/// Reconciliation findings are *not* errors — they are the expected
/// output and travel in the [`TopologyReport`](crate::report::TopologyReport).
/// A [`ReconcileError`] means the engine could not run the check at all
/// because an input artefact was missing or malformed; it maps to the
/// CLI's exit code `2` (input parse failure).
#[derive(Debug)]
pub enum ReconcileError {
    /// An input artefact failed to parse while the engine consumed it.
    Ingest(IngestError),
}

impl std::fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ingest(e) => write!(f, "input artefact ingest failed: {e}"),
        }
    }
}

impl std::error::Error for ReconcileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Ingest(e) => Some(e),
        }
    }
}

impl From<IngestError> for ReconcileError {
    fn from(e: IngestError) -> Self {
        Self::Ingest(e)
    }
}

/// Parse a MAC address into its canonical six bytes.
///
/// Lenient on presentation: `:` and `-` separators are both accepted and
/// stripped, and hex digits may be upper or lower case. The cleaned input
/// must be exactly twelve hex digits — anything else returns `None`.
///
/// This is the single normalisation point for MAC comparison: a declared
/// `Spar_Identity::MAC_Address` and an observed PCAPNG MAC are only ever
/// compared after both have passed through here, so the comparison is
/// separator- and case-insensitive by construction.
#[must_use]
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let cleaned: String = s.chars().filter(|c| *c != ':' && *c != '-').collect();
    if cleaned.len() != 12 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// Format six bytes as a canonical lower-case colon-separated MAC string.
fn format_mac(mac: &[u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    )
}

/// `true` when the multicast/group bit (the least-significant bit of the
/// first octet) is set.
///
/// A group-addressed MAC — broadcast `ff:ff:ff:ff:ff:ff`, the IEEE
/// reserved `01:80:c2:…` range (LLDP, STP), `01:1b:19:…` (gPTP), the IPv4
/// `01:00:5e:…` and IPv6 `33:33:…` multicast ranges — is *not* a unicast
/// station identity. Such MACs are reconciled against `Multicast_Group`
/// declarations, never against `MAC_Address`, so `IdentityUnknown` skips
/// them. Without this filter every capture would flood findings for the
/// protocol traffic that any real deployment carries.
fn is_multicast_mac(mac: &[u8; 6]) -> bool {
    mac[0] & 0x01 != 0
}

/// Normalise an LLDP chassis-id for set membership.
///
/// A MAC-shaped value is folded to its canonical colon form (so a
/// chassis-id reported as `AA-BB-CC-DD-EE-FF` matches a declared
/// `aa:bb:cc:dd:ee:ff`); anything else is trimmed and lower-cased. The
/// identical normalisation is applied to declared and observed values, so
/// the comparison is separator- and case-insensitive on both sides.
fn normalise_chassis_id(s: &str) -> String {
    let trimmed = s.trim();
    if let Some(mac) = parse_mac(trimmed) {
        format_mac(&mac)
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// The identities an AADL model declares, indexed for reconciliation.
///
/// Built once — typically with [`DeclaredModel::from_instance`] — and then
/// queried by the checks. The collections are [`BTreeSet`]s so iteration
/// is ordered, which keeps finding emission deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredModel {
    /// Canonical six-byte MACs declared via `Spar_Identity::MAC_Address`.
    declared_macs: BTreeSet<[u8; 6]>,
    /// Normalised LLDP chassis-ids declared via
    /// `Spar_Identity::LLDP_Chassis_Id`.
    declared_chassis_ids: BTreeSet<String>,
}

impl DeclaredModel {
    /// Create an empty model — no declared identities.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a declared `Spar_Identity::MAC_Address`.
    ///
    /// A value that does not parse as a MAC is dropped: the engine then
    /// cannot recognise an observed frame carrying that address and will
    /// (correctly, conservatively) raise an `IdentityUnknown` finding for
    /// it — surfacing the malformed AADL declaration rather than masking
    /// it.
    pub fn declare_mac(&mut self, mac: &str) {
        if let Some(m) = parse_mac(mac) {
            self.declared_macs.insert(m);
        }
    }

    /// Record a declared `Spar_Identity::LLDP_Chassis_Id`.
    pub fn declare_chassis_id(&mut self, chassis_id: &str) {
        self.declared_chassis_ids
            .insert(normalise_chassis_id(chassis_id));
    }

    /// Build the declared model by walking every component instance of an
    /// instantiated AADL system and reading its `Spar_Identity::*`
    /// property surface.
    ///
    /// Components that declare no identity property simply contribute
    /// nothing — no component-category filtering is needed.
    #[must_use]
    pub fn from_instance(instance: &SystemInstance) -> Self {
        let mut model = Self::new();
        for (idx, _component) in instance.all_components() {
            let props = instance.properties_for(idx);
            if let Some(mac) = identity::get_mac_address(props) {
                model.declare_mac(&mac);
            }
            if let Some(chassis_id) = identity::get_lldp_chassis_id(props) {
                model.declare_chassis_id(&chassis_id);
            }
        }
        model
    }

    /// `true` when `mac` (in any accepted presentation) is declared.
    #[must_use]
    pub fn knows_mac(&self, mac: &str) -> bool {
        parse_mac(mac).is_some_and(|m| self.declared_macs.contains(&m))
    }

    /// `true` when `chassis_id` is declared, comparing under the same
    /// normalisation [`declare_chassis_id`](Self::declare_chassis_id) uses.
    #[must_use]
    pub fn knows_chassis_id(&self, chassis_id: &str) -> bool {
        self.declared_chassis_ids
            .contains(&normalise_chassis_id(chassis_id))
    }
}

/// `IdentityUnknown` — design §4.1.
///
/// **Pass:** every unicast MAC observed in the frame capture, and every
/// LLDP neighbor chassis-id observed in the topology snapshot, is declared
/// by some `Spar_Identity::*` property on the AADL model.
///
/// **Fail:** an observed identity has no AADL declaration — one
/// [`ReconcileFinding::IdentityUnknown`] per distinct unknown identity,
/// carrying the observed value and the artefact context.
///
/// Group-addressed MACs are skipped (see [`is_multicast_mac`]). An LLDP
/// chassis-id that is itself a declared `MAC_Address` counts as known —
/// the chassis-id and MAC surfaces name the same physical station.
///
/// Findings are deterministic: PCAPNG MAC findings first (ascending by
/// address), then LLDP chassis-id findings (ascending lexically), each
/// distinct identity reported exactly once.
///
/// # Errors
///
/// Returns [`ReconcileError::Ingest`] if the frame capture cannot be read
/// to completion.
pub fn check_identity_unknown(
    declared: &DeclaredModel,
    frames: &mut dyn FrameSource,
    topology: &dyn TopologySource,
) -> Result<Vec<ReconcileFinding>, ReconcileError> {
    // Collect distinct observed unicast MACs. BTreeSet => ordered, deduped.
    let mut observed_macs: BTreeSet<[u8; 6]> = BTreeSet::new();
    for frame in frames.frames() {
        let frame = frame?;
        for mac in [frame.mac_src, frame.mac_dst] {
            if !is_multicast_mac(&mac) {
                observed_macs.insert(mac);
            }
        }
    }

    let mut findings = Vec::new();
    for mac in &observed_macs {
        if !declared.declared_macs.contains(mac) {
            findings.push(ReconcileFinding::IdentityUnknown {
                observed: format_mac(mac),
                context: "PCAPNG frame capture".to_string(),
            });
        }
    }

    // Collect distinct observed LLDP chassis-ids, normalised.
    let mut observed_chassis: BTreeSet<String> = BTreeSet::new();
    for neighbor in topology.neighbors() {
        observed_chassis.insert(normalise_chassis_id(&neighbor.remote_chassis_id));
    }
    for chassis_id in &observed_chassis {
        let known = declared.declared_chassis_ids.contains(chassis_id)
            || parse_mac(chassis_id).is_some_and(|m| declared.declared_macs.contains(&m));
        if !known {
            findings.push(ReconcileFinding::IdentityUnknown {
                observed: chassis_id.clone(),
                context: "LLDP neighbor snapshot".to_string(),
            });
        }
    }

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{CapturedFrame, LldpNeighbor};

    /// In-memory [`FrameSource`] for tests — single-pass, no real I/O.
    struct VecFrames(Vec<CapturedFrame>);

    impl FrameSource for VecFrames {
        fn frames(&mut self) -> Box<dyn Iterator<Item = Result<CapturedFrame, IngestError>> + '_> {
            Box::new(std::mem::take(&mut self.0).into_iter().map(Ok))
        }
    }

    /// In-memory [`TopologySource`] for tests.
    struct VecTopology(Vec<LldpNeighbor>);

    impl TopologySource for VecTopology {
        fn neighbors(&self) -> &[LldpNeighbor] {
            &self.0
        }
    }

    fn frame(src: [u8; 6], dst: [u8; 6]) -> CapturedFrame {
        CapturedFrame {
            mac_src: src,
            mac_dst: dst,
            vlan_id: None,
            pcp: None,
            timestamp_ns: 0,
        }
    }

    fn neighbor(remote_chassis_id: &str) -> LldpNeighbor {
        LldpNeighbor {
            local_port: "eth0".to_string(),
            remote_chassis_id: remote_chassis_id.to_string(),
            remote_chassis_id_type: "mac".to_string(),
            remote_port_id: "p1".to_string(),
            remote_port_id_type: "ifname".to_string(),
            remote_system_name: None,
        }
    }

    #[test]
    fn parse_mac_accepts_colon_dash_and_case() {
        let expected = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff"), Some(expected));
        assert_eq!(parse_mac("AA-BB-CC-DD-EE-FF"), Some(expected));
        assert_eq!(parse_mac("aabbccddeeff"), Some(expected));
    }

    #[test]
    fn parse_mac_rejects_malformed() {
        assert_eq!(parse_mac("aa:bb:cc:dd:ee"), None); // too short
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff:00"), None); // too long
        assert_eq!(parse_mac("gg:bb:cc:dd:ee:ff"), None); // non-hex
        assert_eq!(parse_mac(""), None);
    }

    #[test]
    fn is_multicast_mac_detects_group_bit() {
        assert!(is_multicast_mac(&[0xff; 6])); // broadcast
        assert!(is_multicast_mac(&[0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e])); // LLDP
        assert!(is_multicast_mac(&[0x01, 0x1b, 0x19, 0x00, 0x00, 0x00])); // gPTP
        assert!(!is_multicast_mac(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01])); // unicast
    }

    #[test]
    fn normalise_chassis_id_folds_mac_and_lowercases() {
        assert_eq!(
            normalise_chassis_id("AA-BB-CC-DD-EE-FF"),
            "aa:bb:cc:dd:ee:ff"
        );
        assert_eq!(normalise_chassis_id("  Core-Switch-1 "), "core-switch-1");
    }

    #[test]
    fn identity_unknown_clean_when_all_declared() {
        let mut model = DeclaredModel::new();
        model.declare_mac("02:00:00:00:00:01");
        model.declare_mac("02:00:00:00:00:02");
        model.declare_chassis_id("core-switch-1");

        let a = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let b = [0x02, 0x00, 0x00, 0x00, 0x00, 0x02];
        let mut frames = VecFrames(vec![frame(a, b)]);
        let topology = VecTopology(vec![neighbor("core-switch-1")]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn identity_unknown_flags_undeclared_mac() {
        let mut model = DeclaredModel::new();
        model.declare_mac("02:00:00:00:00:01");

        let known = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let unknown = [0x02, 0x00, 0x00, 0x00, 0x00, 0x09];
        let mut frames = VecFrames(vec![frame(known, unknown)]);
        let topology = VecTopology(vec![]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        assert_eq!(
            findings,
            vec![ReconcileFinding::IdentityUnknown {
                observed: "02:00:00:00:00:09".to_string(),
                context: "PCAPNG frame capture".to_string(),
            }]
        );
    }

    #[test]
    fn identity_unknown_exempts_multicast_and_broadcast() {
        // Empty model: any unicast MAC would be flagged. The capture only
        // carries group-addressed traffic, so the report stays clean.
        let model = DeclaredModel::new();
        let broadcast = [0xff; 6];
        let lldp = [0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e];
        let unicast_src = [0x02, 0x00, 0x00, 0x00, 0x00, 0x07];
        let mut frames = VecFrames(vec![
            frame(unicast_src, broadcast),
            frame(unicast_src, lldp),
        ]);
        let topology = VecTopology(vec![]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        // Only the unicast source is flagged; the group destinations are not.
        assert_eq!(
            findings,
            vec![ReconcileFinding::IdentityUnknown {
                observed: "02:00:00:00:00:07".to_string(),
                context: "PCAPNG frame capture".to_string(),
            }]
        );
    }

    #[test]
    fn identity_unknown_dedups_and_sorts() {
        let model = DeclaredModel::new();
        let high = [0x02, 0x00, 0x00, 0x00, 0x00, 0x09];
        let low = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        // `high` observed twice, in reverse address order.
        let mut frames = VecFrames(vec![frame(high, high), frame(low, low)]);
        let topology = VecTopology(vec![]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        let observed: Vec<&str> = findings
            .iter()
            .map(|f| match f {
                ReconcileFinding::IdentityUnknown { observed, .. } => observed.as_str(),
                _ => unreachable!(),
            })
            .collect();
        // Each MAC once, ascending — deterministic regardless of frame order.
        assert_eq!(observed, ["02:00:00:00:00:01", "02:00:00:00:00:09"]);
    }

    #[test]
    fn identity_unknown_flags_undeclared_chassis_id() {
        let mut model = DeclaredModel::new();
        model.declare_chassis_id("core-switch-1");
        let mut frames = VecFrames(vec![]);
        let topology = VecTopology(vec![neighbor("rogue-switch-9")]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        assert_eq!(
            findings,
            vec![ReconcileFinding::IdentityUnknown {
                observed: "rogue-switch-9".to_string(),
                context: "LLDP neighbor snapshot".to_string(),
            }]
        );
    }

    #[test]
    fn identity_unknown_accepts_chassis_id_equal_to_declared_mac() {
        // A chassis-id of type `mac` names the same station as the
        // declared MAC_Address — it must not be flagged.
        let mut model = DeclaredModel::new();
        model.declare_mac("aa:bb:cc:dd:ee:ff");
        let mut frames = VecFrames(vec![]);
        let topology = VecTopology(vec![neighbor("AA:BB:CC:DD:EE:FF")]);

        let findings = check_identity_unknown(&model, &mut frames, &topology).unwrap();
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn ingest_failure_surfaces_as_reconcile_error() {
        struct FailingFrames;
        impl FrameSource for FailingFrames {
            fn frames(
                &mut self,
            ) -> Box<dyn Iterator<Item = Result<CapturedFrame, IngestError>> + '_> {
                Box::new(std::iter::once(Err(IngestError::Truncated)))
            }
        }
        let model = DeclaredModel::new();
        let topology = VecTopology(vec![]);
        let err = check_identity_unknown(&model, &mut FailingFrames, &topology).unwrap_err();
        assert!(matches!(
            err,
            ReconcileError::Ingest(IngestError::Truncated)
        ));
    }
}
