//! RUNTIME data-flow oracle for REQ-CODEGEN-WIT-DATAPLANE-001 (oracle #3).
//!
//! The routing unit test proves the topology, `cargo check` proves the marshalling
//! is ABI-valid, and the wasip2 build proves it componentizes — but NONE of those
//! proves a byte actually moves at runtime. This harness closes that gap (the #294
//! trap: a requirement claiming data flow must not rest on a gate proving only
//! compilation):
//!
//!   1. generate the DATAPLANE crate via `spar_codegen::generate`;
//!   2. patch the `e` thread's `compute` into an ECHO (`dout = din`,
//!      `rec_out = rec_in`) — this stands in for USER logic; the code under test is
//!      spar's generated marshalling glue, not the echo;
//!   3. build it to a `wasm32-wasip2` COMPONENT;
//!   4. instantiate under wasmtime with a host that implements the imported
//!      `p-ports` interface, feeding SENTINEL-distinct values, call `e-compute`,
//!      and assert the exact values came back out.
//!
//! `bindgen!` is compile-time but the `.wit` is generated at runtime, so the host
//! binds against a COMMITTED `wit/dp.wit`; a fast test in spar-codegen golden-locks
//! that `generate()` reproduces it byte-for-byte, so the two cannot drift.

use anyhow::{Context, Result, bail};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

// Host-side bindings generated from the SAME committed WIT the codegen emits.
wasmtime::component::bindgen!({
    world: "p-world",
    path: "wit/dp.wit",
});

use dp_pkg::p::p_ports::{Host as PortsHost, SnapshotImpl};

/// Sentinel inputs the host feeds the component (non-zero, per-field distinct so a
/// dead path or field-collapse cannot masquerade as success).
const PIN_INPUT: u32 = 0xDEAD_BEEF;
const SNAP_ADDR_INPUT: u32 = 0xCAFE_BABE;
const SNAP_FLAGS_INPUT: u8 = 0x5A;

/// Store state: WASI (the component's std imports) + the port I/O slots. The
/// `*_received` slots start at a value DISTINCT from the inputs, so if `compute`
/// never wrote them the final assertion fails instead of a false pass.
struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    pout_received: u32,
    psnap_received: SnapshotImpl,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl PortsHost for HostState {
    fn pin(&mut self) -> u32 {
        PIN_INPUT
    }
    fn set_pout(&mut self, val: u32) {
        self.pout_received = val;
    }
    fn psnap_in(&mut self) -> SnapshotImpl {
        SnapshotImpl {
            addr: SNAP_ADDR_INPUT,
            flags: SNAP_FLAGS_INPUT,
        }
    }
    fn set_psnap_out(&mut self, val: SnapshotImpl) {
        self.psnap_received = val;
    }
}

const DATAPLANE_AADL: &str = include_str!("../../test-data/codegen/dataplane.aadl");

fn main() -> Result<()> {
    // 1. Generate the data-plane crate.
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "dp.aadl".to_string(), DATAPLANE_AADL.to_string());
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = spar_hir_def::resolver::GlobalScope::from_trees(vec![tree]);
    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::name::Name::new("DpPkg"),
        &spar_hir_def::name::Name::new("Top"),
        &spar_hir_def::name::Name::new("Impl"),
    );
    let config = spar_codegen::CodegenConfig {
        root_name: "dp".into(),
        output_dir: "out".into(),
        format: spar_codegen::OutputFormat::Both,
        verify: None,
        rivet: false,
        dry_run: true,
    };
    let out = spar_codegen::generate(&inst, &scope, &config).context("codegen failed")?;

    // 2. Materialize + patch the echo body.
    let root = std::env::temp_dir().join("spar-dataplane-exec-oracle");
    let _ = std::fs::remove_dir_all(&root);
    for f in &out.files {
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let content = if f.path == "crates/p/src/e.rs" {
            patch_echo(&f.content)?
        } else {
            f.content.clone()
        };
        std::fs::write(&path, content)?;
    }

    // 3. Build to a wasm32-wasip2 component.
    let target_dir = std::env::temp_dir().join("spar-dataplane-exec-target");
    let status = std::process::Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        .current_dir(&root)
        .env("CARGO_INCREMENTAL", "0")
        .env("CARGO_TARGET_DIR", &target_dir)
        .status()
        .context("failed to spawn cargo build")?;
    if !status.success() {
        bail!("data-plane crate failed to build to wasm32-wasip2; inspect {root:?}");
    }
    let wasm = target_dir.join("wasm32-wasip2/debug/p.wasm");

    // 4. Instantiate under wasmtime and prove the round-trip.
    let engine = Engine::default();
    let component = Component::from_file(&engine, &wasm)
        .with_context(|| format!("wasmtime could not load {wasm:?}"))?;

    let mut linker: Linker<HostState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).context("add wasi to linker")?;
    PWorld::add_to_linker::<_, wasmtime::component::HasSelf<_>>(&mut linker, |s: &mut HostState| s)
        .context("add p-ports to linker")?;

    let mut store = Store::new(
        &engine,
        HostState {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            // sentinel: distinct from every input.
            pout_received: 0,
            psnap_received: SnapshotImpl { addr: 0, flags: 0 },
        },
    );

    let world = PWorld::instantiate(&mut store, &component, &linker).context("instantiate")?;
    world.call_e_compute(&mut store).context("call e-compute")?;

    // The echo copies In -> Out; the marshalling glue is what carried the bytes
    // across the ABI in both directions. Assert EXACT equality against the inputs.
    let s = store.data();
    if s.pout_received != PIN_INPUT {
        bail!(
            "scalar did NOT flow: fed pin={PIN_INPUT:#x}, got pout={:#x} (0 = compute never wrote it)",
            s.pout_received
        );
    }
    if s.psnap_received.addr != SNAP_ADDR_INPUT || s.psnap_received.flags != SNAP_FLAGS_INPUT {
        bail!(
            "record did NOT flow: fed {{addr:{SNAP_ADDR_INPUT:#x}, flags:{SNAP_FLAGS_INPUT:#x}}}, \
             got {{addr:{:#x}, flags:{:#x}}}",
            s.psnap_received.addr,
            s.psnap_received.flags
        );
    }

    println!(
        "EXEC ORACLE PASSED: scalar pin->dout->pout ({PIN_INPUT:#x}) and record \
         psnap_in->rec_out->psnap_out ({SNAP_ADDR_INPUT:#x}/{SNAP_FLAGS_INPUT:#x}) flowed across the WIT ABI"
    );
    Ok(())
}

/// Turn the generated (empty) `E::compute` skeleton into an echo, standing in for
/// user logic. The replacement MUST fire exactly once — a silent no-op (codegen
/// formatting drift) would leave `compute` empty and the outputs at their default,
/// which a naive harness would then read as "no flow" AND blame the marshalling.
fn patch_echo(src: &str) -> Result<String> {
    let needle =
        "    fn compute(&mut self, _ports: &mut EPorts) {\n        // TODO: compute logic\n    }";
    let echo = "    fn compute(&mut self, ports: &mut EPorts) {\n        ports.dout = ports.din;\n        ports.rec_out = ports.rec_in.clone();\n    }";
    let n = src.matches(needle).count();
    if n != 1 {
        bail!("echo patch expected exactly 1 match of the compute skeleton, found {n}");
    }
    Ok(src.replace(needle, echo))
}
