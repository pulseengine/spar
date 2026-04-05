//! Workspace generation: Cargo.toml + BUILD.bazel files.
//!
//! Generates a Rust workspace layout with:
//! - Root `Cargo.toml` with all process crates as workspace members
//! - Per-crate `Cargo.toml` with correct dependencies
//! - Root `BUILD.bazel` (basic, for future Bazel integration)
//! - Per-crate `BUILD.bazel` with `rust_library` + `wasm_component` targets

use crate::GeneratedFile;

/// Sanitize a name for safe use as a path component.
///
/// Strips any characters that are not alphanumeric, `_`, or `-`.
/// This prevents path traversal attacks (e.g., `../` in a process name)
/// and ensures names are safe for use in file paths.
fn sanitize_path_component(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if s.is_empty() {
        "unnamed".to_string()
    } else {
        s
    }
}

/// Generate all workspace files.
///
/// Returns a list of generated files including:
/// - `Cargo.toml` (workspace root)
/// - `BUILD.bazel` (workspace root)
/// - `crates/{name}/Cargo.toml` (per-crate)
/// - `crates/{name}/BUILD.bazel` (per-crate)
pub fn generate_workspace(root_name: &str, process_names: &[String]) -> Vec<GeneratedFile> {
    let mut files = Vec::new();

    // Root Cargo.toml
    files.push(generate_workspace_cargo_toml(root_name, process_names));

    // Root BUILD.bazel
    files.push(generate_workspace_build_bazel(root_name));

    // Per-crate files
    for name in process_names {
        files.push(generate_crate_cargo_toml(name));
        files.push(generate_crate_build_bazel(name));
        files.push(generate_crate_lib_rs(name));
    }

    files
}

fn generate_workspace_cargo_toml(_root_name: &str, process_names: &[String]) -> GeneratedFile {
    let mut toml = String::new();

    toml.push_str("# Generated workspace for AADL system.\n");
    toml.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");
    toml.push_str("[workspace]\n");
    toml.push_str("resolver = \"2\"\n");
    toml.push_str("members = [\n");

    for name in process_names {
        let safe = sanitize_path_component(name);
        toml.push_str(&format!("    \"crates/{safe}\",\n"));
    }

    toml.push_str("]\n\n");

    toml.push_str("[workspace.package]\n");
    toml.push_str("version = \"0.1.0\"\n");
    toml.push_str("edition = \"2024\"\n\n");

    toml.push_str("[workspace.dependencies]\n");
    toml.push_str("wit-bindgen = \"0.36\"\n");
    toml.push_str("serde = { version = \"1\", features = [\"derive\"] }\n");
    toml.push_str("serde_json = \"1\"\n");
    toml.push_str("toml = \"0.8\"\n");

    for name in process_names {
        let safe = sanitize_path_component(name);
        toml.push_str(&format!("{safe} = {{ path = \"crates/{safe}\" }}\n"));
    }

    GeneratedFile {
        path: "Cargo.toml".to_string(),
        content: toml,
    }
}

fn generate_workspace_build_bazel(root_name: &str) -> GeneratedFile {
    let mut bazel = String::new();

    bazel.push_str("# Generated BUILD.bazel for AADL system.\n");
    bazel.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    bazel.push_str("load(\"@rules_rust//rust:defs.bzl\", \"rust_binary\", \"rust_library\")\n");
    bazel.push_str("load(\"//tools/bazel/rules_wasm_component:defs.bzl\", \"wasm_component\")\n");
    bazel.push_str("load(\"//tools/bazel/rules_verus:defs.bzl\", \"verus_verify\")\n");
    bazel.push_str(
        "load(\"//tools/bazel/rules_lean:defs.bzl\", \"lean_library\", \"lean_verify\")\n\n",
    );

    bazel.push_str(&format!("# Root package: {root_name}\n"));
    bazel.push_str("package(default_visibility = [\"//visibility:public\"])\n\n");

    bazel.push_str("filegroup(\n");
    bazel.push_str("    name = \"all_sources\",\n");
    bazel.push_str("    srcs = glob([\"crates/**/*.rs\"]),\n");
    bazel.push_str(")\n");

    GeneratedFile {
        path: "BUILD.bazel".to_string(),
        content: bazel,
    }
}

fn generate_crate_cargo_toml(name: &str) -> GeneratedFile {
    let safe = sanitize_path_component(name);
    let mut toml = String::new();

    toml.push_str(&format!("# Generated crate for AADL process: {safe}\n"));
    toml.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    toml.push_str("[package]\n");
    toml.push_str(&format!("name = \"{safe}\"\n"));
    toml.push_str("version.workspace = true\n");
    toml.push_str("edition.workspace = true\n\n");

    toml.push_str("[lib]\n");
    toml.push_str("crate-type = [\"cdylib\", \"rlib\"]\n\n");

    toml.push_str("[dependencies]\n");
    toml.push_str("wit-bindgen.workspace = true\n");
    toml.push_str("serde.workspace = true\n");
    toml.push_str("serde_json.workspace = true\n\n");

    toml.push_str("[dev-dependencies]\n");

    GeneratedFile {
        path: format!("crates/{safe}/Cargo.toml"),
        content: toml,
    }
}

fn generate_crate_build_bazel(name: &str) -> GeneratedFile {
    let safe = sanitize_path_component(name);
    let mut bazel = String::new();

    bazel.push_str(&format!("# Generated BUILD.bazel for process: {safe}\n"));
    bazel.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    bazel.push_str("load(\"@rules_rust//rust:defs.bzl\", \"rust_library\", \"rust_test\")\n");
    bazel.push_str("load(\"//tools/bazel/rules_wasm_component:defs.bzl\", \"wasm_component\")\n\n");

    bazel.push_str("package(default_visibility = [\"//visibility:public\"])\n\n");

    // rust_library target
    bazel.push_str("rust_library(\n");
    bazel.push_str(&format!("    name = \"{safe}\",\n"));
    bazel.push_str("    srcs = glob([\"src/**/*.rs\"]),\n");
    bazel.push_str("    deps = [\n");
    bazel.push_str("        \"@crates//:wit-bindgen\",\n");
    bazel.push_str("        \"@crates//:serde\",\n");
    bazel.push_str("        \"@crates//:serde_json\",\n");
    bazel.push_str("    ],\n");
    bazel.push_str(")\n\n");

    // wasm_component target
    bazel.push_str("wasm_component(\n");
    bazel.push_str(&format!("    name = \"{safe}_component\",\n"));
    bazel.push_str(&format!("    lib = \":{safe}\",\n"));
    bazel.push_str(&format!("    wit = \"//wit:{safe}.wit\",\n"));
    bazel.push_str("    target = \"wasm32-wasip2\",\n");
    bazel.push_str(")\n\n");

    // rust_test target
    bazel.push_str("rust_test(\n");
    bazel.push_str(&format!("    name = \"{safe}_test\",\n"));
    bazel.push_str(&format!("    crate = \":{safe}\",\n"));
    bazel.push_str(")\n");

    GeneratedFile {
        path: format!("crates/{safe}/BUILD.bazel"),
        content: bazel,
    }
}

fn generate_crate_lib_rs(name: &str) -> GeneratedFile {
    let safe = sanitize_path_component(name);
    let mut code = String::new();

    code.push_str(&format!(
        "//! Generated component crate for AADL process: {safe}\n"
    ));
    code.push_str("//! DO NOT EDIT -- regenerate with `spar codegen`.\n\n");
    code.push_str("// Component modules will be generated here by spar codegen.\n");
    code.push_str("// Each thread becomes a submodule with port types and dispatch trait.\n");

    GeneratedFile {
        path: format!("crates/{safe}/src/lib.rs"),
        content: code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_cargo_toml_has_members() {
        let files = generate_workspace(
            "test_system",
            &["controller".to_string(), "monitor".to_string()],
        );

        let cargo_toml = files.iter().find(|f| f.path == "Cargo.toml").unwrap();
        assert!(
            cargo_toml.content.contains("[workspace]"),
            "Must have workspace section"
        );
        assert!(
            cargo_toml.content.contains("members = ["),
            "Must have members list"
        );
        assert!(
            cargo_toml.content.contains("\"crates/controller\""),
            "Must include controller crate"
        );
        assert!(
            cargo_toml.content.contains("\"crates/monitor\""),
            "Must include monitor crate"
        );
        assert!(
            cargo_toml.content.contains("[workspace.dependencies]"),
            "Must have workspace dependencies"
        );
    }

    #[test]
    fn workspace_build_bazel_has_targets() {
        let files = generate_workspace("test_system", &["controller".to_string()]);

        let build_bazel = files.iter().find(|f| f.path == "BUILD.bazel").unwrap();
        assert!(
            build_bazel.content.contains("rules_rust"),
            "Must load rules_rust"
        );
        assert!(
            build_bazel.content.contains("filegroup"),
            "Must have filegroup target"
        );
    }

    #[test]
    fn workspace_build_bazel_loads_verification_rules() {
        let files = generate_workspace("test_system", &["controller".to_string()]);

        let build_bazel = files.iter().find(|f| f.path == "BUILD.bazel").unwrap();
        assert!(
            build_bazel
                .content
                .contains("//tools/bazel/rules_wasm_component:defs.bzl"),
            "Must load wasm_component from tools/bazel"
        );
        assert!(
            build_bazel
                .content
                .contains("//tools/bazel/rules_verus:defs.bzl"),
            "Must load verus rules from tools/bazel"
        );
        assert!(
            build_bazel
                .content
                .contains("//tools/bazel/rules_lean:defs.bzl"),
            "Must load lean rules from tools/bazel"
        );
    }

    #[test]
    fn crate_cargo_toml_has_deps() {
        let files = generate_workspace("test_system", &["controller".to_string()]);

        let crate_toml = files
            .iter()
            .find(|f| f.path == "crates/controller/Cargo.toml")
            .unwrap();
        assert!(
            crate_toml.content.contains("[package]"),
            "Must have package section"
        );
        assert!(
            crate_toml.content.contains("name = \"controller\""),
            "Must have correct package name"
        );
        assert!(
            crate_toml.content.contains("cdylib"),
            "Must be a cdylib for WASM"
        );
        assert!(
            crate_toml.content.contains("[dependencies]"),
            "Must have dependencies"
        );
        assert!(
            crate_toml.content.contains("wit-bindgen"),
            "Must depend on wit-bindgen"
        );
    }

    #[test]
    fn crate_build_bazel_has_targets() {
        let files = generate_workspace("test_system", &["controller".to_string()]);

        let build = files
            .iter()
            .find(|f| f.path == "crates/controller/BUILD.bazel")
            .unwrap();
        assert!(
            build.content.contains("rust_library"),
            "Must have rust_library target"
        );
        assert!(
            build.content.contains("wasm_component"),
            "Must have wasm_component target"
        );
        assert!(
            build.content.contains("rust_test"),
            "Must have rust_test target"
        );
        assert!(
            build.content.contains("wasm32-wasip2"),
            "Must target wasm32-wasip2"
        );
        assert!(
            build
                .content
                .contains("//tools/bazel/rules_wasm_component:defs.bzl"),
            "Must load wasm_component from tools/bazel, not @rules_rust//wasm_bindgen"
        );
    }

    #[test]
    fn crate_lib_rs_generated() {
        let files = generate_workspace("test_system", &["controller".to_string()]);

        let lib_rs = files
            .iter()
            .find(|f| f.path == "crates/controller/src/lib.rs")
            .unwrap();
        assert!(
            lib_rs.content.contains("Generated component crate"),
            "Must have generated header"
        );
    }

    #[test]
    fn empty_process_list_produces_workspace() {
        let files = generate_workspace("test_system", &[]);
        // Should still have workspace Cargo.toml and BUILD.bazel
        assert!(files.iter().any(|f| f.path == "Cargo.toml"));
        assert!(files.iter().any(|f| f.path == "BUILD.bazel"));
    }

    #[test]
    fn sanitize_path_component_strips_traversal() {
        assert_eq!(sanitize_path_component("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_path_component(".."), "unnamed");
        assert_eq!(sanitize_path_component("foo/../bar"), "foobar");
        assert_eq!(sanitize_path_component("a/b"), "ab");
        assert_eq!(sanitize_path_component("a\\b"), "ab");
    }

    #[test]
    fn sanitize_path_component_preserves_safe_chars() {
        assert_eq!(sanitize_path_component("controller"), "controller");
        assert_eq!(sanitize_path_component("my-process"), "my-process");
        assert_eq!(sanitize_path_component("my_process"), "my_process");
        assert_eq!(sanitize_path_component("ctrl123"), "ctrl123");
    }

    #[test]
    fn sanitize_path_component_empty_input() {
        assert_eq!(sanitize_path_component(""), "unnamed");
        assert_eq!(sanitize_path_component("..."), "unnamed");
        assert_eq!(sanitize_path_component("///"), "unnamed");
    }

    #[test]
    fn path_traversal_name_produces_safe_paths() {
        let files = generate_workspace("test_system", &["../../../etc/passwd".to_string()]);

        // No generated file path should contain ".." or "/"
        // (other than the expected directory separators in the template)
        for file in &files {
            assert!(
                !file.path.contains(".."),
                "Path must not contain '..': {}",
                file.path
            );
            // The original slash-separated traversal path must not survive
            assert!(
                !file.path.contains("etc/passwd"),
                "Raw traversal path must not appear: {}",
                file.path
            );
        }

        // The sanitized name "etcpasswd" (dots and slashes stripped) should be used
        assert!(
            files
                .iter()
                .any(|f| f.path == "crates/etcpasswd/Cargo.toml"),
            "Sanitized crate path expected"
        );
    }
}
