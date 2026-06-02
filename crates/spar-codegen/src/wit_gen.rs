//! WIT interface definition generation from AADL process instances.
//!
//! For each AADL process component, generates a `.wit` file that describes
//! the process's ports as WIT imports/exports, following the WASI Component
//! Model conventions.

use std::collections::BTreeSet;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};

use crate::GeneratedFile;

/// Default WIT representation for an AADL data type with no further detail.
/// A byte buffer is always valid and bindable; `bytes` is NOT a WIT primitive.
const DEFAULT_WIT_TYPE: &str = "list<u8>";

/// Generate a WIT file for a process instance.
///
/// Identifiers are emitted in WIT kebab-case (see [`wit_ident`]) — NOT the
/// Rust `snake_case` used elsewhere in codegen — and every named data type a
/// port references is emitted as a `type` alias so the interface resolves
/// under `wasm-tools` / `wit-bindgen` (see issue #254).
pub fn generate_wit(inst: &SystemInstance, proc_idx: ComponentInstanceIdx) -> GeneratedFile {
    let comp = inst.component(proc_idx);
    let name = wit_ident(comp.name.as_str());
    let pkg_name = wit_ident(comp.package.as_str());

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

    // First pass: collect every named data type referenced by a port so we can
    // emit `type` definitions. WIT rejects references to undefined types, so an
    // interface that names `mattermessage` without defining it will not bind.
    let mut referenced_types: BTreeSet<String> = BTreeSet::new();
    for &fi in &comp.features {
        let feat = &inst.features[fi];
        if let Some(c) = feat.classifier.as_ref() {
            referenced_types.insert(wit_ident(&c.to_string()));
        }
    }

    // Generate an interface for the process's own ports
    wit.push_str(&format!("interface {name}-ports {{\n"));

    // Emit a type alias for each referenced data type. Without per-type
    // `Data_Size` resolution (the generator has no scope handle here) every
    // AADL data type maps to a byte buffer — always valid and bindable.
    // Refining scalar sizes (8 bytes -> u64, etc.) is tracked as a follow-up.
    for ty in &referenced_types {
        wit.push_str(&format!("    type {ty} = {DEFAULT_WIT_TYPE};\n"));
    }
    if !referenced_types.is_empty() {
        wit.push('\n');
    }

    for &fi in &comp.features {
        let feat = &inst.features[fi];
        let feat_name = wit_ident(feat.name.as_str());
        let type_name = feat
            .classifier
            .as_ref()
            .map(|c| wit_ident(&c.to_string()))
            .unwrap_or_else(|| DEFAULT_WIT_TYPE.to_string());

        match feat.kind {
            FeatureKind::DataPort => {
                let dir = feat.direction.unwrap_or(Direction::In);
                match dir {
                    Direction::In => {
                        wit.push_str(&format!("    {feat_name}: func() -> {type_name};\n"));
                    }
                    Direction::Out => {
                        wit.push_str(&format!("    set-{feat_name}: func(val: {type_name});\n"));
                    }
                    Direction::InOut => {
                        wit.push_str(&format!("    {feat_name}: func() -> {type_name};\n"));
                        wit.push_str(&format!("    set-{feat_name}: func(val: {type_name});\n"));
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
                        wit.push_str(&format!("    emit-{feat_name}: func(val: {type_name});\n"));
                    }
                    Direction::InOut => {
                        wit.push_str(&format!(
                            "    on-{feat_name}: func() -> option<{type_name}>;\n"
                        ));
                        wit.push_str(&format!("    emit-{feat_name}: func(val: {type_name});\n"));
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
        let child_name = wit_ident(child.name.as_str());
        wit.push_str(&format!("    export {child_name}: func();\n"));
    }

    wit.push_str("}\n");

    GeneratedFile {
        path: format!("wit/{name}.wit"),
        content: wit,
    }
}

/// WIT reserved words that must be `%`-escaped when used as identifiers.
/// (A representative set covering the WIT keywords an AADL-derived name could
/// realistically collide with.)
const WIT_KEYWORDS: &[&str] = &[
    "use", "type", "func", "record", "enum", "flags", "variant", "resource", "interface", "world",
    "import", "export", "package", "include", "as", "from", "static", "constructor", "list",
    "option", "result", "tuple", "future", "stream", "bool", "u8", "u16", "u32", "u64", "s8",
    "s16", "s32", "s64", "f32", "f64", "char", "string",
];

/// Sanitize an arbitrary AADL identifier into a valid WIT identifier.
///
/// WIT identifiers are kebab-case (`word ('-' word)*`, each `word` matching
/// `[a-z][a-z0-9]*`), NOT the Rust `snake_case` produced by
/// [`crate::sanitize_ident`]. Underscores and other separators become hyphens,
/// words that would start with a digit are letter-prefixed, and collisions with
/// WIT keywords are `%`-escaped. See issue #254.
fn wit_ident(name: &str) -> String {
    // Split into lowercase alphanumeric words on any run of separators
    // (`_`, `.`, `-`, whitespace, etc.).
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }

    // Each WIT word must start with a letter; prefix a leading-digit word.
    for w in &mut words {
        if w.starts_with(|c: char| c.is_ascii_digit()) {
            w.insert(0, 'n');
        }
    }

    if words.is_empty() {
        return "unnamed".to_string();
    }

    let joined = words.join("-");

    // `%`-escape WIT keywords (only meaningful for single-word identifiers).
    if WIT_KEYWORDS.contains(&joined.as_str()) {
        return format!("%{joined}");
    }
    joined
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
    data Sample
    end Sample;

    process Controller
        features
            sensor_in: in data port;
            cmd_out: out data port;
            message_in: in event data port Sample;
            announce_out: out event data port Sample;
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

    /// Regression guard for #254: the generated WIT must parse + resolve under
    /// `wit-parser` — no invalid identifiers (underscores), no references to
    /// undefined types. Without this, `spar codegen --format wit` could emit
    /// files that only fail later in `wasm-tools` / `wit-bindgen`.
    #[test]
    fn generated_wit_is_valid_and_bindable() {
        let inst = build_test_instance();
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx)
            .expect("test model has a process");
        let file = generate_wit(&inst, idx);

        // No stray underscores in the emitted WIT (kebab-case only). The header
        // comment echoes the AADL names, so check the body after it.
        let body: String = file
            .content
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !body.contains('_'),
            "WIT body must be kebab-case (no underscores):\n{body}"
        );

        // The classifier-typed ports must have produced a `type` definition so
        // the interface resolves rather than referencing an undefined type.
        assert!(
            body.contains("type sample = list<u8>;"),
            "expected a type alias for the `Sample` data type:\n{body}"
        );

        // Authoritative check: feed the WIT to wit-parser. push_str errors on
        // invalid identifiers OR references to undefined types.
        let mut resolve = wit_parser::Resolve::new();
        resolve
            .push_str("generated.wit", &file.content)
            .unwrap_or_else(|e| panic!("generated WIT failed to parse/resolve: {e}\n---\n{}", file.content));
    }

    #[test]
    fn wit_ident_kebab_cases() {
        // The exact identifiers from issue #254.
        assert_eq!(wit_ident("Wohl_Matter"), "wohl-matter");
        assert_eq!(wit_ident("message_in"), "message-in");
        assert_eq!(wit_ident("announce_out"), "announce-out");
        // Dotted + mixed separators collapse to single hyphens.
        assert_eq!(wit_ident("Ctrl.Impl"), "ctrl-impl");
        assert_eq!(wit_ident("My-Thread"), "my-thread");
        // Leading-digit words get a letter prefix (WIT words start with a letter).
        assert_eq!(wit_ident("3d_pos"), "n3d-pos");
        // WIT keywords are %-escaped.
        assert_eq!(wit_ident("type"), "%type");
        assert_eq!(wit_ident("list"), "%list");
        // Degenerate input.
        assert_eq!(wit_ident("___"), "unnamed");
        assert_eq!(wit_ident(""), "unnamed");
    }
}
