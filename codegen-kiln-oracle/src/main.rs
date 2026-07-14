//! RUNTIME oracle for REQ-CODEGEN-WIT-DATAPLANE-KILN-001: a spar-generated
//! component, executed on the **kiln** interpreter (a second runtime beside
//! wasmtime), proving the generated data-plane + state wiring behaves.
//!
//! This dogfoods the full PulseEngine chain end-to-end:
//!   spar codegen → wasm32-wasip2 component → `meld fuse` (in-process, meld-core)
//!   → CORE module → kiln `CapabilityAwareEngine` executes it, driving the custom
//!   AADL-port host imports through a chained host handler.
//!
//! Why meld is in the loop: kiln runs CORE modules, not Component Model
//! components (kiln#269 — direct component instantiation is a won't-fix path that
//! still panics, kiln#427). meld lowers the component to a core module, flattening
//! the `state-pkg:p/p-ports` interface into flat core (module, field) imports.
//!
//! Two proofs on ONE live kiln instance (state persists only within a live
//! instance — the same discriminator the wasmtime state-oracle uses):
//!   1. PERSISTENCE — feed acc-in 5 then 7 across two `a-compute` dispatches;
//!      assert set-acc-out == 12. A fresh instance / call-local buffer yields 7.
//!   2. INTER-THREAD — feed src-in=42, dispatch `pr-compute` then `cn-compute`
//!      SEPARATELY; assert set-sink-out == 42. The thread_local buffer surviving
//!      between the two dispatches (inside the one kiln instance) is under test.
//!
//! Sabotage (non-vacuity): revert the generated thread_local state to a per-call
//! Default and persistence drops to 7; drop the inter-thread buffer write and the
//! inter-thread proof drops to 0. The numbers discriminate.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use kiln_foundation::traits::{HostImportHandler, MemoryAccessor};
use kiln_foundation::values::Value;
use kiln_runtime::engine::{CapabilityAwareEngine, CapabilityEngine, EnginePreset};

const STATE_AADL: &str = include_str!("../../test-data/codegen/state.aadl");
const PORTS: &str = "state-pkg:p/p-ports";

/// Host side of the AADL ports. Feeds are set by the harness between dispatches;
/// received slots are written by the guest via set-* imports.
#[derive(Default)]
struct HostState {
    acc_in_feed: i32,
    src_in_feed: i32,
    acc_out_recv: Option<i32>,
    sink_out_recv: Option<i32>,
}

fn as_i32(v: Option<&Value>) -> Option<i32> {
    match v {
        Some(Value::I32(n)) => Some(*n),
        Some(Value::U32(n)) => Some(*n as i32),
        _ => None,
    }
}

/// kiln routes EVERY runtime import call through the single engine host_handler,
/// so we multiplex wasi:* → WasiDispatcher and the custom AADL ports → host state
/// (the kilnd InterComponentHandler pattern). register_host_function alone only
/// resolves imports at load/instantiate — runtime dispatch needs this handler.
struct PortsHandler {
    wasi: kiln_wasi::WasiDispatcher,
    state: Arc<Mutex<HostState>>,
}

impl HostImportHandler for PortsHandler {
    fn call_import(
        &mut self,
        module: &str,
        function: &str,
        args: &[Value],
        memory: Option<&dyn MemoryAccessor>,
    ) -> kiln_error::Result<Vec<Value>> {
        if module.starts_with("wasi:") {
            return self.wasi.call_import(module, function, args, memory);
        }
        let mut st = self.state.lock().unwrap();
        match function {
            "acc-in" => Ok(vec![Value::I32(st.acc_in_feed)]),
            "src-in" => Ok(vec![Value::I32(st.src_in_feed)]),
            "set-acc-out" => {
                st.acc_out_recv = as_i32(args.first());
                Ok(vec![])
            }
            "set-sink-out" => {
                st.sink_out_recv = as_i32(args.first());
                Ok(vec![])
            }
            other => Err(kiln_error::Error::runtime_error(Box::leak(
                format!("unexpected custom import: {module}/{other}").into_boxed_str(),
            ))),
        }
    }
}

fn main() -> Result<()> {
    // 1. Generate the STATE crate (same source the wasmtime state-oracle uses).
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

    // 2. Materialize + patch user component logic (the code UNDER TEST is spar's
    //    generated state/inter-thread wiring; the patches stand in for hand-written
    //    accumulate/echo bodies).
    let root = std::env::temp_dir().join("spar-kiln-oracle");
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
    let target_dir = std::env::temp_dir().join("spar-kiln-oracle-target");
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
    let wasm_path = target_dir.join("wasm32-wasip2/debug/p.wasm");
    let component_bytes = std::fs::read(&wasm_path)
        .with_context(|| format!("reading generated component {wasm_path:?}"))?;

    // 4. meld fuse the component → CORE module (in-process).
    let mut fuser = meld_core::Fuser::new(meld_core::FuserConfig::default());
    fuser
        .add_component_named(&component_bytes, Some("p"))
        .context("meld: add_component_named")?;
    let core_bytes = fuser.fuse().context("meld: fuse to core module")?;
    println!(
        "meld fused {} B component -> {} B core module",
        component_bytes.len(),
        core_bytes.len()
    );

    // 5. Run the CORE module on kiln, both proofs on ONE live instance.
    kiln_foundation::memory_init::MemoryInitializer::initialize()
        .map_err(|e| anyhow::anyhow!("MemoryInitializer::initialize: {e:?}"))?;

    let state = Arc::new(Mutex::new(HostState::default()));

    let mut engine = CapabilityAwareEngine::with_preset(EnginePreset::QM)
        .map_err(|e| anyhow::anyhow!("with_preset: {e:?}"))?;
    engine
        .enable_wasi()
        .map_err(|e| anyhow::anyhow!("enable_wasi: {e:?}"))?;

    // Stubs resolve the custom imports at load/instantiate; the handler below is
    // the actual runtime dispatch path.
    for f in ["acc-in", "set-acc-out", "src-in", "set-sink-out"] {
        engine
            .register_host_function(PORTS, f, |_a: &[Value]| Ok(vec![]))
            .map_err(|e| anyhow::anyhow!("register {f}: {e:?}"))?;
    }
    let wasi = kiln_wasi::WasiDispatcher::with_defaults()
        .map_err(|e| anyhow::anyhow!("WasiDispatcher::with_defaults: {e:?}"))?;
    engine.set_host_handler(Box::new(PortsHandler {
        wasi,
        state: Arc::clone(&state),
    }));

    let mh = engine
        .load_module(&core_bytes)
        .map_err(|e| anyhow::anyhow!("load_module: {e:?}"))?;
    let inst = engine
        .instantiate(mh)
        .map_err(|e| anyhow::anyhow!("instantiate: {e:?}"))?;

    // --- Proof 1: persistence. Two a-compute dispatches on the same instance. ---
    state.lock().unwrap().acc_in_feed = 5;
    engine
        .execute(inst, "a-compute", &[])
        .map_err(|e| anyhow::anyhow!("a-compute #1: {e:?}"))?;
    state.lock().unwrap().acc_in_feed = 7;
    engine
        .execute(inst, "a-compute", &[])
        .map_err(|e| anyhow::anyhow!("a-compute #2: {e:?}"))?;
    let acc = state.lock().unwrap().acc_out_recv;
    if acc != Some(12) {
        bail!(
            "state did NOT persist on kiln: fed 5 then 7, expected set-acc-out=12, got {acc:?} \
             (Some(7) = fresh-construct or call-local; None = never written)"
        );
    }

    // --- Proof 2: inter-thread. Producer -> thread_local buffer -> Consumer. ---
    state.lock().unwrap().src_in_feed = 42;
    engine
        .execute(inst, "pr-compute", &[])
        .map_err(|e| anyhow::anyhow!("pr-compute: {e:?}"))?;
    engine
        .execute(inst, "cn-compute", &[])
        .map_err(|e| anyhow::anyhow!("cn-compute: {e:?}"))?;
    let sink = state.lock().unwrap().sink_out_recv;
    if sink != Some(42) {
        bail!(
            "inter-thread did NOT deliver on kiln: fed src-in=42, expected set-sink-out=42, \
             got {sink:?} (None/0 = buffer never carried the value between dispatches)"
        );
    }

    println!(
        "KILN ORACLE PASSED: spar→meld→kiln — persistence acc(5,7)->12 across two dispatches, \
         and inter-thread pr.po->buffer->cn.ci->sink-out delivered 42 (on the kiln interpreter)"
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

/// Give the Accum component a persistent `acc` field and a compute that
/// accumulates its In port into it and writes the running total to its Out port.
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
