//! Integration oracle for REQ-CODEGEN-WIT-RECORDS-001 (#319 items 4 & 7):
//! `Dispatch_Protocol`-derived lifecycle + WIT world ⟷ Rust `Guest` alignment.
//!
//! Two levels of oracle:
//!  1. **String alignment** (fast, always runs): the WIT world exports the exact
//!     lifecycle a thread's `Dispatch_Protocol` implies, and the generated
//!     bindings `impl Guest` names the matching methods.
//!  2. **Compiler-enforced** (`#[ignore]`, opt-in): emit the whole crate and run
//!     `cargo check`. wit-bindgen's `generate!` derives the `Guest` trait from
//!     the world, so a mangling / keyword / collision bug in the emitter is a
//!     compile error — the string test cannot catch a mismatch it replicates.
//!     Ignored because it shells out to `cargo` (network for the first build,
//!     disk for the target dir). Run with:
//!       `cargo test -p spar-codegen --test lifecycle_bindings -- --ignored`

use spar_codegen::rust_gen::generate_process_bindings;
use spar_codegen::wit_gen::generate_wit;
use spar_codegen::{CodegenConfig, OutputFormat, generate};
use spar_hir_def::instance::SystemInstance;
use spar_hir_def::item_tree::ComponentCategory;
use spar_hir_def::name::Name;
use spar_hir_def::resolver::GlobalScope;

/// A process `Ctrl` with a Periodic thread and a Sporadic thread, so the two
/// lifecycle shapes are exercised in one model.
const LIFECYCLE_AADL: &str = r#"
package LifePkg
public
    thread PeriodicWorker
        properties
            Thread_Properties::Dispatch_Protocol => Periodic;
    end PeriodicWorker;

    thread implementation PeriodicWorker.Impl
    end PeriodicWorker.Impl;

    thread SporadicWorker
        properties
            Thread_Properties::Dispatch_Protocol => Sporadic;
    end SporadicWorker;

    thread implementation SporadicWorker.Impl
    end SporadicWorker.Impl;

    process Ctrl
    end Ctrl;

    process implementation Ctrl.Impl
        subcomponents
            pw: thread PeriodicWorker.Impl;
            sw: thread SporadicWorker.Impl;
    end Ctrl.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            ctrl: process Ctrl.Impl;
    end Top.Impl;
end LifePkg;
"#;

fn build(aadl: &str, pkg: &str, typ: &str, imp: &str) -> (GlobalScope, SystemInstance) {
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "life.aadl".to_string(), aadl.to_string());
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    let inst =
        SystemInstance::instantiate(&scope, &Name::new(pkg), &Name::new(typ), &Name::new(imp));
    (scope, inst)
}

fn process_idx(inst: &SystemInstance) -> spar_hir_def::instance::ComponentInstanceIdx {
    inst.all_components()
        .find(|(_, c)| c.category == ComponentCategory::Process)
        .map(|(idx, _)| idx)
        .expect("model has a process")
}

/// Item 7: the WIT world exports the lifecycle each thread's `Dispatch_Protocol`
/// implies — Periodic → `compute` only; Sporadic → initialize/compute/finalize —
/// NOT the old opaque single `{thread}: func()`.
#[test]
fn world_exports_dispatch_derived_lifecycle() {
    let (scope, inst) = build(LIFECYCLE_AADL, "LifePkg", "Top", "Impl");
    let idx = process_idx(&inst);
    let wit = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
    let body = &wit.content;

    // Exports are keyed by the *subcomponent instance* name (`pw`, `sw`) — unique
    // within the process — not the thread type name.
    //
    // Periodic instance `pw`: compute only.
    assert!(
        body.contains("export pw-compute: func();"),
        "periodic instance must export compute:\n{body}"
    );
    assert!(
        !body.contains("export pw-initialize"),
        "periodic instance must NOT export initialize:\n{body}"
    );
    assert!(
        !body.contains("export pw-finalize"),
        "periodic instance must NOT export finalize:\n{body}"
    );

    // Sporadic instance `sw`: full lifecycle. If this fails with only `-compute`,
    // the Dispatch_Protocol did not resolve and defaulted to Periodic — a real
    // bug, not a fixture typo.
    for m in ["initialize", "compute", "finalize"] {
        assert!(
            body.contains(&format!("export sw-{m}: func();")),
            "sporadic instance must export {m} (Dispatch_Protocol must resolve):\n{body}"
        );
    }

    // The old opaque single-func export must be gone.
    assert!(
        !body.contains("export pw: func();"),
        "opaque per-thread export must be replaced by lifecycle exports:\n{body}"
    );
}

/// Items 4 & 7: the generated bindings crate root wires the world to Rust —
/// `generate!` + `impl Guest` (matching the world's lifecycle exports) + `export!`.
#[test]
fn bindings_wire_generate_guest_and_export() {
    let (_scope, inst) = build(LIFECYCLE_AADL, "LifePkg", "Top", "Impl");
    let idx = process_idx(&inst);
    let file = generate_process_bindings(&inst, idx);
    let src = &file.content;

    assert_eq!(
        file.path, "crates/ctrl/src/lib.rs",
        "bindings land at the process crate root"
    );
    assert!(
        src.contains("wit_bindgen::generate!({"),
        "must invoke wit_bindgen::generate!:\n{src}"
    );
    assert!(
        src.contains("world: \"ctrl-world\","),
        "generate! must target the process world:\n{src}"
    );
    assert!(
        src.contains("impl Guest for Component {"),
        "must impl the world's Guest trait:\n{src}"
    );
    assert!(
        src.contains("export!(Component);"),
        "must export the component:\n{src}"
    );

    // Guest methods match the world exports (instance name; kebab `-` → snake `_`).
    assert!(
        src.contains("fn pw_compute()"),
        "periodic instance → single compute method:\n{src}"
    );
    for m in ["initialize", "compute", "finalize"] {
        assert!(
            src.contains(&format!("fn sw_{m}()")),
            "sporadic instance → {m} method:\n{src}"
        );
    }
    assert!(
        !src.contains("fn pw_initialize()"),
        "periodic instance must not get initialize:\n{src}"
    );
}

/// A RECORD-bearing, MULTI-PROCESS, mixed-dispatch model: `ProcA` has a
/// record-typed port (`Snapshot.Impl` = u32 + u8) and a Periodic thread; `ProcB`
/// has a scalar port and a Sporadic thread. This is what the compile oracle must
/// exercise — the requirement's subject is record interfaces, and two processes
/// surface wit-package / `../../wit/*.wit` path collisions a single-process
/// fixture never would.
const RECORDS_MP_AADL: &str = r#"
package MpkgRec
public
    data Word32
        properties
            Data_Size => 4 Bytes;
    end Word32;

    data Byte8
        properties
            Data_Size => 1 Bytes;
    end Byte8;

    data Snapshot
    end Snapshot;

    data implementation Snapshot.Impl
        subcomponents
            addr: data Word32;
            flags: data Byte8;
    end Snapshot.Impl;

    thread WorkerA
        properties
            Thread_Properties::Dispatch_Protocol => Periodic;
    end WorkerA;

    thread implementation WorkerA.Impl
    end WorkerA.Impl;

    thread WorkerB
        properties
            Thread_Properties::Dispatch_Protocol => Sporadic;
    end WorkerB;

    thread implementation WorkerB.Impl
    end WorkerB.Impl;

    process ProcA
        features
            snap_in: in data port Snapshot.Impl;
    end ProcA;

    process implementation ProcA.Impl
        subcomponents
            wa: thread WorkerA.Impl;
    end ProcA.Impl;

    process ProcB
        features
            count_in: in data port Word32;
    end ProcB;

    process implementation ProcB.Impl
        subcomponents
            wb: thread WorkerB.Impl;
    end ProcB.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            pa: process ProcA.Impl;
            pb: process ProcB.Impl;
    end Top.Impl;
end MpkgRec;
"#;

/// COMPILER-ENFORCED oracle (opt-in): emit the full crate and `cargo check` it.
/// This is the real item-4/7 gate — it runs wit-bindgen's `generate!`, so the
/// `impl Guest` is type-checked against the world. A name-mangling or keyword
/// bug in the emitter fails here even when the string test above is green.
///
/// Uses the RECORD-bearing, MULTI-PROCESS fixture so the check covers a bindings
/// crate whose world imports a WIT `record` interface, and two member crates with
/// distinct wit packages — not just the portless single-process alignment case.
///
/// `#[ignore]` because it shells out to cargo. Run:
///   `cargo test -p spar-codegen --test lifecycle_bindings -- --ignored`
#[test]
#[ignore = "shells out to cargo check (network/disk); run with --ignored"]
fn emitted_crate_cargo_checks() {
    use std::process::Command;

    let (scope, inst) = build(RECORDS_MP_AADL, "MpkgRec", "Top", "Impl");
    let config = CodegenConfig {
        root_name: "life".into(),
        output_dir: "out".into(),
        format: OutputFormat::Both,
        verify: None,
        rivet: false,
        dry_run: true,
    };
    let out = generate(&inst, &scope, &config).expect("codegen should succeed");

    // Materialize the emitted workspace into a temp dir.
    let root = std::env::temp_dir().join("spar-p3-bindings-oracle");
    let _ = std::fs::remove_dir_all(&root);
    for f in &out.files {
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &f.content).unwrap();
    }

    // A dedicated target dir on the internal disk avoids the global 512GB
    // redirect (which may be full) and file-lock contention.
    let target_dir = std::env::temp_dir().join("spar-p3-bindings-target");
    let status = Command::new("cargo")
        .arg("check")
        .current_dir(&root)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to spawn cargo check");

    assert!(
        status.success(),
        "generated crate failed to `cargo check` (world⟷Guest wiring is broken); \
         inspect {}",
        root.display()
    );
}
