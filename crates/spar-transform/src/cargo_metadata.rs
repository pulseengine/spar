//! Parser for `cargo metadata --format-version=1` JSON output.
//!
//! Defines minimal serde structs for the subset of cargo metadata
//! needed to build an AADL representation of a Rust workspace.

use serde::Deserialize;

/// Top-level cargo metadata output.
#[derive(Debug, Clone, Deserialize)]
pub struct CargoMetadata {
    pub packages: Vec<MetadataPackage>,
    pub workspace_members: Vec<String>,
    #[serde(default)]
    pub workspace_root: String,
    pub resolve: Option<MetadataResolve>,
}

/// A package entry from cargo metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: Vec<MetadataDependency>,
    #[serde(default)]
    pub targets: Vec<MetadataTarget>,
    #[serde(default)]
    pub features: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub manifest_path: String,
}

/// A dependency of a package.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataDependency {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub rename: Option<String>,
}

/// A build target (lib, bin, etc.) within a package.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataTarget {
    pub name: String,
    pub kind: Vec<String>,
}

/// The dependency resolution graph.
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataResolve {
    pub nodes: Vec<ResolveNode>,
}

/// A single node in the resolve graph.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolveNode {
    pub id: String,
    #[serde(default)]
    pub deps: Vec<ResolveDep>,
}

/// A resolved dependency edge.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolveDep {
    pub name: String,
    pub pkg: String,
}

/// Parse cargo metadata JSON into structured data.
pub fn parse_cargo_metadata(json: &str) -> Result<CargoMetadata, String> {
    serde_json::from_str(json).map_err(|e| format!("failed to parse cargo metadata: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_metadata() {
        let json = r#"{
            "packages": [],
            "workspace_members": [],
            "workspace_root": "/tmp/ws",
            "resolve": null,
            "target_directory": "/tmp/ws/target",
            "version": 1,
            "workspace_default_members": []
        }"#;
        let meta = parse_cargo_metadata(json).unwrap();
        assert!(meta.packages.is_empty());
        assert!(meta.workspace_members.is_empty());
        assert_eq!(meta.workspace_root, "/tmp/ws");
        assert!(meta.resolve.is_none());
    }

    #[test]
    fn parse_single_package() {
        let json = r#"{
            "packages": [{
                "name": "my-crate",
                "version": "0.1.0",
                "dependencies": [
                    {"name": "serde", "kind": null},
                    {"name": "tokio", "kind": "dev"}
                ],
                "targets": [
                    {"name": "my-crate", "kind": ["lib"]},
                    {"name": "my-bin", "kind": ["bin"]}
                ],
                "features": {
                    "default": ["std"],
                    "std": [],
                    "async": ["tokio"]
                },
                "manifest_path": "/tmp/ws/Cargo.toml"
            }],
            "workspace_members": ["my-crate 0.1.0 (path+file:///tmp/ws)"],
            "workspace_root": "/tmp/ws",
            "resolve": {
                "nodes": [{
                    "id": "my-crate 0.1.0",
                    "deps": [
                        {"name": "serde", "pkg": "serde 1.0.0"}
                    ]
                }]
            },
            "target_directory": "/tmp/ws/target",
            "version": 1
        }"#;
        let meta = parse_cargo_metadata(json).unwrap();
        assert_eq!(meta.packages.len(), 1);
        assert_eq!(meta.packages[0].name, "my-crate");
        assert_eq!(meta.packages[0].dependencies.len(), 2);
        assert_eq!(meta.packages[0].targets.len(), 2);
        assert_eq!(meta.packages[0].features.len(), 3);
        assert!(meta.resolve.is_some());
        let resolve = meta.resolve.unwrap();
        assert_eq!(resolve.nodes.len(), 1);
        assert_eq!(resolve.nodes[0].deps.len(), 1);
    }

    #[test]
    fn parse_error_on_invalid_json() {
        let result = parse_cargo_metadata("not json");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to parse cargo metadata"));
    }
}
