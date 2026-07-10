//! Integration oracle for REQ-CODEGEN-WIT-RECORDS-001 (#319 items 4 & 7):
//! `Dispatch_Protocol`-derived lifecycle + WIT world ⟷ Rust `Guest` alignment.
//!
//! Two levels of oracle:
//! 1. **String alignment** (fast, always runs): the WIT world exports the exact
//!    lifecycle a thread's `Dispatch_Protocol` implies, and the generated
//!    bindings `impl Guest` names the matching methods.
//! 2. **Compiler-enforced** (`#[ignore]`, opt-in): emit the whole crate and run
//!    `cargo check`. wit-bindgen's `generate!` derives the `Guest` trait from
//!    the world, so a mangling / keyword / collision bug in the emitter is a
//!    compile error — the string test cannot catch a mismatch it replicates.
//!    Ignored because it shells out to `cargo` (network for the first build,
//!    disk for the target dir). Run with:
//!    `cargo test -p spar-codegen --test lifecycle_bindings -- --ignored`

use spar_codegen::rust_gen::{PortRoute, RouteDir, generate_process_bindings, resolve_port_routes};
use spar_codegen::wit_gen::generate_wit;
use spar_codegen::{CodegenConfig, OutputFormat, generate};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
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
    let (scope, inst) = build(LIFECYCLE_AADL, "LifePkg", "Top", "Impl");
    let idx = process_idx(&inst);
    let file = generate_process_bindings(&inst, &scope, idx);
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

/// FULL-PATH data-plane fixture (REQ-CODEGEN-WIT-DATAPLANE-001). `Proc` has
/// scalar AND record boundary ports (in+out) connected through to the `e` (Echo)
/// thread's ports, PLUS the two blind-spot cases in one model:
///   * `e.unwired` — a thread In port with NO process connection → no route.
///   * `prod.p_out -> cons.c_in` — an inter-thread connection (both ends
///     `Some(...)`) → must be SKIPPED, not misrouted as a boundary port.
const DATAPLANE_AADL: &str = r#"
package DpPkg
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

    thread Echo
        features
            din: in data port Word32;
            dout: out data port Word32;
            rec_in: in data port Snapshot.Impl;
            rec_out: out data port Snapshot.Impl;
            unwired: in data port Word32;
    end Echo;

    thread implementation Echo.Impl
    end Echo.Impl;

    thread Producer
        features
            p_out: out data port Word32;
    end Producer;

    thread implementation Producer.Impl
    end Producer.Impl;

    thread Consumer
        features
            c_in: in data port Word32;
    end Consumer;

    thread implementation Consumer.Impl
    end Consumer.Impl;

    process Proc
        features
            pin: in data port Word32;
            pout: out data port Word32;
            psnap_in: in data port Snapshot.Impl;
            psnap_out: out data port Snapshot.Impl;
    end Proc;

    process implementation Proc.Impl
        subcomponents
            e: thread Echo.Impl;
            prod: thread Producer.Impl;
            cons: thread Consumer.Impl;
        connections
            c1: port pin -> e.din;
            c2: port e.dout -> pout;
            c3: port psnap_in -> e.rec_in;
            c4: port e.rec_out -> psnap_out;
            c5: port prod.p_out -> cons.c_in;
    end Proc.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            p: process Proc.Impl;
    end Top.Impl;
end DpPkg;
"#;

/// Find a thread instance by its subcomponent instance name.
fn thread_idx_by_name(inst: &SystemInstance, name: &str) -> ComponentInstanceIdx {
    inst.all_components()
        .find(|(_, c)| c.category == ComponentCategory::Thread && c.name.as_str() == name)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| panic!("model has a thread named `{name}`"))
}

/// ROUTING RESOLVER oracle (REQ-CODEGEN-WIT-DATAPLANE-001, oracle #1): the pure
/// `SystemInstance -> per-thread routes` resolver is asserted EXACT against the
/// full-path fixture, including the two blind spots. This is the executed proof
/// that the process<->thread topology traversal is correct BEFORE any bytes move
/// — a compile/exec oracle cannot localize a misrouted port the way this can.
#[test]
fn routing_resolver_is_exact_including_blind_spots() {
    let (_scope, inst) = build(DATAPLANE_AADL, "DpPkg", "Top", "Impl");

    // Echo thread: scalar in/out + record in/out, in process connection order.
    // `unwired` has no connection, so it MUST NOT appear.
    let echo = resolve_port_routes(&inst, thread_idx_by_name(&inst, "e"));
    let expected = vec![
        PortRoute {
            thread_feature: "din".into(),
            boundary_feature: "pin".into(),
            dir: RouteDir::In,
        },
        PortRoute {
            thread_feature: "dout".into(),
            boundary_feature: "pout".into(),
            dir: RouteDir::Out,
        },
        PortRoute {
            thread_feature: "rec_in".into(),
            boundary_feature: "psnap_in".into(),
            dir: RouteDir::In,
        },
        PortRoute {
            thread_feature: "rec_out".into(),
            boundary_feature: "psnap_out".into(),
            dir: RouteDir::Out,
        },
    ];
    assert_eq!(
        echo, expected,
        "Echo routes must be exact + in connection order, with `unwired` absent"
    );
    assert!(
        !echo.iter().any(|r| r.thread_feature == "unwired"),
        "unconnected thread port must yield no route: {echo:?}"
    );

    // Blind spot: the inter-thread connection `prod.p_out -> cons.c_in` has BOTH
    // ends `Some(...)`, so NEITHER thread sees it as a boundary route.
    let prod = resolve_port_routes(&inst, thread_idx_by_name(&inst, "prod"));
    let cons = resolve_port_routes(&inst, thread_idx_by_name(&inst, "cons"));
    assert!(
        prod.is_empty(),
        "producer's only connection is inter-thread → no boundary route: {prod:?}"
    );
    assert!(
        cons.is_empty(),
        "consumer's only connection is inter-thread → no boundary route: {cons:?}"
    );
}

/// MARSHALLING string oracle (REQ-CODEGEN-WIT-DATAPLANE-001): the generated Guest
/// `compute` reads In ports from the imported funcs before compute and writes Out
/// ports after — scalars directly, records via a `From`/`Into` bridge. This is the
/// fast (non-ignored) shape check; the compile + wasmtime-exec oracles prove it is
/// ABI-valid and that bytes actually flow.
#[test]
fn compute_body_marshals_scalar_and_record_ports() {
    let (scope, inst) = build(DATAPLANE_AADL, "DpPkg", "Top", "Impl");
    let idx = process_idx(&inst);
    let src = generate_process_bindings(&inst, &scope, idx).content;

    // Scalar In read + Out write, direct (no conversion).
    assert!(
        src.contains("ports.din = crate::dp_pkg::p::p_ports::pin();"),
        "scalar In port must be read from the imported func:\n{src}"
    );
    assert!(
        src.contains("crate::dp_pkg::p::p_ports::set_pout(ports.dout);"),
        "scalar Out port must be written to the imported func:\n{src}"
    );
    // Record In read + Out write, through the `.into()` bridge.
    assert!(
        src.contains("ports.rec_in = crate::dp_pkg::p::p_ports::psnap_in().into();"),
        "record In port must marshal through the From bridge:\n{src}"
    );
    assert!(
        src.contains("crate::dp_pkg::p::p_ports::set_psnap_out(ports.rec_out.into());"),
        "record Out port must marshal through the Into bridge:\n{src}"
    );
    // Both bridge directions are emitted for the record type.
    assert!(
        src.contains(
            "impl From<crate::dp_pkg::p::p_ports::SnapshotImpl> for crate::types::SnapshotImpl"
        ) && src.contains(
            "impl From<crate::types::SnapshotImpl> for crate::dp_pkg::p::p_ports::SnapshotImpl"
        ),
        "both From directions for the record must be generated:\n{src}"
    );
    // The inter-thread threads (prod/cons) have NO marshalling — their only
    // connection is thread->thread, deferred. Their compute bodies must contain
    // no imported-port call.
    let prod_body = src
        .split("fn prod_compute()")
        .nth(1)
        .and_then(|s| s.split("fn ").next())
        .unwrap_or("");
    assert!(
        !prod_body.contains("p_ports::"),
        "inter-thread producer must not marshal any boundary port:\n{prod_body}"
    );
    // `unwired` (unconnected thread In port) must NOT be marshalled.
    assert!(
        !src.contains("ports.unwired ="),
        "unconnected thread port must not be marshalled:\n{src}"
    );
}

/// COMPILE oracle for the DATA PLANE (REQ-CODEGEN-WIT-DATAPLANE-001, oracle #2):
/// emit the DATAPLANE crate and `cargo check` it. This type-checks the marshalling
/// against the REAL wit-bindgen `generate!` output — the imported-func module path
/// (`crate::dp_pkg::p::p_ports::*`), the scalar-alias transparency, and the record
/// `From`/`Into` bridges. A wrong module path, port name, or bridge field is a
/// compile error here even when the string oracle above is green.
///
/// `#[ignore]` (shells out to cargo). Run:
///   `cargo test -p spar-codegen --test lifecycle_bindings -- --ignored`
#[test]
#[ignore = "shells out to cargo check (network/disk); run with --ignored"]
fn emitted_dataplane_crate_cargo_checks() {
    use std::process::Command;

    let (scope, inst) = build(DATAPLANE_AADL, "DpPkg", "Top", "Impl");
    let config = CodegenConfig {
        root_name: "dp".into(),
        output_dir: "out".into(),
        format: OutputFormat::Both,
        verify: None,
        rivet: false,
        dry_run: true,
    };
    let out = generate(&inst, &scope, &config).expect("codegen should succeed");

    let root = std::env::temp_dir().join("spar-dataplane-oracle");
    let _ = std::fs::remove_dir_all(&root);
    for f in &out.files {
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &f.content).unwrap();
    }

    let target_dir = std::env::temp_dir().join("spar-dataplane-target");
    let status = Command::new("cargo")
        .arg("check")
        .current_dir(&root)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to spawn cargo check");

    assert!(
        status.success(),
        "generated data-plane crate failed to `cargo check` (marshalling/bridge \
         wiring is broken); inspect {}",
        root.display()
    );
}

/// A model with an OPAQUE top-level port (a `data` type with no Data_Size and no
/// subcomponents). This is the negative control for the no_alloc oracle: wit_gen
/// deliberately keeps the `list<u8>` fallback here (P2a), so the generated WIT
/// MUST contain `list<u8>` — proving the no-alloc check can actually fire.
const OPAQUE_PORT_AADL: &str = r#"
package OpaquePkg
public
    data Blob
    end Blob;

    thread Worker
        properties
            Thread_Properties::Dispatch_Protocol => Periodic;
    end Worker;

    thread implementation Worker.Impl
    end Worker.Impl;

    process Proc
        features
            blob_in: in data port Blob;
    end Proc;

    process implementation Proc.Impl
        subcomponents
            w: thread Worker.Impl;
    end Proc.Impl;

    system Top
    end Top;

    system implementation Top.Impl
        subcomponents
            p: process Proc.Impl;
    end Top.Impl;
end OpaquePkg;
"#;

/// NO-ALLOC ORACLE (REQ-CODEGEN-WIT-RECORDS-002 item 3c). The sound signal is at
/// the WIT level: `list`/`string` are the only unbounded canonical-ABI types (the
/// ones forcing dynamic `cabi_realloc`); everything scalar/record is bounded. A
/// probe (2026-07-08) showed the wasm `cabi_realloc` SYMBOL is unconditional
/// wit-bindgen-rt glue — present in both alloc and no-alloc components — so a
/// symbol check cannot discriminate. This checks the WIT instead.
#[test]
fn no_alloc_all_scalar_record_model_has_no_unbounded_wit_types() {
    let (scope, inst) = build(RECORDS_MP_AADL, "MpkgRec", "Top", "Impl");
    // Check every process's generated WIT.
    let mut checked = 0;
    for (idx, comp) in inst.all_components() {
        if comp.category != ComponentCategory::Process {
            continue;
        }
        let wit = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
        assert!(
            !wit.content.contains("list<") && !wit.content.contains("string"),
            "all-scalar/record model must emit no unbounded (list/string) WIT \
             types:\n{}",
            wit.content
        );
        checked += 1;
    }
    assert_eq!(checked, 2, "fixture has two processes");
}

/// Negative control: the no_alloc oracle can fire. An opaque top-level port keeps
/// the `list<u8>` fallback, so its WIT MUST contain `list<u8>` — confirming the
/// assertion above is not vacuous, and marking the scope boundary (opaque ports
/// are out of the no-alloc guarantee).
#[test]
fn no_alloc_opaque_port_model_does_contain_list_fallback() {
    let (scope, inst) = build(OPAQUE_PORT_AADL, "OpaquePkg", "Top", "Impl");
    let idx = process_idx(&inst);
    let wit = generate_wit(&inst, &scope, idx).expect("wit generation should succeed");
    assert!(
        wit.content.contains("list<u8>"),
        "opaque port must fall back to list<u8> (negative control):\n{}",
        wit.content
    );
}

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

/// STRONGER EVIDENCE (opt-in, not a completion gate): the emitted crate builds all
/// the way to the real deployment target `wasm32-wasip2` — proving the records +
/// bindings + delegation survive componentization, not just a host type-check.
/// Per REQ-CODEGEN-WIT-RECORDS-002 this is nice-to-have; the required no-alloc
/// oracle is the WIT-level check above (a wasm `cabi_realloc` symbol is
/// unconditional wit-bindgen-rt glue and cannot discriminate).
///
/// `#[ignore]` — needs the wasm32-wasip2 target + shells out to cargo. Run:
///   `cargo test -p spar-codegen --test lifecycle_bindings -- --ignored`
#[test]
#[ignore = "needs wasm32-wasip2 target + shells out to cargo; run with --ignored"]
fn emitted_crate_builds_to_wasm32_wasip2() {
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

    let root = std::env::temp_dir().join("spar-p3-wasm-oracle");
    let _ = std::fs::remove_dir_all(&root);
    for f in &out.files {
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &f.content).unwrap();
    }

    let target_dir = std::env::temp_dir().join("spar-p3-wasm-target");
    let status = Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        .current_dir(&root)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .expect("failed to spawn cargo build");

    assert!(
        status.success(),
        "generated crate failed to build to wasm32-wasip2; inspect {}",
        root.display()
    );
}
