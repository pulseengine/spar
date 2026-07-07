//! WIT interface definition generation from AADL process instances.
//!
//! For each AADL process component, generates a `.wit` file that describes
//! the process's ports as WIT imports/exports, following the WASI Component
//! Model conventions.

use std::collections::BTreeMap;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::{ComponentCategory, Direction, FeatureKind};
use spar_hir_def::name::ClassifierRef;
use spar_hir_def::resolver::{DataField, DataShape, GlobalScope, ScalarKind};

use crate::GeneratedFile;

/// Default WIT representation for an AADL data type with no further detail.
/// A byte buffer is always valid and bindable; `bytes` is NOT a WIT primitive.
const DEFAULT_WIT_TYPE: &str = "list<u8>";

/// The WIT scalar for a power-of-two `Data_Size` in bytes and its numeric
/// representation, or `None` if not directly representable — which would force a
/// heap-allocating `list<u8>` at the ABI boundary (a no_alloc violation). WIT
/// has no f8/f16, so sub-32-bit floats are unrepresentable.
/// REQ-CODEGEN-WIT-RECORDS-001 (#319).
fn scalar_wit(bytes: u64, kind: ScalarKind) -> Option<&'static str> {
    match kind {
        ScalarKind::Float => match bytes {
            4 => Some("f32"),
            8 => Some("f64"),
            _ => None,
        },
        ScalarKind::Signed => match bytes {
            1 => Some("s8"),
            2 => Some("s16"),
            4 => Some("s32"),
            8 => Some("s64"),
            _ => None,
        },
        ScalarKind::Unsigned => match bytes {
            1 => Some("u8"),
            2 => Some("u16"),
            4 => Some("u32"),
            8 => Some("u64"),
            _ => None,
        },
    }
}

/// Resolve a record's fields to WIT, in declaration order. Three outcomes
/// (REQ-CODEGEN-WIT-RECORDS-001, #319):
/// - `Err(msgs)` — one or more fields are genuinely NON-ENCODABLE without heap
///   allocation (a non-power-of-two `Data_Size`, or an opaque type with neither
///   size nor fields). Codegen must refuse (the user-chosen hard-error policy);
///   every offending field is reported.
/// - `Ok(None)` — no hard errors, but a field is a nested record, which is
///   representable but NOT YET emitted (P2 follow-up). Caller keeps the legacy
///   `list<u8>` for the whole type rather than fail on a supportable case.
/// - `Ok(Some(rows))` — every field is a directly representable no_alloc scalar.
fn record_fields(
    record_name: &str,
    fields: &[DataField],
) -> Result<Option<Vec<(String, String)>>, Vec<String>> {
    let mut rows = Vec::with_capacity(fields.len());
    let mut errors = Vec::new();
    let mut has_nested = false;
    for f in fields {
        let field = f.name.as_str();
        match &f.shape {
            DataShape::Scalar { bytes, kind } => match scalar_wit(*bytes, *kind) {
                Some(w) => rows.push((wit_ident(field), w.to_string())),
                None => errors.push(format!(
                    "record `{record_name}` field `{field}`: {bytes}-byte {kind:?} has no \
                     no_alloc WIT scalar (int 1/2/4/8, float 4/8 only)"
                )),
            },
            // Representable in WIT (nested records are no_alloc), just not emitted
            // yet — fall back rather than reject a supportable type.
            DataShape::Record { .. } => has_nested = true,
            DataShape::Opaque => errors.push(format!(
                "record `{record_name}` field `{field}`: type declares no Data_Size and has no \
                 fields — it cannot be encoded without heap allocation"
            )),
        }
    }
    if !errors.is_empty() {
        Err(errors)
    } else if has_nested {
        Ok(None)
    } else {
        Ok(Some(rows))
    }
}

/// Generate a WIT file for a process instance.
///
/// Identifiers are emitted in WIT kebab-case (see [`wit_ident`]) — NOT the
/// Rust `snake_case` used elsewhere in codegen — and every named data type a
/// port references is emitted as a `type` alias so the interface resolves
/// under `wasm-tools` / `wit-bindgen` (see issue #254).
pub fn generate_wit(
    inst: &SystemInstance,
    scope: &GlobalScope,
    proc_idx: ComponentInstanceIdx,
) -> Result<GeneratedFile, crate::CodegenError> {
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

    // First pass: collect every named data type a port references, keyed by its
    // WIT identifier, remembering the classifier so we can resolve its shape.
    // WIT rejects references to undefined types, so every referenced type must
    // be emitted (as a scalar alias, a record, or a byte-buffer fallback).
    let mut referenced: BTreeMap<String, ClassifierRef> = BTreeMap::new();
    for &fi in &comp.features {
        let feat = &inst.features[fi];
        if let Some(cref) = feat.classifier.as_ref() {
            referenced
                .entry(wit_ident(&cref.to_string()))
                .or_insert_with(|| cref.clone());
        }
    }

    // Generate an interface for the process's own ports.
    wit.push_str(&format!("interface {name}-ports {{\n"));

    // Resolve each referenced data type to its shape and emit it. A
    // `data implementation` with scalar subcomponents becomes a WIT `record`
    // (REQ-CODEGEN-WIT-RECORDS-001, #319) instead of an opaque `list<u8>` blob
    // that would force `cabi_realloc` at the ABI boundary. Scalar-sized data
    // types map to precise WIT integers (1 -> u8 ... 8 -> u64).
    let mut errors: Vec<String> = Vec::new();
    for (ty, cref) in &referenced {
        match scope.resolve_data_shape(&comp.package, cref) {
            DataShape::Record { fields } => match record_fields(ty, &fields) {
                Ok(Some(rows)) => {
                    wit.push_str(&format!("    record {ty} {{\n"));
                    for (fname, fty) in rows {
                        wit.push_str(&format!("        {fname}: {fty},\n"));
                    }
                    wit.push_str("    }\n");
                }
                // A nested-record field: representable but not yet emitted —
                // keep the legacy byte-buffer alias (P2 follow-up).
                Ok(None) => wit.push_str(&format!("    type {ty} = {DEFAULT_WIT_TYPE};\n")),
                // A genuinely non-encodable field: refuse (no_alloc is a hard
                // guarantee). Collect every offending field before failing.
                Err(errs) => errors.extend(errs),
            },
            DataShape::Scalar { bytes, kind } => {
                let wit_ty = scalar_wit(bytes, kind).unwrap_or(DEFAULT_WIT_TYPE);
                wit.push_str(&format!("    type {ty} = {wit_ty};\n"));
            }
            DataShape::Opaque => {
                wit.push_str(&format!("    type {ty} = {DEFAULT_WIT_TYPE};\n"));
            }
        }
    }
    if !errors.is_empty() {
        return Err(crate::CodegenError { errors });
    }
    if !referenced.is_empty() {
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

    Ok(GeneratedFile {
        path: format!("wit/{name}.wit"),
        content: wit,
    })
}

/// WIT reserved words that must be `%`-escaped when used as identifiers.
/// (A representative set covering the WIT keywords an AADL-derived name could
/// realistically collide with.)
const WIT_KEYWORDS: &[&str] = &[
    "use",
    "type",
    "func",
    "record",
    "enum",
    "flags",
    "variant",
    "resource",
    "interface",
    "world",
    "import",
    "export",
    "package",
    "include",
    "as",
    "from",
    "static",
    "constructor",
    "list",
    "option",
    "result",
    "tuple",
    "future",
    "stream",
    "bool",
    "u8",
    "u16",
    "u32",
    "u64",
    "s8",
    "s16",
    "s32",
    "s64",
    "f32",
    "f64",
    "char",
    "string",
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
            // Break camelCase / PascalCase boundaries: an uppercase letter that
            // follows a lowercase letter or digit starts a new word, so
            // `EepromSnapshot` -> `eeprom-snapshot` (issue #319 item 6) instead
            // of the collapsed `eepromsnapshot`.
            if ch.is_ascii_uppercase()
                && cur
                    .chars()
                    .last()
                    .is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
            {
                words.push(std::mem::take(&mut cur));
            }
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

    fn build_test_instance() -> (GlobalScope, SystemInstance) {
        let aadl = r#"
package TestPkg
public
    data Sample
    end Sample;

    data Clock64
        properties
            Data_Size => 8 Bytes;
    end Clock64;

    data Flag8
        properties
            Data_Size => 1 Bytes;
    end Flag8;

    process Controller
        features
            sensor_in: in data port;
            cmd_out: out data port;
            message_in: in event data port Sample;
            announce_out: out event data port Sample;
            clock_in: in event data port Clock64;
            flag_in: in event data port Flag8;
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
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("TestPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        (scope, inst)
    }

    #[test]
    fn wit_gen_produces_output() {
        let (scope, inst) = build_test_instance();
        // Find the process instance
        let proc_idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx);

        if let Some(idx) = proc_idx {
            let file = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
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
        let (scope, inst) = build_test_instance();
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx)
            .expect("test model has a process");
        let file = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");

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

        // Scalar Data_Size maps to precise WIT scalars (REQ-CODEGEN-WIT-TYPES):
        // Clock64 declares 8 Bytes -> u64, Flag8 declares 1 Bytes -> u8;
        // Sample declares nothing -> stays list<u8> (asserted above).
        assert!(
            body.contains("type clock64 = u64;"),
            "8-byte Data_Size must map to u64:\n{body}"
        );
        assert!(
            body.contains("type flag8 = u8;"),
            "1-byte Data_Size must map to u8:\n{body}"
        );

        // Authoritative check: feed the WIT to wit-parser. push_str errors on
        // invalid identifiers OR references to undefined types.
        let mut resolve = wit_parser::Resolve::new();
        resolve
            .push_str("generated.wit", &file.content)
            .unwrap_or_else(|e| {
                panic!(
                    "generated WIT failed to parse/resolve: {e}\n---\n{}",
                    file.content
                )
            });
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

    /// Fixture for REQ-CODEGEN-WIT-RECORDS-001 (#319): a `data implementation`
    /// with scalar subcomponents (the real-world `EepromSnapshot.Impl` =
    /// u32 + u32 + u8 pattern), referenced by a process port.
    fn build_record_test_instance() -> (GlobalScope, SystemInstance) {
        let aadl = r#"
package RecPkg
public
    data Word32
        properties
            Data_Size => 4 Bytes;
    end Word32;

    data Byte8
        properties
            Data_Size => 1 Bytes;
    end Byte8;

    data EepromSnapshot
    end EepromSnapshot;

    data implementation EepromSnapshot.Impl
        subcomponents
            addr: data Word32;
            value: data Word32;
            flags: data Byte8;
    end EepromSnapshot.Impl;

    process Store
        features
            snapshot_in: in data port EepromSnapshot.Impl;
    end Store;

    process implementation Store.Impl
        subcomponents
            st_thread: thread StoreThread.Impl;
    end Store.Impl;

    thread StoreThread
    end StoreThread;

    thread implementation StoreThread.Impl
    end StoreThread.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            store: process Store.Impl;
    end Top.Impl;
end RecPkg;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "rec.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("RecPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        (scope, inst)
    }

    /// RED-before-green oracle for REQ-CODEGEN-WIT-RECORDS-001 (#319 items 1,3,6):
    /// an AADL `data implementation` with scalar subcomponents must generate a
    /// WIT `record` with one typed field per subcomponent — NOT an opaque
    /// `list<u8>` blob (which would need `cabi_realloc` at the ABI boundary,
    /// defeating no_alloc). Currently RED: wit_gen emits `type ... = list<u8>`.
    #[test]
    fn data_implementation_becomes_wit_record() {
        let (scope, inst) = build_record_test_instance();
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx)
            .expect("test model has a process");
        let file = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
        let body: String = file
            .content
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // Must NOT degrade the structured type to an opaque byte blob.
        assert!(
            !body.contains("= list<u8>"),
            "data implementation must NOT degrade to list<u8>:\n{body}"
        );
        // Must emit a typed record (name kebab-cased with PascalCase boundaries
        // preserved: EepromSnapshot.Impl -> eeprom-snapshot-impl).
        assert!(
            body.contains("record eeprom-snapshot-impl {"),
            "expected a WIT record for EepromSnapshot.Impl:\n{body}"
        );
        // One typed field per scalar subcomponent (4B->u32, 4B->u32, 1B->u8).
        for field in ["addr: u32", "value: u32", "flags: u8"] {
            assert!(
                body.contains(field),
                "expected record field `{field}`:\n{body}"
            );
        }

        // The record must be VALID, bindable WIT — not just string-present.
        // wit-parser rejects malformed records / undefined type references.
        let mut resolve = wit_parser::Resolve::new();
        resolve
            .push_str("record.wit", &file.content)
            .unwrap_or_else(|e| {
                panic!(
                    "generated record WIT failed to parse/resolve: {e}\n---\n{}",
                    file.content
                )
            });
    }

    /// Fixture for the hard-error path: a record with fields that CANNOT be
    /// encoded without heap allocation — a 3-byte scalar (no WIT integer) and an
    /// opaque type (no Data_Size, no fields).
    fn build_bad_record_instance() -> (GlobalScope, SystemInstance) {
        let aadl = r#"
package BadPkg
public
    data Word32
        properties
            Data_Size => 4 Bytes;
    end Word32;

    data Odd3
        properties
            Data_Size => 3 Bytes;
    end Odd3;

    data Blob
    end Blob;

    data BadRecord
    end BadRecord;

    data implementation BadRecord.Impl
        subcomponents
            good: data Word32;
            odd: data Odd3;
            blob: data Blob;
    end BadRecord.Impl;

    process BadStore
        features
            in_port: in data port BadRecord.Impl;
    end BadStore;

    process implementation BadStore.Impl
        subcomponents
            th: thread BadThread.Impl;
    end BadStore.Impl;

    thread BadThread
    end BadThread;

    thread implementation BadThread.Impl
    end BadThread.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            store: process BadStore.Impl;
    end Top.Impl;
end BadPkg;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "bad.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("BadPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        (scope, inst)
    }

    /// Oracle for the user-chosen hard-error policy (REQ-CODEGEN-WIT-RECORDS-001,
    /// #319): a record field that cannot be a no_alloc scalar makes codegen
    /// REFUSE (return an error) rather than silently emit an allocating
    /// `list<u8>`. Both offending fields must be reported; the good field must
    /// not appear as an error.
    #[test]
    fn non_encodable_record_field_is_a_hard_error() {
        let (scope, inst) = build_bad_record_instance();
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx)
            .expect("test model has a process");

        let err = generate_wit(&inst, &scope, idx)
            .expect_err("codegen must refuse a record with non-encodable fields");

        // The 3-byte field and the opaque field are both reported…
        assert!(
            err.errors.iter().any(|e| e.contains("`odd`")),
            "expected a hard error naming the 3-byte field `odd`: {:?}",
            err.errors
        );
        assert!(
            err.errors.iter().any(|e| e.contains("`blob`")),
            "expected a hard error naming the opaque field `blob`: {:?}",
            err.errors
        );
        // …and the representable `good` field is NOT flagged.
        assert!(
            !err.errors.iter().any(|e| e.contains("`good`")),
            "the representable u32 field `good` must not be an error: {:?}",
            err.errors
        );
    }

    /// Fixture with signed / float / unsigned scalar fields (Data Modeling Annex
    /// `Number_Representation` / `Data_Representation` properties).
    fn build_signed_float_instance() -> (GlobalScope, SystemInstance) {
        let aadl = r#"
package NumPkg
public
    data FloatWord
        properties
            Data_Size => 4 Bytes;
            Data_Representation => Float;
    end FloatWord;

    data SignedWord
        properties
            Data_Size => 4 Bytes;
            Number_Representation => Signed;
    end SignedWord;

    data Ushort
        properties
            Data_Size => 2 Bytes;
    end Ushort;

    data Reading
    end Reading;

    data implementation Reading.Impl
        subcomponents
            temp: data FloatWord;
            offset: data SignedWord;
            count: data Ushort;
    end Reading.Impl;

    process Store
        features
            snapshot_in: in data port Reading.Impl;
    end Store;

    process implementation Store.Impl
        subcomponents
            st_thread: thread StoreThread.Impl;
    end Store.Impl;

    thread StoreThread
    end StoreThread;

    thread implementation StoreThread.Impl
    end StoreThread.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            store: process Store.Impl;
    end Top.Impl;
end NumPkg;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "num.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("NumPkg"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        (scope, inst)
    }

    /// Oracle for signed/float mapping (REQ-CODEGEN-WIT-RECORDS-001, #319 P2c):
    /// `Data_Representation => Float` maps to `f32`/`f64`, `Number_Representation
    /// => Signed` to `s8..s64`, and unspecified stays unsigned `u*`.
    #[test]
    fn record_fields_honor_signedness_and_float() {
        let (scope, inst) = build_signed_float_instance();
        let idx = inst
            .all_components()
            .find(|(_, c)| c.category == ComponentCategory::Process)
            .map(|(idx, _)| idx)
            .expect("test model has a process");
        let file = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
        let body: String = file
            .content
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for field in ["temp: f32", "offset: s32", "count: u16"] {
            assert!(
                body.contains(field),
                "expected record field `{field}` (signed/float mapping):\n{body}"
            );
        }

        // Still valid, bindable WIT.
        let mut resolve = wit_parser::Resolve::new();
        resolve
            .push_str("num.wit", &file.content)
            .unwrap_or_else(|e| {
                panic!("generated WIT failed to parse: {e}\n---\n{}", file.content)
            });
    }
}
