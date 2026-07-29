//! Hardware-target artifact generation from an AADL instance model.
//!
//! REQ-CODEGEN-TARGET-ARTIFACTS-001 (#338).
//!
//! Emits build-time target artifacts derived from a *hardware* AADL model —
//! the model becomes the single source of truth for an embedded target build:
//!
//! - a Rust **constants module** ([`emit_constants`], `spar emit --format
//!   constants`): memory-region base addresses and sizes, plus peripheral
//!   (`device`) base addresses, as `pub const` items a firmware crate `include!`s;
//! - a **linker memory map** ([`emit_linker`], `spar emit --format linker`): a
//!   GNU-ld `MEMORY { }` block (the rust-embedded `memory.x` shape), one region
//!   per `memory` component.
//!
//! Both are derived purely from standard AADL `Memory_Properties` on the
//! instantiated model: `Base_Address` (an `aadlinteger`) for the origin, and
//! `Byte_Count` or `Memory_Size` for the length. Values are read from the
//! resolved instance property map, so type/impl inheritance and `applies to`
//! associations are already folded in.
//!
//! **Scope (honest).** This increment emits only base addresses and sizes —
//! the mechanical, standard-property-backed core of the #338 ask. Device
//! *register-detail offsets* and the *WIT device-driver world binding* (asks 1
//! and 3 of the issue) are NOT emitted here; they require a register/binding
//! convention AADL does not standardize and are tracked by the successor
//! REQ-CODEGEN-TARGET-ARTIFACTS-002.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::{ComponentCategory, PropertyExpr};
use spar_hir_def::properties::PropertyMap;

/// A memory region extracted from a `memory` component instance.
struct Region {
    /// Uppercase identifier derived from the component's instance path.
    ident: String,
    /// Full instance path (source comment + tie-break ordering).
    path: String,
    base: u64,
    /// Size in bytes, if the model carries `Byte_Count`/`Memory_Size`.
    size_bytes: Option<u64>,
}

/// A peripheral base extracted from a memory-mapped `device` component instance.
struct Peripheral {
    ident: String,
    path: String,
    base: u64,
}

/// Read `Base_Address` (an `aadlinteger`) off a resolved property map.
///
/// Accepts the standard `Memory_Properties::Base_Address` qualification and a
/// bare unqualified `Base_Address`. Returns `None` when absent or not a
/// non-negative integer.
fn base_address(props: &PropertyMap) -> Option<u64> {
    let expr = props
        .get_typed("Memory_Properties", "Base_Address")
        .or_else(|| props.get_typed("", "Base_Address"))?;
    match expr {
        PropertyExpr::Integer(v, _) if *v >= 0 => Some(*v as u64),
        _ => None,
    }
}

/// Read a memory-region size in **bytes** off a resolved property map.
///
/// Prefers `Byte_Count` (a plain byte count) and falls back to `Memory_Size`
/// (an AADL `Size` with units, e.g. `256 KByte`). A `Memory_Size` that is not
/// a whole number of bytes (sub-byte bit width) yields `None` rather than a
/// truncated value.
fn size_bytes(props: &PropertyMap) -> Option<u64> {
    if let Some(PropertyExpr::Integer(v, _)) = props
        .get_typed("Memory_Properties", "Byte_Count")
        .or_else(|| props.get_typed("", "Byte_Count"))
        && *v >= 0
    {
        return Some(*v as u64);
    }
    // `Memory_Size` is a Size property; property_accessors normalizes it to bits.
    let bits = spar_analysis::property_accessors::get_size_property(props, "Memory_Size")?;
    if bits.is_multiple_of(8) {
        Some(bits / 8)
    } else {
        None
    }
}

/// Derive a stable, C/Rust-identifier-safe constant name from an instance path.
///
/// Uses the leaf component name uppercased, mapping every character outside
/// `[A-Za-z0-9_]` to `_` and prefixing `_` if it would start with a digit.
fn ident_from_path(path: &str) -> String {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let mut out = String::with_capacity(leaf.len());
    for ch in leaf.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Walk the instance model and collect memory regions and peripheral bases,
/// each sorted by `(base, ident)` for deterministic output.
///
/// Returns an error if two collected items would collide on the same
/// identifier (an ambiguous model the caller must disambiguate) — a hard,
/// deterministic failure rather than a silently dropped constant.
fn collect(inst: &SystemInstance) -> Result<(Vec<Region>, Vec<Peripheral>), String> {
    let mut regions = Vec::new();
    let mut peripherals = Vec::new();

    for (idx, comp) in inst.all_components() {
        let props = inst.properties_for(idx);
        let path = inst.component_instance_path(idx);
        match comp.category {
            ComponentCategory::Memory => {
                if let Some(base) = base_address(props) {
                    regions.push(Region {
                        ident: ident_from_path(&path),
                        path,
                        base,
                        size_bytes: size_bytes(props),
                    });
                }
            }
            ComponentCategory::Device => {
                // Only memory-mapped devices (those with a base address) get a
                // peripheral constant; a non-mapped device is legitimately skipped.
                if let Some(base) = base_address(props) {
                    peripherals.push(Peripheral {
                        ident: ident_from_path(&path),
                        path,
                        base,
                    });
                }
            }
            _ => {}
        }
    }

    regions.sort_by(|a, b| a.base.cmp(&b.base).then_with(|| a.ident.cmp(&b.ident)));
    peripherals.sort_by(|a, b| a.base.cmp(&b.base).then_with(|| a.ident.cmp(&b.ident)));

    // Collision check across the full constant namespace.
    let mut seen: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for (ident, path) in regions
        .iter()
        .map(|r| (r.ident.as_str(), r.path.as_str()))
        .chain(
            peripherals
                .iter()
                .map(|p| (p.ident.as_str(), p.path.as_str())),
        )
    {
        if let Some(prev) = seen.insert(ident, path) {
            return Err(format!(
                "target-artifact identifier collision: `{ident}` derived from both `{prev}` \
                 and `{path}`; give the components distinct leaf names"
            ));
        }
    }

    Ok((regions, peripherals))
}

/// A human-readable byte-size annotation, e.g. `1048576 bytes (1024 KiB)`.
fn size_comment(bytes: u64) -> String {
    if bytes != 0 && bytes.is_multiple_of(1024 * 1024) {
        format!("{bytes} bytes ({} MiB)", bytes / (1024 * 1024))
    } else if bytes != 0 && bytes.is_multiple_of(1024) {
        format!("{bytes} bytes ({} KiB)", bytes / 1024)
    } else {
        format!("{bytes} bytes")
    }
}

/// Emit a Rust constants module from a hardware AADL instance model.
///
/// `root_ref` is the `Package::Type.Impl` reference the model was instantiated
/// from, recorded in the generated header. Returns an error if the model
/// yields no base-addressed memory/device components (an empty artifact is
/// almost always a wrong or non-hardware model), or on identifier collision.
pub fn emit_constants(inst: &SystemInstance, root_ref: &str) -> Result<String, String> {
    let (regions, peripherals) = collect(inst)?;
    if regions.is_empty() && peripherals.is_empty() {
        return Err(
            "no `memory` or `device` component with a `Base_Address` was found; \
             this does not look like a hardware target model"
                .to_string(),
        );
    }

    let mut out = String::new();
    out.push_str("// Generated by `spar emit --format constants`. DO NOT EDIT.\n");
    out.push_str(&format!("// Source model: {root_ref}\n"));
    out.push_str(
        "// Target memory map and peripheral base addresses derived from the AADL model.\n",
    );
    out.push_str("#![allow(dead_code)]\n\n");

    if !regions.is_empty() {
        out.push_str("// ── Memory regions ──────────────────────────────────────────\n");
        for r in &regions {
            out.push_str(&format!("// {}\n", r.path));
            out.push_str(&format!(
                "pub const {}_BASE: usize = 0x{:08X};\n",
                r.ident, r.base
            ));
            match r.size_bytes {
                Some(size) => out.push_str(&format!(
                    "pub const {}_SIZE: usize = 0x{:08X}; // {}\n",
                    r.ident,
                    size,
                    size_comment(size)
                )),
                None => out.push_str(&format!(
                    "// {}_SIZE: no Byte_Count/Memory_Size in the model\n",
                    r.ident
                )),
            }
        }
        out.push('\n');
    }

    if !peripherals.is_empty() {
        out.push_str("// ── Peripheral base addresses ───────────────────────────────\n");
        for p in &peripherals {
            out.push_str(&format!("// {}\n", p.path));
            out.push_str(&format!(
                "pub const {}_BASE: usize = 0x{:08X};\n",
                p.ident, p.base
            ));
        }
    }

    Ok(out)
}

/// Emit a GNU-ld `MEMORY { }` block (rust-embedded `memory.x` shape) from a
/// hardware AADL instance model — one region per base-addressed `memory`
/// component. `ORIGIN` is emitted in hex, `LENGTH` in exact decimal bytes.
///
/// Regions without a resolvable size are omitted (a linker region requires a
/// length); if that leaves the block empty, an error is returned.
pub fn emit_linker(inst: &SystemInstance, root_ref: &str) -> Result<String, String> {
    let (regions, _peripherals) = collect(inst)?;
    let sized: Vec<&Region> = regions.iter().filter(|r| r.size_bytes.is_some()).collect();
    if sized.is_empty() {
        return Err("no `memory` component with both `Base_Address` and a size \
             (`Byte_Count`/`Memory_Size`) was found; cannot emit a linker memory map"
            .to_string());
    }

    let mut out = String::new();
    out.push_str("/* Generated by `spar emit --format linker`. DO NOT EDIT. */\n");
    out.push_str(&format!("/* Source model: {root_ref} */\n"));
    out.push_str("MEMORY\n{\n");
    for r in &sized {
        let size = r.size_bytes.expect("filtered to Some above");
        out.push_str(&format!(
            "  {} : ORIGIN = 0x{:08X}, LENGTH = {} /* {} */\n",
            r.ident,
            r.base,
            size,
            size_comment(size)
        ));
    }
    out.push_str("}\n");

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::Name;
    use spar_hir_def::resolver::GlobalScope;

    /// A minimal STM32-shaped hardware target: two `memory` regions (flash via
    /// `Memory_Size`, sram via `Byte_Count`), two memory-mapped `device`
    /// peripherals, and one non-mapped device (no `Base_Address`).
    const BOARD: &str = r#"
package Target_Board
public
  processor Cortex_M4
  end Cortex_M4;

  memory Flash_Region
  end Flash_Region;

  memory Sram_Region
  end Sram_Region;

  device Uart
  end Uart;

  device Gpio
  end Gpio;

  device Led
  end Led;

  system Board
  end Board;

  system implementation Board.Impl
    subcomponents
      cpu: processor Cortex_M4;
      flash: memory Flash_Region {
        Base_Address => 16#08000000#;
        Memory_Size => 1024 KByte;
      };
      sram: memory Sram_Region {
        Base_Address => 16#20000000#;
        Byte_Count => 131072;
      };
      uart1: device Uart {
        Base_Address => 16#40011000#;
      };
      gpioa: device Gpio {
        Base_Address => 16#40020000#;
      };
      status_led: device Led;
  end Board.Impl;
end Target_Board;
"#;

    fn board_instance() -> SystemInstance {
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "board.aadl".to_string(), BOARD.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        SystemInstance::instantiate(
            &scope,
            &Name::new("Target_Board"),
            &Name::new("Board"),
            &Name::new("Impl"),
        )
    }

    #[test]
    fn constants_golden() {
        let inst = board_instance();
        let out = emit_constants(&inst, "Target_Board::Board.Impl").expect("emit");
        let expected = "\
// Generated by `spar emit --format constants`. DO NOT EDIT.
// Source model: Target_Board::Board.Impl
// Target memory map and peripheral base addresses derived from the AADL model.
#![allow(dead_code)]

// ── Memory regions ──────────────────────────────────────────
// Board.Impl/flash
pub const FLASH_BASE: usize = 0x08000000;
pub const FLASH_SIZE: usize = 0x00100000; // 1048576 bytes (1 MiB)
// Board.Impl/sram
pub const SRAM_BASE: usize = 0x20000000;
pub const SRAM_SIZE: usize = 0x00020000; // 131072 bytes (128 KiB)

// ── Peripheral base addresses ───────────────────────────────
// Board.Impl/uart1
pub const UART1_BASE: usize = 0x40011000;
// Board.Impl/gpioa
pub const GPIOA_BASE: usize = 0x40020000;
";
        assert_eq!(out, expected);
    }

    #[test]
    fn linker_golden() {
        let inst = board_instance();
        let out = emit_linker(&inst, "Target_Board::Board.Impl").expect("emit");
        let expected = "\
/* Generated by `spar emit --format linker`. DO NOT EDIT. */
/* Source model: Target_Board::Board.Impl */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1048576 /* 1048576 bytes (1 MiB) */
  SRAM : ORIGIN = 0x20000000, LENGTH = 131072 /* 131072 bytes (128 KiB) */
}
";
        assert_eq!(out, expected);
    }

    /// Oracle (the #338 round-trip): every emitted `*_BASE` / `*_SIZE` value must
    /// equal the value independently re-derived from the resolved instance
    /// property map — i.e. the constants faithfully encode the model, matching
    /// what `spar instance --json` would report. This re-reads the model through
    /// a *different* code path (raw property text) than the emitter's typed one.
    #[test]
    fn constants_match_instance_model() {
        let inst = board_instance();
        let out = emit_constants(&inst, "Target_Board::Board.Impl").expect("emit");

        // Independently collect (leaf-name -> base, size_bytes) from the model,
        // parsing raw property strings rather than the typed PropertyExpr.
        let mut expected: std::collections::BTreeMap<String, (u64, Option<u64>)> =
            std::collections::BTreeMap::new();
        for (idx, comp) in inst.all_components() {
            if !matches!(
                comp.category,
                ComponentCategory::Memory | ComponentCategory::Device
            ) {
                continue;
            }
            let props = inst.properties_for(idx);
            let base_raw = props
                .get("Memory_Properties", "Base_Address")
                .or_else(|| props.get("", "Base_Address"));
            let Some(base_raw) = base_raw else { continue };
            let base = parse_aadl_int(base_raw).expect("base parses");
            let size = props
                .get("Memory_Properties", "Byte_Count")
                .or_else(|| props.get("", "Byte_Count"))
                .and_then(parse_aadl_int)
                .or_else(|| {
                    props
                        .get("Memory_Properties", "Memory_Size")
                        .or_else(|| props.get("", "Memory_Size"))
                        .and_then(parse_aadl_size_bytes)
                });
            expected.insert(comp.name.as_str().to_ascii_uppercase(), (base, size));
        }

        // Every expected constant must appear in the emitted module with the
        // independently-derived value, and vice versa (no phantom constants).
        for (ident, (base, size)) in &expected {
            let base_line = format!("pub const {ident}_BASE: usize = 0x{base:08X};");
            assert!(
                out.contains(&base_line),
                "missing/incorrect base for {ident}: expected `{base_line}`\n--- emitted ---\n{out}"
            );
            if let Some(sz) = size {
                let size_line = format!("pub const {ident}_SIZE: usize = 0x{sz:08X};");
                assert!(
                    out.lines().any(|l| l.starts_with(&size_line)),
                    "missing/incorrect size for {ident}: expected `{size_line}`\n--- emitted ---\n{out}"
                );
            }
        }
        // The non-mapped device must not have produced a constant.
        assert!(
            !out.contains("STATUS_LED_BASE"),
            "non-mapped device leaked into output"
        );
        assert_eq!(expected.len(), 4, "expected 2 memory + 2 mapped devices");
    }

    /// Parse an AADL integer literal (decimal or based `base#digits#`) to u64.
    fn parse_aadl_int(raw: &str) -> Option<u64> {
        let s = raw.trim();
        if let Some(hash) = s.find('#') {
            let base: u32 = s[..hash].parse().ok()?;
            let rest = &s[hash + 1..];
            let end = rest.find('#')?;
            return u64::from_str_radix(rest[..end].trim(), base).ok();
        }
        s.split_whitespace().next()?.parse().ok()
    }

    /// Parse an AADL `Size` value (`N Unit`) to bytes.
    fn parse_aadl_size_bytes(raw: &str) -> Option<u64> {
        let mut it = raw.split_whitespace();
        let n: u64 = it.next()?.parse().ok()?;
        let unit = it.next().unwrap_or("Bytes");
        let bytes = match unit {
            "bits" => {
                return if n.is_multiple_of(8) {
                    Some(n / 8)
                } else {
                    None
                };
            }
            "Bytes" | "bytes" => 1,
            "KByte" => 1024,
            "MByte" => 1024 * 1024,
            "GByte" => 1024 * 1024 * 1024,
            _ => return None,
        };
        Some(n * bytes)
    }

    #[test]
    fn non_hardware_model_errors() {
        let aadl = "package P\npublic\n system S\n end S;\n system implementation S.Impl\n end S.Impl;\nend P;\n";
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "p.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("P"),
            &Name::new("S"),
            &Name::new("Impl"),
        );
        assert!(emit_constants(&inst, "P::S.Impl").is_err());
        assert!(emit_linker(&inst, "P::S.Impl").is_err());
    }

    #[test]
    fn ident_sanitizes() {
        assert_eq!(ident_from_path("root/sub/uart1"), "UART1");
        assert_eq!(ident_from_path("root/9pin"), "_9PIN");
        assert_eq!(ident_from_path("root/a-b.c"), "A_B_C");
    }

    /// Two memory components under different parents that share a leaf name
    /// collide on the derived identifier — the emitter must hard-error rather
    /// than silently drop or overwrite a constant.
    #[test]
    fn collision_is_hard_error() {
        let aadl = r#"
package Coll
public
  memory Mem
  end Mem;

  system Sub
  end Sub;

  system implementation Sub.Impl
    subcomponents
      flash: memory Mem {
        Base_Address => 16#08000000#;
      };
  end Sub.Impl;

  system Top
  end Top;

  system implementation Top.Impl
    subcomponents
      a: system Sub.Impl;
      b: system Sub.Impl;
  end Top.Impl;
end Coll;
"#;
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "coll.aadl".to_string(), aadl.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        let inst = SystemInstance::instantiate(
            &scope,
            &Name::new("Coll"),
            &Name::new("Top"),
            &Name::new("Impl"),
        );
        let err = emit_constants(&inst, "Coll::Top.Impl").expect_err("colliding leaf names");
        assert!(
            err.contains("collision") && err.contains("FLASH"),
            "unexpected error: {err}"
        );
    }
}
