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
//! | `capture.pcapng` | `dumpcap` in the GM namespace (see note below)  |
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
    netns::{
        NetnsGuard, capture_stdout, netns_capture, netns_exec, probe_netns_capability, run_cmd,
        run_id,
    },
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

    // 7. Start packet capture (background dumpcap, -c 50 frames, pcapng).
    // dumpcap, not tcpdump. The fixture is REQUIRED to be pcapng
    // (REQ-TRACE-INGEST-PCAPNG: "given a `.pcapng` file recorded …", and the
    // ingest is built on PcapNGReader), and upstream tcpdump cannot write
    // pcapng at all — `-w` goes through libpcap's pcap_dump, which emits the
    // classic format. tcpdump's own source calls the pcapng flag a macOS
    // extension (tcpdump.c:608). Run 30987112464 therefore produced a valid
    // 4376-byte classic pcap named `capture.pcapng`, and the ingest rejected
    // it with `HeaderNotRecognized`. The extension was the only thing
    // asserting the format.
    //
    // `-n` selects pcapng explicitly. It is already dumpcap's default
    // ("Save as pcapng by default", ui/capture_opts.c:103) but the default is
    // exactly the kind of unstated assumption that produced this bug, so it is
    // stated. `-P` would select classic pcap; passing neither and trusting the
    // default is how the next person inherits this.
    // dumpcap captures to guest-local tmpfs, NOT straight to /fixtures, and
    // gen-fixtures copies the result across afterwards.
    //
    // /fixtures is a 9p share of a host directory (flake.nix: trans=virtio,
    // version=9p2000.L), so it carries the HOST runner's uid and mode. The
    // guest's root can write there only through CAP_DAC_OVERRIDE — and
    // dumpcap drops every capability it holds, deliberately, the moment the
    // capture device is open and before the output file is created:
    //
    //     /* If not using libcap: we now can now set euid/egid to ruid/rgid */
    //     #ifndef HAVE_LIBCAP
    //         relinquish_special_privs_perm();
    //     #else
    //         relinquish_all_capabilities();
    //     #endif
    //                                            — dumpcap.c:3355-3361
    //
    // So run 30991119966 got "Capturing on 'veth-gm'" and then "could not be
    // opened: Permission denied" for the savefile. That is dumpcap doing its
    // job — it is designed to hold privileges for as little as possible — and
    // tcpdump only appeared to work here because it never drops capabilities.
    //
    // Capturing to /tmp sidesteps the whole question rather than tuning 9p
    // uid/mode options that would have to stay aligned with whatever uid the
    // runner happens to use.
    let pcapng_local = "/tmp/capture.pcapng";
    let mut capture_child = netns_spawn_bg(
        &ns_gm.name,
        "dumpcap",
        // `-F pcapng`, not `-n`: dumpcap 4.6 accepts `-n` but answers
        // "'-n' is deprecated; use '-F pcapng' to set the output format".
        // Still explicit rather than relying on the default, for the reason
        // above.
        &[
            "-i",
            veth_gm,
            "-w",
            pcapng_local,
            "-F",
            "pcapng",
            "-c",
            "50",
        ],
    )?;
    eprintln!("gen-fixtures: dumpcap capturing (pcapng) to {pcapng_local} ...");

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
    // `-c 50` means dumpcap normally exits on its own, cleanly, once it has
    // its frames; try_wait tells us whether that happened so the log can say
    // which of the two it was. If it is still running we SIGKILL it, which is
    // survivable now only because `-U` above has already put every captured
    // packet on disk.
    match capture_child.try_wait() {
        Ok(Some(st)) => eprintln!("gen-fixtures: dumpcap exited on its own ({st})"),
        Ok(None) => {
            eprintln!("gen-fixtures: dumpcap still running (fewer than 50 frames); stopping it");
            let _ = capture_child.kill();
            let _ = capture_child.wait();
        }
        Err(e) => eprintln!("gen-fixtures: warning: cannot poll dumpcap: {e}"),
    }
    let _ = ptp4l_child.kill();
    let _ = ptp4l_child.wait();
    thread::sleep(Duration::from_millis(500));

    // This line used to be `eprintln!("wrote {}")` and nothing else — a claim
    // about a file the program had never looked at. The capture is the only
    // fixture producer that is a separate process, so it is the only one whose
    // output can be absent while gen-fixtures reports success, and in run
    // 30984170834 that is exactly what happened: "wrote /fixtures/
    // capture.pcapng" printed above a zero-byte file, and the harness three
    // steps later was the first thing to notice.
    //
    // A "wrote" line should be a measurement, not an assertion.
    let pcapng_bytes = fs::read(pcapng_local).map_err(|e| {
        FixtureError::Transform(format!(
            "dumpcap produced no readable file at {pcapng_local}: {e}"
        ))
    })?;
    if pcapng_bytes.is_empty() {
        return Err(FixtureError::Transform(format!(
            "dumpcap wrote a zero-byte capture at {pcapng_local}. An empty \
             capture is not a capture of no traffic — a Section Header Block is \
             written before any packet arrives, so zero bytes means the writer \
             never flushed or never started."
        )));
    }

    // Size was never the property we needed. Run 30987112464 wrote a perfectly
    // good 4376-byte file and still failed, because the bytes were classic
    // pcap and only the FILE EXTENSION claimed otherwise. Check the format we
    // actually depend on.
    //
    // A pcapng stream opens with a Section Header Block, whose type field is
    // 0x0A0D0D0A (wireshark wiretap/pcapng_module.h:26). It is byte-order
    // independent by construction — that is the point of the value — so a
    // single comparison covers both endiannesses. Classic pcap opens with
    // 0xA1B2C3D4 (or its byte-swapped/nanosecond variants), which is what we
    // were producing.
    const PCAPNG_SHB: [u8; 4] = [0x0A, 0x0D, 0x0D, 0x0A];
    if pcapng_bytes.len() < 4 || pcapng_bytes[..4] != PCAPNG_SHB {
        let head: Vec<String> = pcapng_bytes
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        return Err(FixtureError::Transform(format!(
            "{} is not pcapng: expected a Section Header Block (0a0d0d0a), got [{}]. \
             0xa1b2c3d4 (any byte order) means classic pcap — the capture tool \
             wrote the wrong format and the `.pcapng` suffix is not evidence of \
             anything.",
            pcapng_local,
            head.join(" ")
        )));
    }
    // Cross-check with an INDEPENDENT reader before declaring the capture good.
    //
    // The SHB check above proves the first four bytes; it does not prove the
    // file is a coherent pcapng that a real consumer can walk. tshark is a
    // different implementation by different authors, so its agreement is
    // evidence in a way that spar's parser agreeing with spar's writer is not.
    //
    // This ran on the host runner until now, where tshark was not installed and
    // its exit code was being swallowed by a pipe into `tee`. It runs here
    // because here is where the binary is.
    let tshark_out = capture_stdout("tshark", &["-r", pcapng_local])?;
    let frames = tshark_out.lines().filter(|l| !l.trim().is_empty()).count();
    if frames == 0 {
        return Err(FixtureError::Transform(format!(
            "tshark read {pcapng_local} without error but found zero frames. \
             tshark exits 0 on a valid capture containing no packets, so the \
             frame count has to be checked separately from the exit status."
        )));
    }

    // Now publish it to the 9p share. gen-fixtures still holds every capability
    // it started with, so unlike dumpcap it can write here — the same way the
    // other three fixtures get written.
    fs::write(&paths.pcapng, &pcapng_bytes).map_err(|e| {
        FixtureError::Transform(format!(
            "captured {} bytes to {pcapng_local} but could not publish them to {}: {e}",
            pcapng_bytes.len(),
            paths.pcapng.display()
        ))
    })?;

    // Confirm the published copy, not the source. A copy is another operation
    // that can silently do nothing, and `paths.pcapng` — not the tmpfs file —
    // is what the harness and the ingest will read.
    let published = fs::metadata(&paths.pcapng).map(|m| m.len()).map_err(|e| {
        FixtureError::Transform(format!(
            "published {} but cannot stat it: {e}",
            paths.pcapng.display()
        ))
    })?;
    if published != pcapng_bytes.len() as u64 {
        return Err(FixtureError::Transform(format!(
            "published {} is {published} bytes but the capture was {} — the copy \
             to the 9p share was truncated.",
            paths.pcapng.display(),
            pcapng_bytes.len()
        )));
    }

    eprintln!(
        "gen-fixtures: wrote {} ({published} bytes, pcapng SHB verified, {frames} frames per tshark)",
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
        // Both streams used to be Stdio::null(). Not "piped and never read" —
        // routed to /dev/null at the source, so every background tool this
        // program starts was mute by construction, and there was nothing
        // anywhere to read even in principle.
        //
        // Run 30988563439 is the bill for that: dumpcap exited 1 without
        // producing a file, and the only thing the log could say was "exit
        // status: 1". dumpcap explains itself on stderr in a sentence; that
        // sentence was discarded, so diagnosing it needs another VM dispatch
        // per hypothesis.
        //
        // These are inherited rather than piped because gen-fixtures runs as a
        // systemd unit whose console is on ttyS0 and captured into the CI log
        // (that serial console exists precisely so guest failures are legible).
        // Piping would mean draining two streams from a process we deliberately
        // leave running in the background — inheriting gets the same text to
        // the same place with no plumbing and no risk of a full pipe buffer
        // blocking the child.
        //
        // The capture data is unaffected: dumpcap and tcpdump write the capture
        // via `-w`, never to stdout.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| FixtureError::Command {
            program: program.to_string(),
            detail: format!("could not spawn in ns {ns}: {e}"),
        })
}
