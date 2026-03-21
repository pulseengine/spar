//! Virtual bus protocol library — common communication protocol definitions.
//!
//! This module provides a catalog of AADL virtual bus types representing
//! common communication protocols across automotive, aerospace, industrial,
//! embedded, and general-purpose domains. Each protocol carries
//! deployment-relevant properties: latency overhead, bandwidth, max payload
//! size, security profile, and physical bus type compatibility.
//!
//! The generated ItemTree is equivalent to:
//!
//! ```aadl
//! package Protocol_Library
//! public
//!   virtual bus DDS end DDS;
//!   virtual bus SOME_IP end SOME_IP;
//!   virtual bus CAN end CAN;
//!   virtual bus CAN_FD end CAN_FD;
//!   virtual bus FlexRay end FlexRay;
//!   virtual bus Ethernet end Ethernet;
//!   virtual bus SharedMemory end SharedMemory;
//!   virtual bus AFDX end AFDX;
//!   virtual bus ARINC429 end ARINC429;
//!   virtual bus SpaceWire end SpaceWire;
//!   virtual bus PROFINET end PROFINET;
//!   virtual bus EtherCAT end EtherCAT;
//!   virtual bus MAVLink end MAVLink;
//! end Protocol_Library;
//! ```

use spar_hir_def::item_tree::*;
use spar_hir_def::name::Name;

/// Protocol definition with deployment-relevant properties.
#[derive(Debug, Clone)]
pub struct ProtocolDef {
    /// Protocol name (used as the AADL virtual bus type name).
    pub name: &'static str,
    /// Domain category: "automotive", "aerospace", "industrial", "embedded", "general".
    pub category: &'static str,
    /// Physical bus type compatibility (e.g., "ethernet", "can", "serial").
    pub bus_type: &'static str,
    /// Typical protocol latency overhead in microseconds.
    pub latency_overhead_us: f64,
    /// Typical bandwidth in megabits per second.
    pub bandwidth_mbps: f64,
    /// Maximum payload size in bytes.
    pub max_payload_bytes: u64,
    /// Security profile: "none", "optional-tls", "mandatory-tls", "optional-signing".
    pub security: &'static str,
}

/// Static catalog of all supported communication protocols.
pub const PROTOCOLS: &[ProtocolDef] = &[
    // Automotive
    ProtocolDef {
        name: "DDS",
        category: "automotive",
        bus_type: "ethernet",
        latency_overhead_us: 500.0,
        bandwidth_mbps: 1000.0,
        max_payload_bytes: 65536,
        security: "optional-tls",
    },
    ProtocolDef {
        name: "SOME_IP",
        category: "automotive",
        bus_type: "ethernet",
        latency_overhead_us: 200.0,
        bandwidth_mbps: 1000.0,
        max_payload_bytes: 1400,
        security: "optional-tls",
    },
    ProtocolDef {
        name: "CAN",
        category: "automotive",
        bus_type: "can",
        latency_overhead_us: 100.0,
        bandwidth_mbps: 0.5,
        max_payload_bytes: 8,
        security: "none",
    },
    ProtocolDef {
        name: "CAN_FD",
        category: "automotive",
        bus_type: "can",
        latency_overhead_us: 80.0,
        bandwidth_mbps: 5.0,
        max_payload_bytes: 64,
        security: "none",
    },
    ProtocolDef {
        name: "FlexRay",
        category: "automotive",
        bus_type: "flexray",
        latency_overhead_us: 50.0,
        bandwidth_mbps: 10.0,
        max_payload_bytes: 254,
        security: "none",
    },
    // General
    ProtocolDef {
        name: "Ethernet",
        category: "general",
        bus_type: "ethernet",
        latency_overhead_us: 10.0,
        bandwidth_mbps: 1000.0,
        max_payload_bytes: 1500,
        security: "none",
    },
    ProtocolDef {
        name: "SharedMemory",
        category: "general",
        bus_type: "shared_memory",
        latency_overhead_us: 0.1,
        bandwidth_mbps: 100000.0,
        max_payload_bytes: u64::MAX,
        security: "none",
    },
    // Aerospace
    ProtocolDef {
        name: "AFDX",
        category: "aerospace",
        bus_type: "ethernet",
        latency_overhead_us: 500.0,
        bandwidth_mbps: 100.0,
        max_payload_bytes: 1471,
        security: "none",
    },
    ProtocolDef {
        name: "ARINC429",
        category: "aerospace",
        bus_type: "arinc429",
        latency_overhead_us: 1000.0,
        bandwidth_mbps: 0.1,
        max_payload_bytes: 4,
        security: "none",
    },
    ProtocolDef {
        name: "SpaceWire",
        category: "aerospace",
        bus_type: "spacewire",
        latency_overhead_us: 1.0,
        bandwidth_mbps: 200.0,
        max_payload_bytes: 65536,
        security: "none",
    },
    // Industrial
    ProtocolDef {
        name: "PROFINET",
        category: "industrial",
        bus_type: "ethernet",
        latency_overhead_us: 250.0,
        bandwidth_mbps: 100.0,
        max_payload_bytes: 1440,
        security: "none",
    },
    ProtocolDef {
        name: "EtherCAT",
        category: "industrial",
        bus_type: "ethernet",
        latency_overhead_us: 1.0,
        bandwidth_mbps: 100.0,
        max_payload_bytes: 1486,
        security: "none",
    },
    // Embedded
    ProtocolDef {
        name: "MAVLink",
        category: "embedded",
        bus_type: "serial",
        latency_overhead_us: 50.0,
        bandwidth_mbps: 0.115,
        max_payload_bytes: 255,
        security: "optional-signing",
    },
];

/// Generate the protocol library as an AADL ItemTree.
///
/// Creates a package `Protocol_Library` containing a virtual bus type
/// for each protocol in the catalog.
pub fn protocol_library() -> ItemTree {
    let mut tree = ItemTree::default();
    let mut public_items = Vec::new();

    for proto in PROTOCOLS {
        let ct_idx = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new(proto.name),
            category: ComponentCategory::VirtualBus,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
        });
        public_items.push(ItemRef::ComponentType(ct_idx));
    }

    tree.packages.alloc(Package {
        name: Name::new("Protocol_Library"),
        with_clauses: Vec::new(),
        public_items,
        private_items: Vec::new(),
        renames: Vec::new(),
    });

    tree
}

/// Return all protocols compatible with a given physical bus type.
///
/// Bus type names are matched case-insensitively.
pub fn protocols_for_bus_type(bus_type: &str) -> Vec<&'static ProtocolDef> {
    PROTOCOLS
        .iter()
        .filter(|p| p.bus_type.eq_ignore_ascii_case(bus_type))
        .collect()
}

/// Look up a protocol by name.
///
/// Name matching is case-insensitive.
pub fn protocol_by_name(name: &str) -> Option<&'static ProtocolDef> {
    PROTOCOLS
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_library_has_13_protocols() {
        assert_eq!(PROTOCOLS.len(), 13);
    }

    #[test]
    fn all_protocols_have_valid_properties() {
        for proto in PROTOCOLS {
            assert!(
                proto.bandwidth_mbps > 0.0,
                "{} has zero or negative bandwidth",
                proto.name
            );
            assert!(
                proto.latency_overhead_us >= 0.0,
                "{} has negative latency",
                proto.name
            );
            assert!(
                proto.max_payload_bytes > 0,
                "{} has zero max payload",
                proto.name
            );
            assert!(
                !proto.name.is_empty(),
                "protocol name must not be empty"
            );
            assert!(
                !proto.category.is_empty(),
                "{} has empty category",
                proto.name
            );
            assert!(
                !proto.bus_type.is_empty(),
                "{} has empty bus_type",
                proto.name
            );
            assert!(
                !proto.security.is_empty(),
                "{} has empty security",
                proto.name
            );
        }
    }

    #[test]
    fn protocols_for_ethernet() {
        let ethernet_protos = protocols_for_bus_type("ethernet");
        let names: Vec<&str> = ethernet_protos.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            &["DDS", "SOME_IP", "Ethernet", "AFDX", "PROFINET", "EtherCAT"]
        );
    }

    #[test]
    fn protocols_for_can() {
        let can_protos = protocols_for_bus_type("can");
        let names: Vec<&str> = can_protos.iter().map(|p| p.name).collect();
        assert_eq!(names, &["CAN", "CAN_FD"]);
    }

    #[test]
    fn protocol_by_name_found() {
        let dds = protocol_by_name("DDS");
        assert!(dds.is_some());
        let dds = dds.unwrap();
        assert_eq!(dds.name, "DDS");
        assert_eq!(dds.category, "automotive");
        assert_eq!(dds.bus_type, "ethernet");
    }

    #[test]
    fn protocol_by_name_not_found() {
        assert!(protocol_by_name("INVALID").is_none());
    }

    #[test]
    fn item_tree_has_correct_categories() {
        let tree = protocol_library();
        assert_eq!(tree.component_types.len(), 13);
        for (_, ct) in tree.component_types.iter() {
            assert_eq!(
                ct.category,
                ComponentCategory::VirtualBus,
                "{} should be VirtualBus category",
                ct.name
            );
        }
    }

    #[test]
    fn item_tree_package_structure() {
        let tree = protocol_library();
        assert_eq!(tree.packages.len(), 1);
        let (_, pkg) = tree.packages.iter().next().unwrap();
        assert_eq!(pkg.name.as_str(), "Protocol_Library");
        assert_eq!(pkg.public_items.len(), 13);
        assert!(pkg.private_items.is_empty());
    }

    #[test]
    fn all_virtual_bus_types_are_public() {
        let tree = protocol_library();
        for (_, ct) in tree.component_types.iter() {
            assert!(ct.is_public, "{} should be public", ct.name);
        }
    }

    #[test]
    fn protocol_names_match_item_tree_names() {
        let tree = protocol_library();
        let tree_names: Vec<&str> = tree
            .component_types
            .iter()
            .map(|(_, ct)| ct.name.as_str())
            .collect();
        let catalog_names: Vec<&str> = PROTOCOLS.iter().map(|p| p.name).collect();
        assert_eq!(tree_names, catalog_names);
    }
}
