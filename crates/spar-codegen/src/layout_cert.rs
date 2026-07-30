//! Certificate-checked AADL↔WIT↔canonical-ABI byte-layout equivalence.
//!
//! spar's FIRST ordeal integration and the ordeal-edge pipe-cleaner
//! (REQ-CODEGEN-LAYOUT-CERT-001, GitHub #327). `wit_gen` maps an AADL
//! `data implementation` → WIT `record` → canonical-ABI byte layout, but that
//! width/offset table has always been *syntactic and unchecked* — #319's 9-byte
//! `EepromSnapshot` silently collapsing to an opaque `list<u8>` is the symptom.
//!
//! This module encodes the two layouts as ordeal QF_BV bit-vector terms and
//! discharges [`ordeal::Solver::prove_valid`]:
//!
//! - the **source** layout is the AADL data implementation packed little-endian
//!   (each field at the running byte cursor, no padding);
//! - the **WIT** layout is the generated record's canonical-ABI byte layout
//!   (each field aligned to its natural alignment, the record padded to its
//!   maximum field alignment).
//!
//! For every field we prove that decoding it out of the WIT encoding recovers
//! exactly the value decoded out of the source encoding. `Unsat` ⟹ the WIT
//! layout is a **non-overlapping, value-preserving** re-encoding of the source
//! for *all* inputs, and the proof arrives with a re-checkable LRAT certificate
//! (validated by `ordeal-lrat`, the trusted checker — dependency-free and
//! slated for Lean 4 verification); `Sat` ⟹ two fields' bytes collide, with
//! the differing input as a counterexample.
//!
//! ## What is PROVEN vs ASSUMED
//!
//! PROVEN (certificate-checked, all inputs): the computed offset/width table is
//! a self-consistent lossless codec — no field's bytes clobber another's and
//! each field round-trips bit-exactly. This is the property #319 violated by
//! dropping the record structure entirely.
//!
//! In-bounds is a *structural* invariant, not a solver goal: [`record_layout`]
//! always sizes the record to fit every field, so a generated layout never runs
//! a field off the end. A hand-built [`RecordLayout`] that placed a field past
//! `wit_size_bits` would fail loudly (a bounds panic while building the byte
//! map) rather than be mis-certified `Faithful` — fail-safe, never silent.
//!
//! ASSUMED (trusted, encoded in [`wit_align`]): that the canonical-ABI
//! alignment of a WIT scalar equals its size (u8→1 … u64/f64→8). The proof
//! certifies the layout *spar computed*; it does not re-derive the ABI
//! alignment constants from the Component Model spec.
//!
//! ## Scope (v0.1, roadmap v0.35.0)
//!
//! Flat records of scalar fields whose `Data_Size` is a WIT scalar width
//! (1/2/4/8 bytes ⇒ 8/16/32/64 bits, all within ordeal's 128-bit ceiling).
//! Records containing nested records or arrays are not certified yet
//! ([`record_layout`] returns `None`) — a successor increment. The BvTerm
//! encoding and LRAT-cert plumbing established here are what
//! REQ-BMC-CONCURRENCY-001 and REQ-PROOF-NC-CERT-001 reuse.

use ordeal::layout::{from_le_bytes, to_le_bytes};
use ordeal::{BoolTerm, BvTerm, Certificate, CheckResult, Solver, Sort};
use spar_hir_def::resolver::DataShape;

/// Placement of one scalar field in both the source-packed and WIT
/// canonical-ABI layouts. Offsets and widths are in **bits**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldLayout {
    pub name: String,
    pub width_bits: u32,
    /// Bit offset in the source (AADL packed, no padding) encoding.
    pub src_offset_bits: u32,
    /// Bit offset in the generated WIT canonical-ABI encoding.
    pub wit_offset_bits: u32,
}

/// The two byte layouts of a flat scalar `data implementation`, ready to encode
/// as ordeal terms. `src_size_bits` is the packed size; `wit_size_bits` is the
/// canonical-ABI record size (padded to the maximum field alignment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordLayout {
    pub type_name: String,
    pub fields: Vec<FieldLayout>,
    pub src_size_bits: u32,
    pub wit_size_bits: u32,
}

/// Canonical-ABI alignment (in bytes) of a WIT scalar of the given byte width.
/// For the Component Model's numeric primitives, alignment equals size.
/// ASSUMED: this is the trusted ABI spec the proof is *relative to* (see the
/// module docs), not something the certificate re-derives.
fn wit_align(bytes: u64) -> u32 {
    bytes as u32
}

/// Round `value` up to the next multiple of `align` (a power of two ≥ 1).
fn align_up(value: u32, align: u32) -> u32 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

/// Compute the source-packed and WIT canonical-ABI layouts for a
/// `data implementation`, or `None` when the shape is outside v0.1 scope
/// (not a record, or containing a field that is not a 1/2/4/8-byte scalar).
pub fn record_layout(shape: &DataShape) -> Option<RecordLayout> {
    let DataShape::Record { type_name, fields } = shape else {
        return None;
    };
    if fields.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(fields.len());
    let mut src_cursor_bits = 0u32;
    let mut wit_cursor_bytes = 0u32;
    let mut max_align = 1u32;

    for f in fields {
        let bytes = match &f.shape {
            // 1/2/4/8-byte scalars are exactly the WIT scalar widths; any other
            // size already hard-errors in wit_gen, so it never reaches codegen.
            DataShape::Scalar { bytes, .. } if matches!(bytes, 1 | 2 | 4 | 8) => *bytes,
            // Nested records, arrays, opaque, or non-scalar-width fields: out of
            // v0.1 scope — do not claim a certificate we cannot honestly produce.
            _ => return None,
        };
        let width_bits = bytes as u32 * 8;
        let align = wit_align(bytes);
        let wit_off_bytes = align_up(wit_cursor_bytes, align);

        out.push(FieldLayout {
            name: f.name.as_str().to_string(),
            width_bits,
            src_offset_bits: src_cursor_bits,
            wit_offset_bits: wit_off_bytes * 8,
        });

        src_cursor_bits += width_bits;
        wit_cursor_bytes = wit_off_bytes + bytes as u32;
        max_align = max_align.max(align);
    }

    let wit_size_bytes = align_up(wit_cursor_bytes, max_align);
    Some(RecordLayout {
        type_name: type_name.clone(),
        fields: out,
        src_size_bits: src_cursor_bits,
        wit_size_bits: wit_size_bytes * 8,
    })
}

/// Outcome of discharging a [`RecordLayout`] through ordeal.
#[derive(Debug, Clone)]
pub enum LayoutVerdict {
    /// The WIT layout faithfully re-encodes the source for all inputs. Carries
    /// the LRAT certificate; call [`Certificate::recheck`] to re-validate it
    /// through the trusted checker.
    Faithful(Certificate),
    /// The layouts diverge: one field's bytes collide with another's. Carries
    /// the counterexample assignments (`var name → value`).
    Divergent(Vec<(String, u128)>),
    /// ordeal returned no verdict (e.g. a width outside `1..=128`). NEVER treat
    /// as faithful — an undecided layout is an un-certified layout.
    Undecided,
}

/// Build the little-endian byte terms for one encoding: a vector of 8-bit terms
/// of length `size_bits / 8`, with each field's bytes written at
/// `offset(field)` (via `offset_of`) and every remaining byte a padding zero.
fn encode_bytes(
    layout: &RecordLayout,
    size_bits: u32,
    offset_of: impl Fn(&FieldLayout) -> u32,
) -> Vec<BvTerm> {
    let zero = BvTerm::Const {
        value: 0,
        sort: Sort::new(8),
    };
    let mut bytes = vec![zero; (size_bits / 8) as usize];
    for fl in &layout.fields {
        let var = BvTerm::Var {
            name: fl.name.clone(),
            sort: Sort::new(fl.width_bits),
        };
        // to_le_bytes requires a positive multiple of 8; field widths always are.
        let field_bytes = to_le_bytes(&var, fl.width_bits);
        let base = (offset_of(fl) / 8) as usize;
        for (j, b) in field_bytes.into_iter().enumerate() {
            bytes[base + j] = b;
        }
    }
    bytes
}

/// Encode the source and WIT layouts as ordeal QF_BV terms and prove that every
/// field decodes to the same value from both. One certificate per record.
///
/// The two encodings share one symbolic value per field, so a correct layout
/// makes each `extract(wit) == extract(src)` a tautology and the whole
/// conjunction valid (`Unsat`). A layout bug that lets one field's bytes land
/// on another's (overlap) or run past the record (truncation) breaks the
/// round-trip for the clobbered field, and ordeal returns `Sat` with the
/// differing input — this is what makes the oracle load-bearing rather than
/// vacuous (see the falsification test).
pub fn prove_layout(layout: &RecordLayout) -> LayoutVerdict {
    let src_bytes = encode_bytes(layout, layout.src_size_bits, |fl| fl.src_offset_bits);
    let wit_bytes = encode_bytes(layout, layout.wit_size_bits, |fl| fl.wit_offset_bits);
    let src_term = from_le_bytes(&src_bytes);
    let wit_term = from_le_bytes(&wit_bytes);

    let mut goal: Option<BoolTerm> = None;
    for fl in &layout.fields {
        let src = BvTerm::Extract {
            hi: fl.src_offset_bits + fl.width_bits - 1,
            lo: fl.src_offset_bits,
            arg: Box::new(src_term.clone()),
        };
        let wit = BvTerm::Extract {
            hi: fl.wit_offset_bits + fl.width_bits - 1,
            lo: fl.wit_offset_bits,
            arg: Box::new(wit_term.clone()),
        };
        let eq = BoolTerm::Eq(Box::new(wit), Box::new(src));
        goal = Some(match goal {
            None => eq,
            Some(prev) => BoolTerm::And(Box::new(prev), Box::new(eq)),
        });
    }
    // `record_layout` guarantees ≥1 field, so `goal` is always `Some`.
    let Some(goal) = goal else {
        return LayoutVerdict::Undecided;
    };

    match Solver::prove_valid(goal) {
        CheckResult::Unsat(cert) => LayoutVerdict::Faithful(cert),
        CheckResult::Sat(model) => LayoutVerdict::Divergent(model.assignments),
        CheckResult::Unknown => LayoutVerdict::Undecided,
    }
}

/// Certify a resolved `data` shape's byte layout, if it is in v0.1 scope.
/// `None` mirrors [`record_layout`] returning `None` (not a certifiable record).
pub fn certify_shape(shape: &DataShape) -> Option<(RecordLayout, LayoutVerdict)> {
    let layout = record_layout(shape)?;
    let verdict = prove_layout(&layout);
    Some((layout, verdict))
}

/// Render a [`Certificate`] as textual, re-checkable evidence for a record's
/// layout — the "LRAT-cert-as-rivet-artifact" typed evidence the roadmap calls
/// for. The header is a human-readable summary of the proven layout; the body
/// is the verbatim LRAT proof that `ordeal-lrat` (and, eventually, its
/// Lean-verified twin) re-checks against the emitted CNF.
pub fn layout_cert_evidence(layout: &RecordLayout, cert: &Certificate) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "; ordeal LRAT layout-equivalence certificate for `{}`\n",
        layout.type_name
    ));
    s.push_str("; REQ-CODEGEN-LAYOUT-CERT-001 (#327): AADL packed <-> WIT canonical-ABI\n");
    s.push_str(&format!(
        "; source packed size = {} bits, WIT record size = {} bits\n",
        layout.src_size_bits, layout.wit_size_bits
    ));
    for fl in &layout.fields {
        s.push_str(&format!(
            ";   field {:<16} width={:>3}b  src@{:>4}b  wit@{:>4}b\n",
            fl.name, fl.width_bits, fl.src_offset_bits, fl.wit_offset_bits
        ));
    }
    s.push_str(&format!("; CNF clauses refuted = {}\n", cert.cnf.len()));
    match cert.lrat_text() {
        Some(text) => s.push_str(text),
        None => s.push_str("; (LRAT bytes are not valid UTF-8 — unexpected for ordeal certs)\n"),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::{ClassifierRef, Name};
    use spar_hir_def::resolver::{DataField, GlobalScope, ScalarKind};

    fn scalar(bytes: u64, kind: ScalarKind) -> DataShape {
        DataShape::Scalar { bytes, kind }
    }

    fn field(name: &str, shape: DataShape) -> DataField {
        DataField {
            name: Name::new(name),
            shape,
        }
    }

    /// The #319 record: `EepromSnapshot.Impl = addr:u32 + value:u32 + flags:u8`.
    fn eeprom_shape() -> DataShape {
        DataShape::Record {
            type_name: "EepromSnapshot.Impl".to_string(),
            fields: vec![
                field("addr", scalar(4, ScalarKind::Unsigned)),
                field("value", scalar(4, ScalarKind::Unsigned)),
                field("flags", scalar(1, ScalarKind::Unsigned)),
            ],
        }
    }

    #[test]
    fn eeprom_layout_offsets_match_canonical_abi() {
        let layout = record_layout(&eeprom_shape()).expect("flat scalar record is in scope");
        // Packed source: 4 + 4 + 1 = 9 bytes.
        assert_eq!(layout.src_size_bits, 72);
        // Canonical ABI pads the 9-byte record to 12 (max field alignment 4).
        assert_eq!(layout.wit_size_bits, 96);
        let offs: Vec<_> = layout
            .fields
            .iter()
            .map(|f| (f.src_offset_bits, f.wit_offset_bits, f.width_bits))
            .collect();
        // addr@0, value@4B, flags@8B — offsets coincide because the fields are
        // naturally aligned; only the record's trailing padding differs.
        assert_eq!(offs, vec![(0, 0, 32), (32, 32, 32), (64, 64, 8)]);
    }

    #[test]
    fn eeprom_layout_is_faithful_with_rechecked_certificate() {
        let layout = record_layout(&eeprom_shape()).unwrap();
        match prove_layout(&layout) {
            LayoutVerdict::Faithful(cert) => {
                // The certificate re-checks through ordeal-lrat (the trusted,
                // Lean-bound checker) — the verdict does not rest on the solver.
                cert.recheck()
                    .expect("layout-equivalence LRAT certificate must re-check");
                assert!(
                    !cert.lrat.is_empty(),
                    "a faithful proof carries a non-empty LRAT certificate"
                );
                let ev = layout_cert_evidence(&layout, &cert);
                assert!(ev.contains("EepromSnapshot.Impl"));
                assert!(ev.contains("REQ-CODEGEN-LAYOUT-CERT-001"));
            }
            other => panic!("EepromSnapshot layout must be certified faithful, got {other:?}"),
        }
    }

    /// A `u8` before a `u32` forces canonical-ABI padding: the source packs them
    /// at bytes 0/1, WIT aligns the `u32` to byte 4. The layout is still a
    /// faithful (non-overlapping) codec, so it certifies.
    #[test]
    fn padded_record_is_faithful() {
        let shape = DataShape::Record {
            type_name: "Padded".into(),
            fields: vec![
                field("tag", scalar(1, ScalarKind::Unsigned)),
                field("word", scalar(4, ScalarKind::Unsigned)),
            ],
        };
        let layout = record_layout(&shape).unwrap();
        assert_eq!(layout.fields[1].src_offset_bits, 8, "packed after the u8");
        assert_eq!(layout.fields[1].wit_offset_bits, 32, "aligned to byte 4");
        assert_eq!(layout.wit_size_bits, 64, "8 bytes (align 4)");
        assert!(matches!(prove_layout(&layout), LayoutVerdict::Faithful(_)));
    }

    /// FALSIFICATION (REQ kill-criterion). A deliberately wrong layout — two
    /// 32-bit fields whose WIT bytes overlap (a cursor-advance bug) — MUST be
    /// rejected with a counterexample. A green run that cannot be made red would
    /// mean the encoding is not load-bearing.
    #[test]
    fn overlapping_wit_layout_is_rejected() {
        let layout = RecordLayout {
            type_name: "Overlap".into(),
            fields: vec![
                FieldLayout {
                    name: "a".into(),
                    width_bits: 32,
                    src_offset_bits: 0,
                    wit_offset_bits: 0,
                },
                FieldLayout {
                    name: "b".into(),
                    width_bits: 32,
                    src_offset_bits: 32,
                    // BUG: `b` should start at bit 32; overlapping `a` at 0 lets
                    // `b`'s bytes clobber `a`.
                    wit_offset_bits: 0,
                },
            ],
            src_size_bits: 64,
            wit_size_bits: 32,
        };
        match prove_layout(&layout) {
            LayoutVerdict::Divergent(cx) => {
                assert!(
                    !cx.is_empty(),
                    "the counterexample names the differing inputs"
                );
            }
            other => panic!("an overlapping layout must be rejected (Sat), got {other:?}"),
        }
    }

    #[test]
    fn nested_record_is_out_of_scope() {
        let shape = DataShape::Record {
            type_name: "Outer".into(),
            fields: vec![field(
                "inner",
                DataShape::Record {
                    type_name: "Inner".into(),
                    fields: vec![field("x", scalar(4, ScalarKind::Unsigned))],
                },
            )],
        };
        assert!(
            record_layout(&shape).is_none(),
            "nested records are a successor increment, not silently mis-certified"
        );
    }

    /// End-to-end: the REAL `EepromSnapshot.Impl`, resolved through the same
    /// `resolve_data_shape` path `wit_gen` uses — not a hand-built approximation
    /// — certifies faithful with a re-checkable certificate.
    #[test]
    fn resolved_aadl_record_is_faithful() {
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
end RecPkg;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "rec.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let cref = ClassifierRef::implementation(
            Some(Name::new("RecPkg")),
            Name::new("EepromSnapshot"),
            Name::new("Impl"),
        );
        let shape = scope.resolve_data_shape(&Name::new("RecPkg"), &cref);
        let (layout, verdict) =
            certify_shape(&shape).expect("resolved EepromSnapshot.Impl is a certifiable record");
        assert_eq!(layout.fields.len(), 3);
        assert_eq!(layout.wit_size_bits, 96);
        match verdict {
            LayoutVerdict::Faithful(cert) => cert
                .recheck()
                .expect("resolved-record certificate must re-check"),
            other => panic!("resolved EepromSnapshot must be faithful, got {other:?}"),
        }
    }
}
