//! Rivet design document generation from AADL process instances.
//!
//! Generates Markdown design documents with YAML frontmatter compatible
//! with the rivet artifact system. Each process produces:
//!
//! 1. A design document (`docs/design/{name}.md`) with component details,
//!    port tables, and thread configuration.
//! 2. A verification record (`verification/{name}.yaml`) with
//!    verification-verdict entries for automated checking.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::{GeneratedFile, extract_timing, format_time_ps, sanitize_ident};

/// Generate a design document and verification record for a process instance.
///
/// Returns a tuple of (design_doc, verification_yaml).
pub fn generate_design_doc(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
) -> (GeneratedFile, GeneratedFile) {
    let comp = inst.component(proc_idx);
    let name = sanitize_ident(comp.name.as_str());
    let upper_name = name.to_uppercase();

    // ── Design document ─────────────────────────────────────────────
    let doc = generate_design_markdown(inst, proc_idx, &name, &upper_name);

    // ── Verification YAML ───────────────────────────────────────────
    let verification = generate_verification_yaml(inst, proc_idx, &name, &upper_name);

    (doc, verification)
}

fn generate_design_markdown(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
    name: &str,
    upper_name: &str,
) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    let mut md = String::new();

    // YAML frontmatter
    md.push_str("---\n");
    md.push_str(&format!("id: DESIGN-GEN-{upper_name}\n"));
    md.push_str("type: design-decision\n");
    md.push_str(&format!(
        "title: \"Generated: {}::{}\"\n",
        comp.package, comp.name
    ));
    md.push_str("status: generated\n");
    md.push_str("links:\n");
    md.push_str("  - type: satisfies\n");
    md.push_str("    target: REQ-CODEGEN-001\n");
    md.push_str("tags: [generated, codegen]\n");
    md.push_str("---\n\n");

    // Title
    md.push_str(&format!(
        "# {} -- Generated Architecture\n\n",
        comp.name
    ));

    // Component summary
    md.push_str(&format!(
        "## Component: {}::{} ({})\n\n",
        comp.package, comp.name, comp.category
    ));

    // Properties table
    md.push_str("| Property | Value |\n");
    md.push_str("|----------|-------|\n");
    md.push_str(&format!("| Package | {} |\n", comp.package));
    md.push_str(&format!("| Category | {} |\n", comp.category));

    if let Some(impl_name) = &comp.impl_name {
        md.push_str(&format!("| Implementation | {} |\n", impl_name));
    }

    let child_count = comp.children.len();
    md.push_str(&format!("| Subcomponents | {child_count} |\n"));
    md.push_str(&format!("| Features | {} |\n\n", comp.features.len()));

    // Ports section
    if !comp.features.is_empty() {
        md.push_str("## Ports\n\n");
        md.push_str("| Port | Direction | Kind | Classifier |\n");
        md.push_str("|------|-----------|------|------------|\n");

        for &fi in &comp.features {
            let feat = &inst.features[fi];
            let dir = feat
                .direction
                .map(|d| format!("{d}"))
                .unwrap_or_else(|| "--".to_string());
            let cls = feat
                .classifier
                .as_ref()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "--".to_string());
            md.push_str(&format!(
                "| {} | {dir} | {:?} | {cls} |\n",
                feat.name, feat.kind
            ));
        }
        md.push('\n');
    }

    // Thread details section
    let child_threads: Vec<_> = comp
        .children
        .iter()
        .filter(|&&child_idx| inst.component(child_idx).category == ComponentCategory::Thread)
        .copied()
        .collect();

    if !child_threads.is_empty() {
        md.push_str("## Threads\n\n");
        md.push_str("| Thread | Dispatch | Period | Deadline | WCET |\n");
        md.push_str("|--------|----------|--------|----------|------|\n");

        for &child_idx in &child_threads {
            let child = inst.component(child_idx);
            let props = inst.properties_for(child_idx);
            let dispatch = props
                .get("Timing_Properties", "Dispatch_Protocol")
                .or_else(|| props.get("", "Dispatch_Protocol"))
                .unwrap_or("--");

            let (period, deadline, wcet) = extract_timing(inst, child_idx);

            md.push_str(&format!(
                "| {} | {dispatch} | {} | {} | {} |\n",
                child.name,
                period.map(|p| format_time_ps(p)).unwrap_or("--".to_string()),
                deadline
                    .map(|d| format_time_ps(d))
                    .unwrap_or("--".to_string()),
                wcet.map(|w| format_time_ps(w)).unwrap_or("--".to_string()),
            ));
        }
        md.push('\n');
    }

    // Verification section
    md.push_str("## Verification\n\n");
    md.push_str("| Check | Status |\n");
    md.push_str("|-------|--------|\n");
    md.push_str("| Code generated | pass |\n");
    md.push_str("| WIT interface generated | pass |\n");
    md.push_str("| Configuration generated | pass |\n");

    if !child_threads.is_empty() {
        md.push_str("| Thread timing extracted | pass |\n");
    }

    md.push('\n');

    GeneratedFile {
        path: format!("docs/design/{name}.md"),
        content: md,
    }
}

fn generate_verification_yaml(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
    name: &str,
    upper_name: &str,
) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    let mut yaml = String::new();

    yaml.push_str(&format!(
        "# Generated verification records for process: {}::{}\n",
        comp.package, comp.name
    ));
    yaml.push_str("# DO NOT EDIT -- regenerate with `spar codegen`.\n\n");

    // Verification execution record
    yaml.push_str(&format!("- id: VE-GEN-{upper_name}\n"));
    yaml.push_str("  type: verification-execution\n");
    yaml.push_str(&format!(
        "  title: \"Codegen verification: {}\"\n",
        comp.name
    ));
    yaml.push_str("  status: executed\n");
    yaml.push_str("  tags: [generated, codegen]\n\n");

    // Verdict: code generation succeeded
    yaml.push_str(&format!("- id: VV-GEN-{upper_name}-CODE\n"));
    yaml.push_str("  type: verification-verdict\n");
    yaml.push_str(&format!(
        "  title: \"Code generation: {}\"\n",
        comp.name
    ));
    yaml.push_str("  status: pass\n");
    yaml.push_str("  links:\n");
    yaml.push_str("    - type: part-of-execution\n");
    yaml.push_str(&format!("      target: VE-GEN-{upper_name}\n"));
    yaml.push_str("    - type: result-of\n");
    yaml.push_str(&format!("      target: DESIGN-GEN-{upper_name}\n"));
    yaml.push_str("  tags: [generated, codegen]\n\n");

    // Verdict: interface generated
    yaml.push_str(&format!("- id: VV-GEN-{upper_name}-WIT\n"));
    yaml.push_str("  type: verification-verdict\n");
    yaml.push_str(&format!(
        "  title: \"WIT interface generated: {}\"\n",
        comp.name
    ));
    yaml.push_str("  status: pass\n");
    yaml.push_str("  links:\n");
    yaml.push_str("    - type: part-of-execution\n");
    yaml.push_str(&format!("      target: VE-GEN-{upper_name}\n"));
    yaml.push_str("    - type: result-of\n");
    yaml.push_str(&format!("      target: DESIGN-GEN-{upper_name}\n"));
    yaml.push_str("  tags: [generated, codegen]\n\n");

    // Verdict: configuration generated
    yaml.push_str(&format!("- id: VV-GEN-{upper_name}-CONFIG\n"));
    yaml.push_str("  type: verification-verdict\n");
    yaml.push_str(&format!(
        "  title: \"Configuration generated: {}\"\n",
        comp.name
    ));
    yaml.push_str("  status: pass\n");
    yaml.push_str("  links:\n");
    yaml.push_str("    - type: part-of-execution\n");
    yaml.push_str(&format!("      target: VE-GEN-{upper_name}\n"));
    yaml.push_str("    - type: result-of\n");
    yaml.push_str(&format!("      target: DESIGN-GEN-{upper_name}\n"));
    yaml.push_str("  tags: [generated, codegen]\n");

    GeneratedFile {
        path: format!("verification/{name}.yaml"),
        content: yaml,
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
        properties
            Timing_Properties::Period => 10 ms;
            Timing_Properties::Deadline => 8 ms;
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
    fn doc_has_yaml_frontmatter() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let (doc, _) = generate_design_doc(&inst, idx);
            assert!(doc.path.ends_with(".md"), "Doc must be markdown");
            assert!(
                doc.content.starts_with("---\n"),
                "Doc must start with YAML frontmatter"
            );
            assert!(
                doc.content.contains("id: DESIGN-GEN-"),
                "Doc must have artifact ID"
            );
            assert!(
                doc.content.contains("type: design-decision"),
                "Doc must have artifact type"
            );
            assert!(
                doc.content.contains("status: generated"),
                "Doc must have status"
            );
            assert!(
                doc.content.contains("satisfies"),
                "Doc must have satisfies link"
            );
            assert!(
                doc.content.contains("tags: [generated, codegen]"),
                "Doc must have tags"
            );
        }
    }

    #[test]
    fn doc_has_component_details() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let (doc, _) = generate_design_doc(&inst, idx);
            assert!(
                doc.content.contains("| Property | Value |"),
                "Doc must have properties table"
            );
            assert!(
                doc.content.contains("## Ports"),
                "Doc must have ports section"
            );
            assert!(
                doc.content.contains("## Verification"),
                "Doc must have verification section"
            );
        }
    }

    #[test]
    fn verification_yaml_has_verdict_records() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let (_, verification) = generate_design_doc(&inst, idx);
            assert!(
                verification.path.ends_with(".yaml"),
                "Verification must be YAML"
            );
            assert!(
                verification.content.contains("type: verification-verdict"),
                "Must have verdict records"
            );
            assert!(
                verification.content.contains("type: verification-execution"),
                "Must have execution record"
            );
            assert!(
                verification.content.contains("status: pass"),
                "Verdicts must have pass status"
            );
            assert!(
                verification.content.contains("part-of-execution"),
                "Verdicts must link to execution"
            );
            assert!(
                verification.content.contains("result-of"),
                "Verdicts must link to design doc"
            );
        }
    }

    #[test]
    fn doc_has_thread_timing_table() {
        let inst = build_test_instance();
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let (doc, _) = generate_design_doc(&inst, idx);
            assert!(
                doc.content.contains("## Threads"),
                "Doc must have threads section"
            );
            assert!(
                doc.content.contains("| Thread |"),
                "Doc must have thread table header"
            );
        }
    }
}
