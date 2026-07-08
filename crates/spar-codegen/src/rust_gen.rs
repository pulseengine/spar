//! Rust component skeleton generation from AADL thread instances.
//!
//! For each AADL thread component, generates a Rust source file with
//! the appropriate port struct, dispatch loop, and WASI bindings.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{Direction, FeatureKind};

use crate::{GeneratedFile, extract_timing, format_time_ps, sanitize_ident, to_pascal_case};

/// Generate a Rust component skeleton for a thread instance.
pub fn generate_rust_component(
    inst: &SystemInstance,
    thread_idx: ComponentInstanceIdx,
) -> GeneratedFile {
    let comp = inst.component(thread_idx);
    let name = sanitize_ident(comp.name.as_str());
    let struct_name = to_pascal_case(comp.name.as_str());

    let (period, deadline, wcet) = extract_timing(inst, thread_idx);

    let props = inst.properties_for(thread_idx);
    let dispatch = props
        .get("Thread_Properties", "Dispatch_Protocol")
        .or_else(|| props.get("Timing_Properties", "Dispatch_Protocol"))
        .or_else(|| props.get("Deployment_Properties", "Dispatch_Protocol"))
        .or_else(|| props.get("", "Dispatch_Protocol"))
        .unwrap_or("Periodic");

    let mut code = String::new();

    // Header comment
    code.push_str(&format!(
        "//! Generated from AADL thread: {}::{}\n",
        comp.package, comp.name
    ));
    code.push_str("//! DO NOT EDIT — regenerate with `spar codegen`.\n\n");

    // Timing constants
    if let Some(p) = period {
        code.push_str(&format!("/// Thread period: {}\n", format_time_ps(p)));
        code.push_str(&format!("pub const PERIOD_PS: u64 = {p};\n"));
    }
    if let Some(d) = deadline {
        code.push_str(&format!("/// Thread deadline: {}\n", format_time_ps(d)));
        code.push_str(&format!("pub const DEADLINE_PS: u64 = {d};\n"));
    }
    if let Some(w) = wcet {
        code.push_str(&format!(
            "/// Worst-case execution time: {}\n",
            format_time_ps(w)
        ));
        code.push_str(&format!("pub const WCET_PS: u64 = {w};\n"));
    }
    code.push('\n');

    // Port struct
    code.push_str(&format!("/// Port interface for the {name} thread.\n"));
    code.push_str("#[derive(Debug, Default)]\n");
    code.push_str(&format!("pub struct {struct_name}Ports {{\n"));

    for &fi in &comp.features {
        let feat = &inst.features[fi];
        let feat_name = sanitize_ident(feat.name.as_str());
        let rust_type = feature_to_rust_type(feat.kind, &feat.classifier);

        let dir_comment = match feat.direction {
            Some(Direction::In) => "in",
            Some(Direction::Out) => "out",
            Some(Direction::InOut) => "in out",
            None => "",
        };

        code.push_str(&format!(
            "    /// {dir_comment} {kind:?} port\n",
            kind = feat.kind,
        ));
        code.push_str(&format!("    pub {feat_name}: {rust_type},\n"));
    }

    code.push_str("}\n\n");

    // Component trait. The lifecycle methods are derived from the thread's
    // `Dispatch_Protocol` (REQ-CODEGEN-WIT-RECORDS-001, #319 item 7): a Periodic
    // thread exposes only `compute`; every other protocol gets the full
    // `initialize` / `compute` / `finalize`. This matches the WIT world exports
    // and the generated `Guest` impl — all three derive from `crate::Lifecycle`.
    let methods = crate::lifecycle_for(dispatch).methods();

    code.push_str(&format!(
        "/// Dispatch trait for the {name} thread ({dispatch}).\n"
    ));
    code.push_str(&format!("pub trait {struct_name}Component {{\n"));
    for (i, method) in methods.iter().enumerate() {
        code.push_str(&format!("    /// {}\n", lifecycle_doc(method, dispatch)));
        code.push_str(&format!(
            "    fn {method}(&mut self, ports: &mut {struct_name}Ports);\n"
        ));
        if i + 1 < methods.len() {
            code.push('\n');
        }
    }
    code.push_str("}\n\n");

    // Skeleton implementation
    code.push_str("/// Default implementation skeleton.\n");
    code.push_str(&format!("pub struct {struct_name}Default;\n\n"));
    code.push_str(&format!(
        "impl {struct_name}Component for {struct_name}Default {{\n"
    ));
    for (i, method) in methods.iter().enumerate() {
        code.push_str(&format!(
            "    fn {method}(&mut self, _ports: &mut {struct_name}Ports) {{\n"
        ));
        code.push_str(&format!("        // TODO: {method} logic\n"));
        code.push_str("    }\n");
        if i + 1 < methods.len() {
            code.push('\n');
        }
    }
    code.push_str("}\n");

    // Determine process parent name for path
    let parent_name = comp
        .parent
        .map(|p| sanitize_ident(inst.component(p).name.as_str()))
        .unwrap_or_else(|| "unknown".to_string());

    GeneratedFile {
        path: format!("src/{parent_name}/{name}.rs"),
        content: code,
    }
}

/// Doc-comment text for a lifecycle trait method.
fn lifecycle_doc(method: &str, dispatch: &str) -> String {
    match method {
        "initialize" => "Called once at initialization.".to_string(),
        "compute" => format!("Called on each dispatch ({dispatch})."),
        "finalize" => "Called on finalization.".to_string(),
        other => format!("Lifecycle entry point: {other}."),
    }
}

/// Generate the per-process WIT-binding crate root (`crates/{proc}/src/lib.rs`)
/// that wires the AADL process's WIT `world` to Rust via `wit-bindgen`
/// (REQ-CODEGEN-WIT-RECORDS-001, #319 items 4 & 7).
///
/// Emits the `wit_bindgen::generate!` for the process world, a `Component`
/// struct, and an `impl Guest` whose methods are the lifecycle entry points every
/// child thread's `Dispatch_Protocol` implies — the SAME set [`crate::wit_gen`]
/// exports into the world. Because the `Guest` trait `generate!` produces is
/// derived from that world, the Rust compiler (when the crate is built) *enforces*
/// world⟷Guest alignment: a missing or misnamed method is a compile error, not a
/// silent drift (the item-7 failure mode).
///
/// Method bodies are `todo!()` stubs. This unit proves the *interface* wiring
/// compiles and matches the model; the behavior is supplied by the per-thread
/// `{Struct}Component` implementations ([`generate_rust_component`]).
pub fn generate_process_bindings(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    // Crate directory matches workspace_gen's `crates/{sanitize_ident(name)}`.
    let crate_dir = sanitize_ident(comp.name.as_str());
    // World name + wit file name match wit_gen exactly (kebab-case).
    let world_name = crate::wit_gen::wit_ident(comp.name.as_str());

    let child_threads: Vec<_> = comp
        .children
        .iter()
        .filter(|&&child_idx| {
            inst.component(child_idx).category == spar_hir_def::item_tree::ComponentCategory::Thread
        })
        .collect();

    let mut code = String::new();
    code.push_str(&format!(
        "//! Generated WIT bindings for AADL process: {}::{}\n",
        comp.package, comp.name
    ));
    code.push_str("//! DO NOT EDIT — regenerate with `spar codegen`.\n\n");

    // `generate!` loads the single world file emitted by wit_gen. The path is
    // relative to this crate root (`crates/{proc}/`) reaching the top-level
    // `wit/` directory. An unqualified world name resolves because the file
    // declares exactly one package.
    code.push_str("wit_bindgen::generate!({\n");
    code.push_str(&format!("    world: \"{world_name}-world\",\n"));
    code.push_str(&format!("    path: \"../../wit/{world_name}.wit\",\n"));
    code.push_str("});\n\n");

    code.push_str("/// Component entry point, exported to the WIT world.\n");
    code.push_str("struct Component;\n\n");
    code.push_str("impl Guest for Component {\n");
    for &&child_idx in &child_threads {
        let child = inst.component(child_idx);
        let thread_kebab = crate::wit_gen::wit_ident(child.name.as_str());
        let dispatch = crate::dispatch_protocol(inst, child_idx);
        for method in crate::lifecycle_for(&dispatch).methods() {
            // wit-bindgen mangles the kebab WIT export `{thread}-{method}` to the
            // snake Rust method `{thread}_{method}`. Deriving both names from the
            // same `wit_ident` keeps them consistent; the compiler is the final
            // arbiter (see doc comment).
            let rust_method = format!("{thread_kebab}-{method}").replace('-', "_");
            code.push_str(&format!("    fn {rust_method}() {{\n"));
            code.push_str(&format!(
                "        todo!(\"{}::{method}\")\n",
                child.name.as_str()
            ));
            code.push_str("    }\n");
        }
    }
    code.push_str("}\n\n");
    code.push_str("export!(Component);\n");

    GeneratedFile {
        path: format!("crates/{crate_dir}/src/lib.rs"),
        content: code,
    }
}

/// Convert a feature kind + optional classifier to a Rust type.
fn feature_to_rust_type(
    kind: FeatureKind,
    classifier: &Option<spar_hir_def::name::ClassifierRef>,
) -> String {
    let base_type = classifier
        .as_ref()
        .map(|c| to_pascal_case(&c.to_string()))
        .unwrap_or_else(|| "Vec<u8>".to_string());

    match kind {
        FeatureKind::DataPort => base_type,
        FeatureKind::EventPort => "bool".to_string(),
        FeatureKind::EventDataPort => format!("Option<{base_type}>"),
        _ => base_type,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(to_pascal_case("ctrl_thread"), "CtrlThread");
        assert_eq!(to_pascal_case("my-component.impl"), "MyComponentImpl");
        assert_eq!(to_pascal_case("Sensor"), "Sensor");
    }

    #[test]
    fn feature_rust_type_mapping() {
        assert_eq!(feature_to_rust_type(FeatureKind::EventPort, &None), "bool");
        assert_eq!(
            feature_to_rust_type(FeatureKind::DataPort, &None),
            "Vec<u8>"
        );
        assert_eq!(
            feature_to_rust_type(FeatureKind::EventDataPort, &None),
            "Option<Vec<u8>>"
        );
    }
}
