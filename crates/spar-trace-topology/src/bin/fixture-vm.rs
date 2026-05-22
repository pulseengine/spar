//! `fixture-vm` — QEMU harness that boots the NixOS fixture-generation
//! guest and retrieves the four trace-topology fixture files.
//!
//! # Usage
//!
//! ```text
//! fixture-vm --base-qcow2 <path/to/base.qcow2> --output-dir <dir>
//! fixture-vm --verify <dir>
//! ```
//!
//! ## Normal mode (`--base-qcow2` + `--output-dir`)
//!
//! 1. Creates a copy-on-write QEMU overlay over `<base.qcow2>` so each
//!    run starts from a clean image (the base is never mutated).
//! 2. Boots `qemu-system-x86_64` with `-enable-kvm -nographic`, the
//!    overlay disk, and a virtio-9p share whose host path is `<output-dir>`.
//!    The guest mounts the share under `/fixtures` and the `gen-fixtures`
//!    service writes the four fixture files there.
//! 3. Waits for the QEMU process to exit (the guest powers off when
//!    `gen-fixtures` completes). A 15-minute wall-clock timeout kills
//!    QEMU and returns an error if the guest does not shut down in time.
//! 4. After exit: asserts that all four fixture files exist in
//!    `<output-dir>` and are non-empty.
//!
//! ## Verify mode (`--verify`)
//!
//! Round-trips each fixture file through the corresponding
//! `spar-trace-topology` ingest parser and asserts `Ok`.  Exits 0 if all
//! four parse cleanly, non-zero with a descriptive message otherwise.
//!
//! # RAII guarantees
//!
//! The QEMU child process and the overlay temp-file are both wrapped in
//! RAII guards whose `Drop` implementations clean up unconditionally, so
//! a panic or early `?` return never leaves a stale process or tmp file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::time::{Duration, Instant};

// ── Public re-export so the verify helper can use ingest parsers ──────────

use spar_trace_topology::{
    FrameSource, GptpJsonPtpTimeSource, LldpJsonTopologySource, PcapngFrameSource, PtpTimeSource,
    QccYangSwitchConfigSource, SwitchConfigSource, TopologySource,
};

// ── Error type ────────────────────────────────────────────────────────────

#[derive(Debug)]
enum HarnessError {
    Io(io::Error),
    Ingest(spar_trace_topology::IngestError),
    Other(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Ingest(e) => write!(f, "ingest parse error: {e}"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl From<io::Error> for HarnessError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<spar_trace_topology::IngestError> for HarnessError {
    fn from(e: spar_trace_topology::IngestError) -> Self {
        Self::Ingest(e)
    }
}

// ── CLI config ────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Mode {
    /// Boot the NixOS guest and collect fixtures.
    Run {
        base_qcow2: PathBuf,
        output_dir: PathBuf,
    },
    /// Parse already-present fixtures and assert Ok.
    Verify { fixture_dir: PathBuf },
}

fn parse_args() -> Result<Mode, HarnessError> {
    let args: Vec<String> = std::env::args().collect();
    let mut iter = args.iter().skip(1).peekable();

    match iter.next().map(String::as_str) {
        Some("--verify") => {
            let dir = iter.next().ok_or_else(|| {
                HarnessError::Other("--verify requires a directory argument".to_string())
            })?;
            Ok(Mode::Verify {
                fixture_dir: PathBuf::from(dir),
            })
        }
        Some("--base-qcow2") => {
            let base = iter.next().ok_or_else(|| {
                HarnessError::Other("--base-qcow2 requires a path argument".to_string())
            })?;
            let output_dir = match (iter.next().map(String::as_str), iter.next()) {
                (Some("--output-dir"), Some(d)) => PathBuf::from(d),
                _ => {
                    return Err(HarnessError::Other(
                        "--output-dir <path> is required after --base-qcow2".to_string(),
                    ))
                }
            };
            Ok(Mode::Run {
                base_qcow2: PathBuf::from(base),
                output_dir,
            })
        }
        _ => Err(HarnessError::Other(
            "Usage:\n  fixture-vm --base-qcow2 <path> --output-dir <dir>\n  fixture-vm --verify <dir>".to_string(),
        )),
    }
}

// ── RAII overlay file ─────────────────────────────────────────────────────

/// A temporary QEMU qcow2 overlay file that is deleted on `Drop`.
///
/// Created with `qemu-img create -f qcow2 -b <base> -F qcow2 <overlay>`.
/// Each invocation of `fixture-vm` creates a fresh overlay, so the base
/// image is never mutated and multiple concurrent runs don't collide.
struct OverlayFile {
    path: PathBuf,
}

impl OverlayFile {
    fn create(base: &Path) -> Result<Self, HarnessError> {
        let overlay_path = {
            let dir = base.parent().unwrap_or_else(|| Path::new("."));
            dir.join(format!(
                "fixture-vm-overlay-{}.qcow2",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
            ))
        };

        let status = Command::new("qemu-img")
            .args([
                "create",
                "-f",
                "qcow2",
                "-b",
                &base.to_string_lossy(),
                "-F",
                "qcow2",
                &overlay_path.to_string_lossy(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()?;

        if !status.success() {
            return Err(HarnessError::Other(format!(
                "qemu-img create failed (exit {})",
                status.code().unwrap_or(-1)
            )));
        }

        Ok(Self { path: overlay_path })
    }
}

impl Drop for OverlayFile {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

// ── RAII QEMU child ───────────────────────────────────────────────────────

/// A running QEMU process that is killed + waited on `Drop` if it has
/// not already exited.
struct QemuChild {
    inner: process::Child,
}

impl QemuChild {
    fn spawn(overlay: &OverlayFile, fixture_dir: &Path) -> Result<Self, HarnessError> {
        let child = Command::new("qemu-system-x86_64")
            .args([
                // KVM acceleration (requires /dev/kvm + kvm group).
                "-enable-kvm",
                // No graphical window — all output on stdio.
                "-nographic",
                // 2 vCPUs, 1 GiB RAM — enough for lldpd/ptp4l/tcpdump.
                "-smp",
                "2",
                "-m",
                "1G",
                // Boot disk: CoW overlay over the NixOS base qcow2.
                "-drive",
                &format!(
                    "file={},format=qcow2,if=virtio",
                    overlay.path.to_string_lossy()
                ),
                // virtio-9p share: the host-side fixture directory is
                // exported with mount_tag="fixtures". The guest's NixOS
                // systemd unit mounts this and writes the four fixture
                // files there.
                "-virtfs",
                &format!(
                    "local,path={},mount_tag=fixtures,security_model=none",
                    fixture_dir.to_string_lossy()
                ),
                // Serial console on stdio; the guest's kernel console
                // output appears here for debugging.
                "-serial",
                "mon:stdio",
            ])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        Ok(Self { inner: child })
    }

    /// Wait for the QEMU process to exit, with a wall-clock timeout.
    ///
    /// Returns `Ok(exit_status)` when the guest exits within the
    /// deadline, `Err` with a clear message if the timeout fires (QEMU
    /// is killed before returning).
    fn wait_with_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<process::ExitStatus, HarnessError> {
        let start = Instant::now();
        loop {
            match self.inner.try_wait()? {
                Some(status) => return Ok(status),
                None => {
                    if start.elapsed() >= timeout {
                        let _ = self.inner.kill();
                        let _ = self.inner.wait();
                        return Err(HarnessError::Other(format!(
                            "QEMU guest did not shut down within {} seconds; killed",
                            timeout.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }
}

impl Drop for QemuChild {
    fn drop(&mut self) {
        // Best-effort kill — ignore errors (process may already be gone).
        let _ = self.inner.kill();
        let _ = self.inner.wait();
    }
}

// ── Expected fixture file names ───────────────────────────────────────────

const FIXTURE_FILES: &[&str] = &["capture.pcapng", "lldp.json", "qcc-yang.json", "gptp.json"];

// ── Run mode ──────────────────────────────────────────────────────────────

fn run_vm(base_qcow2: &Path, output_dir: &Path) -> Result<(), HarnessError> {
    // Resolve paths.
    if !base_qcow2.exists() {
        return Err(HarnessError::Other(format!(
            "base qcow2 not found: {}",
            base_qcow2.display()
        )));
    }
    fs::create_dir_all(output_dir)?;

    eprintln!("fixture-vm: base image  = {}", base_qcow2.display());
    eprintln!("fixture-vm: output dir  = {}", output_dir.display());

    // 1. Create a CoW overlay so the base is never written.
    let overlay = OverlayFile::create(base_qcow2)?;
    eprintln!("fixture-vm: overlay      = {}", overlay.path.display());

    // 2. Boot the guest.
    eprintln!("fixture-vm: booting QEMU guest …");
    let mut qemu = QemuChild::spawn(&overlay, output_dir)?;

    // 3. Wait up to 15 minutes for the guest to run gen-fixtures and
    //    power off.
    let timeout = Duration::from_secs(15 * 60);
    let status = qemu.wait_with_timeout(timeout)?;
    eprintln!("fixture-vm: QEMU exited with {status}");

    if !status.success() {
        return Err(HarnessError::Other(format!(
            "QEMU process exited non-zero: {status}"
        )));
    }

    // 4. Assert all four fixture files exist and are non-empty.
    check_fixtures(output_dir)?;

    eprintln!("fixture-vm: all fixture files present and non-empty — done.");
    Ok(())
}

// ── Verify mode ───────────────────────────────────────────────────────────

fn verify_fixtures(fixture_dir: &Path) -> Result<(), HarnessError> {
    eprintln!(
        "fixture-vm --verify: checking fixtures in {}",
        fixture_dir.display()
    );

    // First check raw presence / non-empty.
    check_fixtures(fixture_dir)?;

    let pcapng_path = fixture_dir.join("capture.pcapng");
    let lldp_path = fixture_dir.join("lldp.json");
    let qcc_path = fixture_dir.join("qcc-yang.json");
    let gptp_path = fixture_dir.join("gptp.json");

    // Parse capture.pcapng through PcapngFrameSource.
    let mut pcapng_src = PcapngFrameSource::open(&pcapng_path)?;
    let mut frame_count = 0usize;
    for result in pcapng_src.frames() {
        result?;
        frame_count += 1;
    }
    eprintln!("fixture-vm --verify: capture.pcapng OK ({frame_count} frames)");

    // Parse lldp.json through LldpJsonTopologySource.
    let lldp_src = LldpJsonTopologySource::open(&lldp_path)?;
    let neighbor_count = lldp_src.neighbors().len();
    eprintln!("fixture-vm --verify: lldp.json OK ({neighbor_count} neighbors)");

    // Parse qcc-yang.json through QccYangSwitchConfigSource.
    let qcc_src = QccYangSwitchConfigSource::open(&qcc_path)?;
    let port_count = qcc_src.ports().len();
    eprintln!("fixture-vm --verify: qcc-yang.json OK ({port_count} ports)");

    // Parse gptp.json through GptpJsonPtpTimeSource.
    let gptp_src = GptpJsonPtpTimeSource::open(&gptp_path)?;
    let ptp_port_count = gptp_src.ports().len();
    eprintln!("fixture-vm --verify: gptp.json OK ({ptp_port_count} ptp ports)");

    eprintln!("fixture-vm --verify: all four fixtures parse cleanly.");
    Ok(())
}

// ── Shared fixture-presence check ─────────────────────────────────────────

fn check_fixtures(dir: &Path) -> Result<(), HarnessError> {
    for name in FIXTURE_FILES {
        let path = dir.join(name);
        if !path.exists() {
            return Err(HarnessError::Other(format!(
                "fixture file missing: {}",
                path.display()
            )));
        }
        let metadata = fs::metadata(&path)?;
        if metadata.len() == 0 {
            return Err(HarnessError::Other(format!(
                "fixture file is empty: {}",
                path.display()
            )));
        }
        eprintln!(
            "fixture-vm: {} ({} bytes) ✓",
            path.display(),
            metadata.len()
        );
    }
    Ok(())
}

// ── Entry point ───────────────────────────────────────────────────────────

fn main() {
    let result = match parse_args() {
        Ok(Mode::Run {
            base_qcow2,
            output_dir,
        }) => run_vm(&base_qcow2, &output_dir),
        Ok(Mode::Verify { fixture_dir }) => verify_fixtures(&fixture_dir),
        Err(e) => Err(e),
    };

    if let Err(e) = result {
        eprintln!("fixture-vm: error: {e}");
        process::exit(1);
    }
}
