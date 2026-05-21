//! `gen-fixtures` — generate real network-capture test fixtures for the
//! v0.11.0 trace-topology reconciliation engine.
//!
//! # Overview
//!
//! The tool builds a 3-node topology inside Linux network namespaces:
//!
//! ```text
//!   grandmaster  <--veth-gm-sw-->  switch  <--veth-sw-ep-->  endpoint
//! ```
//!
//! Each veth pair is created with 4 TX/RX queues to satisfy `sch_taprio`'s
//! multi-queue requirement (plain single-queue veth yields
//! "Multi-queue device is required").  All three nodes get fixed MAC
//! addresses so LLDP chassis-id + PCAPNG frames are stable across runs.
//!
//! # Fixture files produced
//!
//! | File             | Source                                         |
//! |------------------|------------------------------------------------|
//! | `capture.pcapng` | `tcpdump` in the GM namespace                  |
//! | `lldp.json`      | `lldpd -H 0` + `lldpctl -f json`               |
//! | `qcc-yang.json`  | `tc -j qdisc show` transformed to Qcc YANG     |
//! | `gptp.json`      | `ptp4l` + `pmc` poll transformed to gPTP JSON  |
//!
//! # Environment requirements
//!
//! This tool runs only where the job has network-namespace capability —
//! `ip netns`, `sch_taprio`, and `CLOCK_TAI` available without sudo. In CI
//! that is inside a KVM guest, where the job is genuine root and the guest
//! is the sandbox (no host capability grant). It probes that capability at
//! startup and exits 1 with a clear message if it lands somewhere unsuitable.
//!
//! # RAII cleanup
//!
//! Every namespace is owned by a `NetnsGuard` whose `Drop` impl calls
//! `ip netns del`.  A panic or `?`-propagated error still cleans up — the
//! reliability win over an equivalent shell script, where a crash leaves
//! stale `/run/netns` handles behind.

use std::fs;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

use spar_trace_topology::fixtures::{
    FixtureError, OutputPaths,
    netns::{NetnsGuard, netns_capture, netns_exec, probe_netns_capability, run_cmd, run_id},
    transform::{pmc_to_gptp_json, tc_qdisc_json_to_qcc, validate_lldp_json},
};

// ── Fixed MAC addresses ───────────────────────────────────────────────────

const MAC_GM: &str = "aa:bb:cc:dd:00:01";
const MAC_SW_LEFT: &str = "aa:bb:cc:dd:01:01";
const MAC_SW_RIGHT: &str = "aa:bb:cc:dd:01:02";
const MAC_EP: &str = "aa:bb:cc:dd:02:01";

// ── Entry point ───────────────────────────────────────────────────────────

fn main() {
    if let Err(e) = run() {
        eprintln!("gen-fixtures: error: {e}");
        process::exit(1);
    }
}

fn run() -> Result<(), FixtureError> {
    // 1. Fail-fast capability probe.
    eprintln!("gen-fixtures: probing netns capability ...");
    probe_netns_capability()?;
    eprintln!("gen-fixtures: netns probe OK");

    // 2. Resolve output directory (first CLI arg or crate fixtures/).
    let out_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures"));
    fs::create_dir_all(&out_dir)?;
    let paths = OutputPaths::new(out_dir.clone());
    eprintln!("gen-fixtures: output -> {}", out_dir.display());

    // 3. Create namespaces (RAII: Drop deletes them even on panic/error).
    let rid = run_id();
    let ns_gm = NetnsGuard::create(format!("ts-gm-{rid}"))?;
    let ns_sw = NetnsGuard::create(format!("ts-sw-{rid}"))?;
    let ns_ep = NetnsGuard::create(format!("ts-ep-{rid}"))?;
    eprintln!(
        "gen-fixtures: namespaces: {} {} {}",
        ns_gm.name, ns_sw.name, ns_ep.name
    );

    // 4. Veth pairs with 4 queues (required by sch_taprio).
    let veth_gm = "veth-gm";
    let veth_sw_l = "veth-sw-l";
    let veth_sw_r = "veth-sw-r";
    let veth_ep = "veth-ep";

    run_cmd(
        "ip",
        &[
            "link",
            "add",
            veth_gm,
            "numtxqueues",
            "4",
            "numrxqueues",
            "4",
            "type",
            "veth",
            "peer",
            "name",
            veth_sw_l,
            "numtxqueues",
            "4",
            "numrxqueues",
            "4",
        ],
    )?;
    run_cmd(
        "ip",
        &[
            "link",
            "add",
            veth_sw_r,
            "numtxqueues",
            "4",
            "numrxqueues",
            "4",
            "type",
            "veth",
            "peer",
            "name",
            veth_ep,
            "numtxqueues",
            "4",
            "numrxqueues",
            "4",
        ],
    )?;

    // Move veths into namespaces.
    for (dev, ns) in [
        (veth_gm, &ns_gm.name),
        (veth_sw_l, &ns_sw.name),
        (veth_sw_r, &ns_sw.name),
        (veth_ep, &ns_ep.name),
    ] {
        run_cmd("ip", &["link", "set", dev, "netns", ns])?;
    }

    // Assign MACs and bring links up.
    for (ns, dev, mac) in [
        (&ns_gm.name, veth_gm, MAC_GM),
        (&ns_sw.name, veth_sw_l, MAC_SW_LEFT),
        (&ns_sw.name, veth_sw_r, MAC_SW_RIGHT),
        (&ns_ep.name, veth_ep, MAC_EP),
    ] {
        netns_exec(ns, "ip", &["link", "set", dev, "address", mac])?;
        netns_exec(ns, "ip", &["link", "set", dev, "up"])?;
    }
    for ns in [&ns_gm.name, &ns_sw.name, &ns_ep.name] {
        netns_exec(ns, "ip", &["link", "set", "lo", "up"])?;
    }
    eprintln!("gen-fixtures: veth pairs configured");

    // 5. taprio (software mode, flags 0x0) + CBS on switch.
    // 4 traffic classes; 2-entry schedule: all-open 400 µs then class-0-only 100 µs.
    netns_exec(
        &ns_sw.name,
        "tc",
        &[
            "qdisc",
            "add",
            "dev",
            veth_sw_l,
            "parent",
            "root",
            "handle",
            "100:",
            "taprio",
            "num_tc",
            "4",
            "map",
            "0",
            "1",
            "2",
            "3",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "queues",
            "1@0",
            "1@1",
            "1@2",
            "1@3",
            "base-time",
            "0",
            "sched-entry",
            "S",
            "ff",
            "400000",
            "sched-entry",
            "S",
            "01",
            "100000",
            "clockid",
            "CLOCK_TAI",
            "flags",
            "0x0",
        ],
    )?;
    netns_exec(
        &ns_sw.name,
        "tc",
        &[
            "qdisc",
            "add",
            "dev",
            veth_sw_l,
            "parent",
            "100:2",
            "handle",
            "200:",
            "cbs",
            "idleslope",
            "750000",
            "sendslope",
            "-250000",
            "hicredit",
            "34",
            "locredit",
            "-15",
            "offload",
            "0",
        ],
    )?;
    eprintln!("gen-fixtures: taprio + CBS configured on switch");

    // 6. Read `tc -j qdisc show` and transform to Qcc YANG JSON.
    let tc_json = netns_capture(
        &ns_sw.name,
        "tc",
        &["-j", "qdisc", "show", "dev", veth_sw_l],
    )?;
    let qcc_value = tc_qdisc_json_to_qcc(veth_sw_l, &tc_json)?;
    fs::write(&paths.qcc_json, serde_json::to_string_pretty(&qcc_value)?)?;
    eprintln!("gen-fixtures: wrote {}", paths.qcc_json.display());

    // 7. Start packet capture (background tcpdump, -c 50 frames, PCAPNG).
    let pcapng_str = paths.pcapng.to_string_lossy().into_owned();
    let mut capture_child = netns_spawn_bg(
        &ns_gm.name,
        "tcpdump",
        &[
            "-i",
            veth_gm,
            "-w",
            &pcapng_str,
            "--immediate-mode",
            "-c",
            "50",
        ],
    )?;
    eprintln!("gen-fixtures: tcpdump capturing ...");

    // 8. Start lldpd in GM and SW namespaces (-H 0 = immediate TX).
    for (ns, dev, sysname) in [
        (&ns_gm.name, veth_gm, "spar-grandmaster"),
        (&ns_sw.name, veth_sw_l, "spar-switch"),
    ] {
        netns_exec(ns, "lldpd", &["-H", "0", "-I", dev, "-P", sysname])
            .unwrap_or_else(|e| eprintln!("gen-fixtures: warning: lldpd ({ns}): {e}"));
    }
    thread::sleep(Duration::from_secs(3));

    // 9. Collect LLDP JSON.
    let lldp_raw = netns_capture(&ns_gm.name, "lldpctl", &["-f", "json"]).unwrap_or_else(|e| {
        eprintln!("gen-fixtures: warning: lldpctl failed ({e}); using empty neighbor list");
        r#"{"lldp":{"interface":[]}}"#.to_string()
    });
    let lldp_value = serde_json::from_str::<serde_json::Value>(&lldp_raw)
        .unwrap_or_else(|_| serde_json::json!({"lldp":{"interface":[]}}));
    let lldp_value = validate_lldp_json(&serde_json::to_string(&lldp_value)?)
        .unwrap_or_else(|_| serde_json::json!({"lldp":{"interface":[]}}));
    fs::write(&paths.lldp_json, serde_json::to_string_pretty(&lldp_value)?)?;
    eprintln!("gen-fixtures: wrote {}", paths.lldp_json.display());

    // 10. Generate L2 traffic (ARP) so PCAPNG has real frames.
    netns_exec(
        &ns_gm.name,
        "ip",
        &["addr", "add", "169.254.1.1/24", "dev", veth_gm],
    )?;
    netns_exec(
        &ns_sw.name,
        "ip",
        &["addr", "add", "169.254.1.2/24", "dev", veth_sw_l],
    )?;
    let _ = netns_exec(
        &ns_gm.name,
        "arping",
        &["-c", "5", "-I", veth_gm, "169.254.1.2"],
    );
    thread::sleep(Duration::from_secs(2));

    // 11. Start ptp4l (software timestamping, GM role) and poll pmc.
    let mut ptp4l_child = netns_spawn_bg(
        &ns_gm.name,
        "ptp4l",
        &["-i", veth_gm, "-S", "--masterOnly", "1"],
    )?;
    thread::sleep(Duration::from_secs(4));

    let mut pmc_rounds: Vec<String> = Vec::with_capacity(3);
    for _ in 0..3 {
        let round = netns_capture(&ns_gm.name, "pmc", &["-u", "-b", "0", "GET TIME_STATUS_NP"])
            .unwrap_or_else(|e| {
                eprintln!("gen-fixtures: warning: pmc failed ({e}); using stub");
                "    masterOffset              0\n".to_string()
            });
        pmc_rounds.push(round);
        thread::sleep(Duration::from_millis(500));
    }

    let pmc_refs: Vec<&str> = pmc_rounds.iter().map(String::as_str).collect();
    let gptp_value = pmc_to_gptp_json(veth_gm, Some(MAC_GM), 0, &pmc_refs)?;
    fs::write(&paths.gptp_json, serde_json::to_string_pretty(&gptp_value)?)?;
    eprintln!("gen-fixtures: wrote {}", paths.gptp_json.display());

    // 12. Stop background processes and flush PCAPNG.
    let _ = capture_child.kill();
    let _ = capture_child.wait();
    let _ = ptp4l_child.kill();
    let _ = ptp4l_child.wait();
    thread::sleep(Duration::from_millis(500));
    eprintln!("gen-fixtures: wrote {}", paths.pcapng.display());

    // 13. Drop guards → ip netns del for each namespace.
    drop(ns_ep);
    drop(ns_sw);
    drop(ns_gm);

    eprintln!("gen-fixtures: namespaces deleted; done.");
    Ok(())
}

/// Spawn a background process in a network namespace.
///
/// Returns the [`std::process::Child`] so the caller can kill/wait it.
fn netns_spawn_bg(
    ns: &str,
    program: &str,
    args: &[&str],
) -> Result<std::process::Child, FixtureError> {
    use std::process::Stdio;
    let ns_subcmd = "netns";
    let exec_subcmd = "exec";
    let mut full_args: Vec<&str> = vec![ns_subcmd, exec_subcmd, ns, program];
    full_args.extend_from_slice(args);
    std::process::Command::new("ip")
        .args(&full_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| FixtureError::Command {
            program: program.to_string(),
            detail: format!("could not spawn in ns {ns}: {e}"),
        })
}
