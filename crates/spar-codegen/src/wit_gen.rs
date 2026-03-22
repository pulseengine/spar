//! WIT interface definition generation from AADL process instances.
//!
//! For each AADL process component, generates a `.wit` file that describes
//! the process's ports as WIT imports/exports, following the WASI Component
//! Model conventions.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};

use crate::{GeneratedFile, sanitize_ident};

/// Generate a WIT file for a process instance.
pub fn generate_wit(inst: &SystemInstance, proc_idx: ComponentInstanceIdx) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    let name = sanitize_ident(comp.name.as_str());
    let pkg_name = sanitize_ident(comp.package.as_str());

    let mut wit = String::new();
    wit.push_str(&format!(
        "// Generated from AADL process: {}::{}\n",
        comp.package, comp.name
    ));
    wit.push_str(&format!("package {pkg_name}:{name};\n\n"));

    // Collect child threads to generate interfaces
    let child_threads: Vec<_> = comp
        .children
        .iter()
        .filter(|&&child_idx| inst.component(child_idx).category == ComponentCategory::Thread)
        .collect();

    // Generate an interface for the process's own ports
    wit.push_str(&format!("interface {name}-ports {{\n"));

    for &fi in &comp.features {
        let feat = &inst.features[fi];
        let feat_name = sanitize_ident(feat.name.as_str());
        let type_name = feat
            .classifier
            .as_ref()
            .map(|c| sanitize_ident(&c.to_string()))
            .unwrap_or_else(|| "bytes".to_string());

        match feat.kind {
            FeatureKind::DataPort => {
                let dir = feat.direction.unwrap_or(Direction::In);
                match dir {
                    Direction::In => {
                        wit.push_str(&format!("    {feat_name}: func() -> {type_name};\n"));
                    }
                    Direction::Out => {
                        wit.push_str(&format!(
                            "    set-{feat_name}: func(val: {type_name});\n"
                        ));
                    }
                    Direction::InOut => {
                        wit.push_str(&format!("    {feat_name}: func() -> {type_name};\n"));
                        wit.push_str(&format!(
                            "    set-{feat_name}: func(val: {type_name});\n"
                        ));
                    }
                }
            }
            FeatureKind::EventPort => {
                wit.push_str(&format!("    {feat_name}: func();\n"));
            }
            FeatureKind::EventDataPort => {
                let dir = feat.direction.unwrap_or(Direction::In);
                match dir {
                    Direction::In => {
                        wit.push_str(&format!(
                            "    on-{feat_name}: func() -> option<{type_name}>;\n"
                        ));
                    }
                    Direction::Out => {
                        wit.push_str(&format!(
                            "    emit-{feat_name}: func(val: {type_name});\n"
                        ));
                    }
                    Direction::InOut => {
                        wit.push_str(&format!(
                            "    on-{feat_name}: func() -> option<{type_name}>;\n"
                        ));
                        wit.push_str(&format!(
                            "    emit-{feat_name}: func(val: {type_name});\n"
                        ));
                    }
                }
            }
            _ => {
                wit.push_str(&format!("    // unsupported feature kind: {feat_name}\n"));
            }
        }
    }

    wit.push_str("}\n\n");

    // Generate world
    wit.push_str(&format!("world {name}-world {{\n"));
    wit.push_str(&format!("    import {name}-ports;\n"));

    for &&child_idx in &child_threads {
        let child = inst.component(child_idx);
        let child_name = sanitize_ident(child.name.as_str());
        wit.push_str(&format!("    export {child_name}: func();\n"));
    }

    wit.push_str("}\n");

    GeneratedFile {
        path: format!("wit/{name}.wit"),
        content: wit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::instance::SystemInstance;
    use spar_hir_def::name::Name;
    use spar_hir_def::resolver::GlobalScope;

    fn build_test_instance() -> SystemInstance {
        let aadl = r#"
package TestPkg
public
    process Controller
        features
            sensor_in: in data port;
            cmd_out: out data port;
    end Controller;

    process implementation Controller.Impl
        subcomponents
            ctrl_thread: thread CtrlThread.Impl;
    end Controller.Impl;

    thread CtrlThread
    end CtrlThread;

    thread implementation CtrlThread.Impl
    end CtrlThread.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            ctrl: process Controller.Impl;
    end Top.Impl;
end TestPkg;
"#;

        let db = spar_hir_def::HirDefDatabase::default();
        let sf =
            spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        SystemInstance::instantiate(
            &scope,
            &Name::new("TestPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        )
    }

    #[test]
    fn wit_gen_produces_output() {
        let inst = build_test_instance();
        // Find the process instance
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_wit(&inst, idx);
            assert!(file.path.ends_with(".wit"));
            assert!(file.content.contains("package"));
            assert!(file.content.contains("world"));
        }
    }
}
