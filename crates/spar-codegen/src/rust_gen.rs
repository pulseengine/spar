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
        .get("Timing_Properties", "Dispatch_Protocol")
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

    // Component trait
    code.push_str(&format!(
        "/// Dispatch trait for the {name} thread ({dispatch}).\n"
    ));
    code.push_str(&format!("pub trait {struct_name}Component {{\n"));
    code.push_str("    /// Called once at initialization.\n");
    code.push_str(&format!(
        "    fn initialize(&mut self, ports: &mut {struct_name}Ports);\n\n"
    ));
    code.push_str(&format!("    /// Called on each dispatch ({dispatch}).\n"));
    code.push_str(&format!(
        "    fn compute(&mut self, ports: &mut {struct_name}Ports);\n\n"
    ));
    code.push_str("    /// Called on finalization.\n");
    code.push_str(&format!(
        "    fn finalize(&mut self, ports: &mut {struct_name}Ports);\n"
    ));
    code.push_str("}\n\n");

    // Skeleton implementation
    code.push_str("/// Default implementation skeleton.\n");
    code.push_str(&format!("pub struct {struct_name}Default;\n\n"));
    code.push_str(&format!(
        "impl {struct_name}Component for {struct_name}Default {{\n"
    ));
    code.push_str(&format!(
        "    fn initialize(&mut self, _ports: &mut {struct_name}Ports) {{\n"
    ));
    code.push_str("        // TODO: initialization logic\n");
    code.push_str("    }\n\n");
    code.push_str(&format!(
        "    fn compute(&mut self, _ports: &mut {struct_name}Ports) {{\n"
    ));
    code.push_str("        // TODO: periodic compute logic\n");
    code.push_str("    }\n\n");
    code.push_str(&format!(
        "    fn finalize(&mut self, _ports: &mut {struct_name}Ports) {{\n"
    ));
    code.push_str("        // TODO: finalization logic\n");
    code.push_str("    }\n");
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
