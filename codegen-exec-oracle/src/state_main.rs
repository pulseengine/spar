//! RUNTIME oracle for REQ-CODEGEN-WIT-STATE-001: persistent component state +
//! inter-thread connections, proven at runtime under wasmtime (the load-bearing
//! gate — the fast tests prove the wiring shape, this proves behavior).
//!
//! Two proofs on ONE live instance (state persists only within a live instance):
//!   1. PERSISTENCE — an Accum component accumulates across dispatches. Feed 5 then
//!      7 to `acc-in`; assert `acc-out == 12` on the second dispatch. Fresh-construct
//!      or a call-local buffer would yield 7 — the number discriminates persistence.
//!   2. INTER-THREAD — Producer echoes its boundary-in to its Out port; that value
//!      crosses a thread_local buffer to Consumer's In port, which Consumer echoes to
//!      its boundary-out. Feed 42 to `src-in`, dispatch `pr` then `cn` SEPARATELY,
//!      assert `sink-out == 42` — the buffer surviving between the two dispatches is
//!      what's under test.
//!
//! Both are sabotaged in the harness comments' spirit: revert the thread_local to a
//! per-call Default and persistence drops to 7; drop the buffer write and inter-
//! thread drops to 0.

use anyhow::{Context, Result, bail};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    world: "p-world",
    path: "wit/state.wit",
});

use state_pkg::p::p_ports::Host as PortsHost;

/// Store state: WASI + host-provided inputs + sentinel-initialized received slots.
struct HostState {
    table: ResourceTable,
    wasi: WasiCtx,
    acc_in_val: u32,
    src_in_val: u32,
    acc_out_recv: u32,
    sink_out_recv: u32,
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
    fn acc_in(&mut self) -> u32 {
        self.acc_in_val
    }
    fn set_acc_out(&mut self, val: u32) {
        self.acc_out_recv = val;
    }
    fn src_in(&mut self) -> u32 {
        self.src_in_val
    }
    fn set_sink_out(&mut self, val: u32) {
        self.sink_out_recv = val;
    }
}

const STATE_AADL: &str = include_str!("../../test-data/codegen/state.aadl");

fn main() -> Result<()> {
    // 1. Generate the STATE crate.
    let db = spar_hir_def::HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "state.aadl".to_string(), STATE_AADL.to_string());
    let tree = spar_hir_def::file_item_tree(&db, sf);
    let scope = spar_hir_def::resolver::GlobalScope::from_trees(vec![tree]);
    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::name::Name::new("StatePkg"),
        &spar_hir_def::name::Name::new("Top"),
        &spar_hir_def::name::Name::new("Impl"),
    );
    let config = spar_codegen::CodegenConfig {
        root_name: "st".into(),
        output_dir: "out".into(),
        format: spar_codegen::OutputFormat::Both,
        verify: None,
        rivet: false,
        dry_run: true,
    };
    let out = spar_codegen::generate(&inst, &scope, &config).context("codegen failed")?;

    // 2. Materialize + patch user component logic (stands in for hand-written code;
    //    the code UNDER TEST is spar's generated state/inter-thread wiring).
    let root = std::env::temp_dir().join("spar-state-exec-oracle");
    let _ = std::fs::remove_dir_all(&root);
    for f in &out.files {
        let path = root.join(&f.path);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let content = match f.path.as_str() {
            "crates/p/src/a.rs" => patch_accumulator(&f.content)?,
            "crates/p/src/pr.rs" => patch_echo(&f.content, "Pr", "po", "pi")?,
            "crates/p/src/cn.rs" => patch_echo(&f.content, "Cn", "co", "ci")?,
            _ => f.content.clone(),
        };
        std::fs::write(&path, content)?;
    }

    // 3. Build to a wasm32-wasip2 component.
    let target_dir = std::env::temp_dir().join("spar-state-exec-target");
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
        bail!("STATE crate failed to build to wasm32-wasip2; inspect {root:?}");
    }
    let wasm = target_dir.join("wasm32-wasip2/debug/p.wasm");

    // 4. Instantiate ONCE and run both proofs on the same live instance.
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
            acc_in_val: 0,
            src_in_val: 0,
            acc_out_recv: 0,  // sentinel != 12
            sink_out_recv: 0, // sentinel != 42
        },
    );
    let world = PWorld::instantiate(&mut store, &component, &linker).context("instantiate")?;

    // --- Proof 1: persistence. Two dispatches on the same instance, 5 then 7. ---
    store.data_mut().acc_in_val = 5;
    world.call_a_compute(&mut store).context("a-compute #1")?;
    store.data_mut().acc_in_val = 7;
    world.call_a_compute(&mut store).context("a-compute #2")?;
    let acc = store.data().acc_out_recv;
    if acc != 12 {
        bail!(
            "state did NOT persist: fed 5 then 7, expected acc-out=12, got {acc} \
             (7 = fresh-construct or call-local; 0 = never written)"
        );
    }

    // --- Proof 2: inter-thread. Producer -> buffer -> Consumer, separate dispatches. ---
    store.data_mut().src_in_val = 42;
    world.call_pr_compute(&mut store).context("pr-compute")?;
    world.call_cn_compute(&mut store).context("cn-compute")?;
    let sink = store.data().sink_out_recv;
    if sink != 42 {
        bail!(
            "inter-thread did NOT deliver: fed src-in=42, expected sink-out=42, got {sink} \
             (0 = buffer never carried the value)"
        );
    }

    println!(
        "STATE ORACLE PASSED: persistence acc(5,7)->12 across two dispatches, and \
         inter-thread pr.po->buffer->cn.ci->sink-out delivered 42"
    );
    Ok(())
}

/// Replace exactly one occurrence of `needle` with `replacement`, or fail.
fn patch_once(src: &str, needle: &str, replacement: &str, what: &str) -> Result<String> {
    let n = src.matches(needle).count();
    if n != 1 {
        bail!("{what}: expected exactly 1 match, found {n}");
    }
    Ok(src.replace(needle, replacement))
}

/// Give the Accum component a persistent `acc` field and a compute that accumulates
/// its In port into it and writes the running total to its Out port.
fn patch_accumulator(src: &str) -> Result<String> {
    let src = patch_once(
        src,
        "#[derive(Default)]\npub struct ADefault;",
        "#[derive(Default)]\npub struct ADefault { acc: u32 }",
        "accumulator state field",
    )?;
    patch_once(
        &src,
        "    fn compute(&mut self, _ports: &mut APorts) {\n        // TODO: compute logic\n    }",
        "    fn compute(&mut self, ports: &mut APorts) {\n        self.acc = self.acc.wrapping_add(ports.ai);\n        ports.ao = self.acc;\n    }",
        "accumulator compute",
    )
}

/// Give a thread's compute an echo body copying its In port to its Out port.
fn patch_echo(src: &str, pascal: &str, out_field: &str, in_field: &str) -> Result<String> {
    let needle = format!(
        "    fn compute(&mut self, _ports: &mut {pascal}Ports) {{\n        // TODO: compute logic\n    }}"
    );
    let repl = format!(
        "    fn compute(&mut self, ports: &mut {pascal}Ports) {{\n        ports.{out_field} = ports.{in_field};\n    }}"
    );
    patch_once(src, &needle, &repl, &format!("{pascal} echo compute"))
}
