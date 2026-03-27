//! TOML configuration file generation from AADL process instances.
//!
//! Generates deployment configuration files that capture the timing
//! properties, thread dispatch protocols, and port configurations
//! extracted from the AADL model.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::{GeneratedFile, extract_timing, format_time_ps, sanitize_ident};

/// Generate a TOML configuration file for a process instance.
pub fn generate_config(inst: &SystemInstance, proc_idx: ComponentInstanceIdx) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    let name = sanitize_ident(comp.name.as_str());

    let mut toml = String::new();
    toml.push_str(&format!(
        "# Generated from AADL process: {}::{}\n",
        comp.package, comp.name
    ));
    toml.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    toml.push_str("[process]\n");
    toml.push_str(&format!("name = \"{}\"\n", comp.name));
    toml.push_str(&format!("package = \"{}\"\n", comp.package));
    toml.push_str(&format!("category = \"{}\"\n\n", comp.category));

    // Thread configurations
    for &child_idx in &comp.children {
        let child = inst.component(child_idx);
        if child.category != ComponentCategory::Thread {
            continue;
        }

        let thread_name = sanitize_ident(child.name.as_str());
        toml.push_str("[[threads]]\n");
        toml.push_str(&format!("name = \"{}\"\n", child.name));

        let props = inst.properties_for(child_idx);
        let dispatch = props
            .get("Timing_Properties", "Dispatch_Protocol")
            .or_else(|| props.get("", "Dispatch_Protocol"))
            .unwrap_or("Periodic");
        toml.push_str(&format!("dispatch = \"{dispatch}\"\n"));

        let (period, deadline, wcet) = extract_timing(inst, child_idx);

        if let Some(p) = period {
            toml.push_str(&format!("period = \"{}\"\n", format_time_ps(p)));
        }
        if let Some(d) = deadline {
            toml.push_str(&format!("deadline = \"{}\"\n", format_time_ps(d)));
        }
        if let Some(w) = wcet {
            toml.push_str(&format!("wcet = \"{}\"\n", format_time_ps(w)));
        }

        // Port listing
        let thread_features: Vec<_> = child
            .features
            .iter()
            .map(|&fi| &inst.features[fi])
            .collect();

        if !thread_features.is_empty() {
            toml.push_str(&format!("\n[threads.{thread_name}.ports]\n"));
            for feat in &thread_features {
                let dir = feat.direction.map(|d| format!("{d}")).unwrap_or_default();
                toml.push_str(&format!(
                    "{} = {{ kind = \"{:?}\", direction = \"{dir}\" }}\n",
                    sanitize_ident(feat.name.as_str()),
                    feat.kind,
                ));
            }
        }

        toml.push('\n');
    }

    GeneratedFile {
        path: format!("config/{name}.toml"),
        content: toml,
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
    end Controller;

    process implementation Controller.Impl
        subcomponents
            ctrl_thread: thread CtrlThread.Impl;
    end Controller.Impl;

    thread CtrlThread
        properties
            Timing_Properties::Dispatch_Protocol => Periodic;
            Timing_Properties::Period => 10 ms;
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
        let sf = spar_base_db::SourceFile::new(&db, "test.aadl".to_string(), aadl.to_string());
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
    fn config_gen_produces_toml() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_config(&inst, idx);
            assert!(file.path.ends_with(".toml"));
            assert!(file.content.contains("[process]"));
            assert!(file.content.contains("[[threads]]"));
        }
    }
}
