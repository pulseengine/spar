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
            // `--immediate-mode` and `-U` are NOT the same knob, and only the
            // second one was missing. `--immediate-mode` governs how quickly
            // the KERNEL hands packets to libpcap; `-U` governs whether
            // libpcap's dump writer flushes each packet to the FILE instead of
            // accumulating a stdio buffer.
            //
            // Without `-U`, run 30984170834 produced a 0-byte capture: the
            // teardown below calls Child::kill(), which is SIGKILL, and a
            // SIGKILLed process does not flush stdio. The buffer went with it.
            "--immediate-mode",
            "-U",
            "-c",
            "50",
        ],
    )?;
    eprintln!("gen-fixtures: tcpdump capturing ...");

    // 8. Start lldpd in GM and SW namespaces (-H 0 = immediate TX).
    //
    // Each instance gets its OWN control socket via `-u`. `ip netns exec`
    // unshares the *network* namespace and nothing else, so all three
    // instances share one mount namespace and would otherwise every one of
    // them bind the compiled-in default, LLDPD_CTL_SOCKET =
    // /var/run/lldpd.socket (lldpd.c:1764). One socket, several daemons,
    // and an lldpctl that reaches whichever won — a result that would look
    // like a successful capture of the wrong namespace.
    let lldp_socket = |ns: &str| format!("/run/lldpd-{ns}.socket");

    // Failure here is fatal. It used to warn and continue, which meant a
    // dead lldpd was discovered three seconds later as an empty neighbour
    // list — a plausible-looking LLDP result. If the daemon we are about to
    // interrogate did not start, there is nothing to capture.
    for (ns, dev, sysname) in [
        (&ns_gm.name, veth_gm, "spar-grandmaster"),
        (&ns_sw.name, veth_sw_l, "spar-switch"),
    ] {
        let sock = lldp_socket(ns);
        netns_exec(
            ns,
            "lldpd",
            &["-H", "0", "-I", dev, "-P", sysname, "-u", &sock],
        )?;
    }
    thread::sleep(Duration::from_secs(3));

    // 9. Collect LLDP JSON.
    // These used to be three stacked `unwrap_or_else` arms — capture failure,
    // JSON parse failure and validation failure — each substituting the SAME
    // empty neighbour list. So lldpd not running, lldpctl absent from the
    // image, a changed output shape, and a rejected document all produced a
    // byte-identical "successful" fixture, and the run went on to print
    // `wrote …/lldp.json` and exit 0.
    //
    // This program's entire output is a set of fixtures asserted to be REAL
    // captures from a real kernel. A fabricated one that is indistinguishable
    // from a captured one does not degrade the artifact, it invalidates it —
    // and silently, because an empty neighbour list is a perfectly plausible
    // thing for LLDP to report. Every one of these is now fatal.
    // The capture error propagates UNWRAPPED on purpose: netns_capture already
    // distinguishes ToolNotFound from CapabilityMissing, and re-wrapping would
    // collapse them back into one sentence — the exact regression the previous
    // commit split apart.
    // `-u` must match the socket the GM instance was started on, above.
    let gm_socket = lldp_socket(&ns_gm.name);
    let lldp_raw = netns_capture(&ns_gm.name, "lldpctl", &["-u", &gm_socket, "-f", "json"])?;
    let lldp_parsed: serde_json::Value = serde_json::from_str(&lldp_raw).map_err(|e| {
        FixtureError::Transform(format!(
            "lldpctl -f json emitted unparseable JSON: {e}; first 400 bytes: {:?}",
            lldp_raw.chars().take(400).collect::<String>()
        ))
    })?;
    let lldp_value = validate_lldp_json(&serde_json::to_string(&lldp_parsed)?)?;
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
    // Deliberately non-fatal: arping exits non-zero when it gets no reply,
    // and the capture is still usable without ARP frames. But it must not be
    // SILENT. `let _ =` here discarded the one signal that would have said
    // arping was missing from the image entirely — which it was, absent from
    // both environment.systemPackages and the unit `path`, so this line has
    // been a no-op that reported nothing. Non-fatal and unobserved are
    // different things; the surrounding lldpd/lldpctl/pmc calls already warn.
    netns_exec(
        &ns_gm.name,
        "arping",
        &["-c", "5", "-I", veth_gm, "169.254.1.2"],
    )
    .unwrap_or_else(|e| eprintln!("gen-fixtures: warning: arping: {e}"));
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
        // The stub this replaces was `"    masterOffset              0\n"`,
        // and it was load-bearing in the worst way. `masterOffset` is not a
        // linuxptp field — real `pmc` prints `master_offset` — so the parser,
        // written to the same invented spelling, could read the stub and NOT
        // real output. That inverted the two paths: with pmc working the run
        // failed, and with pmc absent it succeeded, writing a gPTP fixture
        // reporting sync_error_ns 0 on every sample. Perfect clock sync,
        // fabricated, on the success path.
        //
        // A capture that cannot be taken is not a measurement of zero error.
        // Propagated unwrapped, same reason as the lldpctl capture above.
        let round = netns_capture(&ns_gm.name, "pmc", &["-u", "-b", "0", "GET TIME_STATUS_NP"])?;
        pmc_rounds.push(round);
        thread::sleep(Duration::from_millis(500));
    }

    let pmc_refs: Vec<&str> = pmc_rounds.iter().map(String::as_str).collect();
    let gptp_value = pmc_to_gptp_json(veth_gm, Some(MAC_GM), 0, &pmc_refs)?;
    fs::write(&paths.gptp_json, serde_json::to_string_pretty(&gptp_value)?)?;
    eprintln!("gen-fixtures: wrote {}", paths.gptp_json.display());

    // 12. Stop background processes and flush PCAPNG.
    //
    // `-c 50` means tcpdump normally exits on its own, cleanly, once it has
    // its frames; try_wait tells us whether that happened so the log can say
    // which of the two it was. If it is still running we SIGKILL it, which is
    // survivable now only because `-U` above has already put every captured
    // packet on disk.
    match capture_child.try_wait() {
        Ok(Some(st)) => eprintln!("gen-fixtures: tcpdump exited on its own ({st})"),
        Ok(None) => {
            eprintln!("gen-fixtures: tcpdump still running (fewer than 50 frames); stopping it");
            let _ = capture_child.kill();
            let _ = capture_child.wait();
        }
        Err(e) => eprintln!("gen-fixtures: warning: cannot poll tcpdump: {e}"),
    }
    let _ = ptp4l_child.kill();
    let _ = ptp4l_child.wait();
    thread::sleep(Duration::from_millis(500));

    // This line used to be `eprintln!("wrote {}")` and nothing else — a claim
    // about a file the program had never looked at. tcpdump is the only
    // fixture producer that is a separate process, so it is the only one whose
    // output can be absent while gen-fixtures reports success, and in run
    // 30984170834 that is exactly what happened: "wrote /fixtures/
    // capture.pcapng" printed above a zero-byte file, and the harness three
    // steps later was the first thing to notice.
    //
    // A "wrote" line should be a measurement, not an assertion.
    let pcapng_len = fs::metadata(&paths.pcapng).map(|m| m.len()).map_err(|e| {
        FixtureError::Transform(format!(
            "tcpdump produced no file at {}: {e}",
            paths.pcapng.display()
        ))
    })?;
    if pcapng_len == 0 {
        return Err(FixtureError::Transform(format!(
            "tcpdump wrote a zero-byte capture at {}. An empty capture is not \
             a capture of no traffic — libpcap writes a file header before any \
             packet arrives, so zero bytes means the writer never flushed or \
             never started.",
            paths.pcapng.display()
        )));
    }
    eprintln!(
        "gen-fixtures: wrote {} ({pcapng_len} bytes)",
        paths.pcapng.display()
    );

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
