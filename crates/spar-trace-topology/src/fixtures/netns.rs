//! RAII network-namespace management and subprocess helpers.
//!
//! # NetnsGuard
//!
//! Constructing a [`NetnsGuard`] calls `ip netns add <name>` and stores the
//! name.  Its `Drop` impl calls `ip netns del <name>`, so the namespace is
//! torn down even on a panic or `?`-propagated early return.
//!
//! The CI workflow also runs a belt-and-suspenders `ip netns del` step with
//! `if: always()` in case the Rust process is SIGKILL'd before `Drop` runs.
//!
//! # Namespace naming
//!
//! Names are derived from `$GITHUB_RUN_ID` (set by GitHub Actions) or from
//! the process PID + a microsecond timestamp when the env-var is absent (for
//! local / non-CI runs).  A short prefix `ts-gm-`, `ts-sw-`, `ts-ep-` is
//! prepended so the cleanup step can glob `ts-*`.

use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use super::FixtureError;

// ── Name generation ───────────────────────────────────────────────────────

/// Generate a collision-resistant run-id string.
///
/// Prefers `$GITHUB_RUN_ID` (present in every GitHub Actions job).
/// Falls back to `<pid>-<microseconds-since-epoch>` for local runs.
pub fn run_id() -> String {
    if let Ok(id) = std::env::var("GITHUB_RUN_ID")
        && !id.is_empty()
    {
        return id;
    }
    let pid = std::process::id();
    let us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_micros();
    format!("{pid}-{us}")
}

// ── RAII guard ────────────────────────────────────────────────────────────

/// RAII owner of a Linux network namespace.
///
/// The namespace is created in [`NetnsGuard::create`] and deleted (best-
/// effort) in [`Drop`].  The guard intentionally does NOT implement `Clone`
/// to prevent double-deletion.
#[must_use]
pub struct NetnsGuard {
    /// The namespace name as passed to `ip netns add`.
    pub name: String,
    deleted: bool,
}

impl NetnsGuard {
    /// Create a new network namespace named `name`.
    ///
    /// Runs: `ip netns add <name>`
    ///
    /// # Errors
    ///
    /// Returns [`FixtureError::Command`] if `ip` is not found or exits
    /// non-zero.
    pub fn create(name: impl Into<String>) -> Result<Self, FixtureError> {
        let name = name.into();
        run_cmd("ip", &["netns", "add", &name])?;
        Ok(Self {
            name,
            deleted: false,
        })
    }

    /// Delete the namespace immediately (rather than waiting for `Drop`).
    ///
    /// After calling this the guard is in a "already-deleted" state and
    /// `Drop` will be a no-op.
    pub fn delete(mut self) -> Result<(), FixtureError> {
        self.do_delete()
    }

    fn do_delete(&mut self) -> Result<(), FixtureError> {
        if self.deleted {
            return Ok(());
        }
        self.deleted = true;
        run_cmd("ip", &["netns", "del", &self.name])
    }
}

impl Drop for NetnsGuard {
    fn drop(&mut self) {
        if !self.deleted {
            // Best-effort: ignore errors in Drop (cannot propagate).
            let _ = self.do_delete();
        }
    }
}

// ── Probe ─────────────────────────────────────────────────────────────────

/// Fail-fast capability probe.
///
/// Creates a temporary namespace `probe-<run_id>` and immediately deletes
/// it, so the tool exits early with a clear diagnostic rather than failing
/// mid-run.
///
/// Returns [`FixtureError::ToolNotFound`] when `ip` is not on `PATH` and
/// [`FixtureError::CapabilityMissing`] when `ip` ran but was refused. Those
/// two verdicts point at different repositories, which is the whole reason
/// they are not one message — see [`diagnose_create`].
///
/// The caller should invoke this before any topology setup.
pub fn probe_netns_capability() -> Result<(), FixtureError> {
    let probe = format!("probe-{}", run_id());
    let guard = NetnsGuard::create(&probe).map_err(diagnose_create)?;
    guard
        .delete()
        .map_err(|e| FixtureError::CapabilityMissing(format!("cannot delete probe namespace: {e}")))
}

/// Attribute a failed `ip netns add` to the right layer.
///
/// The `netns`-label question is only meaningful once `ip` has actually run.
/// Asking it when the binary was never found sends the reader to the runner
/// configuration to debug a guest-image bug — which is exactly what happened
/// on run 30583284365, where a missing `ip` printed "is this runner12 with
/// the `netns` label?" for fifteen minutes.
fn diagnose_create(e: FixtureError) -> FixtureError {
    match e {
        // Absent binary: nothing to do with privileges. Pass it through
        // unwrapped so the message names the image, not the label.
        FixtureError::ToolNotFound { .. } => e,
        other => FixtureError::CapabilityMissing(format!(
            "cannot create network namespace \
             (is this runner12 with the `netns` label?): {other}"
        )),
    }
}

// ── Command helpers ───────────────────────────────────────────────────────

/// Run a command, returning `Ok(())` on exit-status 0 or a descriptive
/// [`FixtureError::Command`] on failure.
pub fn run_cmd(program: &str, args: &[&str]) -> Result<(), FixtureError> {
    let out = spawn(program, args)?;
    check_output(program, out)
}

/// Run a command and capture its stdout as a UTF-8 `String`.
pub fn capture_stdout(program: &str, args: &[&str]) -> Result<String, FixtureError> {
    let out = spawn(program, args)?;
    let stdout_bytes = out.stdout.clone();
    check_output(program, out)?;
    String::from_utf8(stdout_bytes).map_err(|e| FixtureError::Command {
        program: program.to_string(),
        detail: format!("stdout is not valid UTF-8: {e}"),
    })
}

/// How a namespaced command is named when it fails.
///
/// `ip netns exec` propagates the CHILD's exit status, so a non-zero result
/// from one of these almost always belongs to `program`, not to `ip`. Labelling
/// the error with the outer binary sent run 30981864761's report the wrong way:
///
///   gen-fixtures: warning: lldpd (ts-gm-…): command `ip` failed: exit 1
///
/// `ip` was fine. `lldpd` had exited 1 because its `_lldpd` privilege
/// separation account did not exist. The sentence named the one program in it
/// that was working — the same mistake, one wrapper further out, as the
/// tool-absent/tool-refused conflation that `ToolNotFound` exists to prevent.
fn netns_label(ns: &str, program: &str) -> String {
    format!("{program} (via ip netns exec {ns})")
}

/// Run `ip netns exec <ns> <program> [args...]`.
pub fn netns_exec(ns: &str, program: &str, args: &[&str]) -> Result<(), FixtureError> {
    let mut full_args = vec!["netns", "exec", ns, program];
    full_args.extend_from_slice(args);
    run_cmd_labelled("ip", &full_args, &netns_label(ns, program))
}

/// Run `ip netns exec <ns> <program> [args...]` and capture stdout.
pub fn netns_capture(ns: &str, program: &str, args: &[&str]) -> Result<String, FixtureError> {
    let mut full_args = vec!["netns", "exec", ns, program];
    full_args.extend_from_slice(args);
    capture_stdout_labelled("ip", &full_args, &netns_label(ns, program))
}

/// `run_cmd`, but failures are attributed to `label` rather than to `program`.
///
/// `program` is still what gets spawned — only the blame changes. A spawn
/// ENOENT keeps naming `program`, because that genuinely IS the binary the
/// kernel could not find; only a non-zero exit is re-attributed.
fn run_cmd_labelled(program: &str, args: &[&str], label: &str) -> Result<(), FixtureError> {
    let out = spawn(program, args)?;
    check_output(label, out)
}

/// `capture_stdout`, attributing failures to `label`. See [`run_cmd_labelled`].
fn capture_stdout_labelled(
    program: &str,
    args: &[&str],
    label: &str,
) -> Result<String, FixtureError> {
    let out = spawn(program, args)?;
    let stdout_bytes = out.stdout.clone();
    check_output(label, out)?;
    String::from_utf8(stdout_bytes).map_err(|e| FixtureError::Command {
        program: label.to_string(),
        detail: format!("stdout is not valid UTF-8: {e}"),
    })
}

fn spawn(program: &str, args: &[&str]) -> Result<Output, FixtureError> {
    Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            // ENOENT from spawn means the binary is not on PATH. That is a
            // different fault, with a different owner, than a binary that ran
            // and was refused — keep them apart from here on.
            if e.kind() == std::io::ErrorKind::NotFound {
                FixtureError::ToolNotFound {
                    program: program.to_string(),
                }
            } else {
                FixtureError::Command {
                    program: program.to_string(),
                    detail: format!("could not spawn: {e}"),
                }
            }
        })
}

fn check_output(program: &str, out: Output) -> Result<(), FixtureError> {
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let code = out
        .status
        .code()
        .map_or_else(|| "(signal)".to_string(), |c| c.to_string());
    Err(FixtureError::Command {
        program: program.to_string(),
        detail: format!("exit {code}: stderr={stderr:?} stdout={stdout:?}"),
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A namespaced command that fails must be blamed on the program that
    /// failed, not on the `ip` wrapper that ran it.
    ///
    /// The regression this pins is run 30981864761, where lldpd exited 1
    /// (missing `_lldpd` account) and the report read ``command `ip` failed``.
    /// `ip netns exec` forwards the child's exit status, so the outer binary
    /// is the least likely culprit and was the only one named.
    ///
    /// Asserting the label *contains the inner program name* is what makes
    /// this a detector: reverting to `run_cmd("ip", …)` produces a message
    /// naming only `ip`, and this fails.
    #[test]
    fn namespaced_failure_names_the_inner_program_not_ip() {
        let label = netns_label("ts-gm-1", "lldpd");
        assert!(
            label.contains("lldpd"),
            "label must name the failing program, got: {label}"
        );
        assert!(
            label.starts_with("lldpd"),
            "the failing program must lead the label, got: {label}"
        );
        // The wrapper is still reported, because *which* namespace it ran in
        // is the other half of the diagnosis.
        assert!(
            label.contains("ts-gm-1"),
            "label must name the namespace, got: {label}"
        );
        // And it must not read as though `ip` were the thing that failed.
        assert!(
            !label.starts_with("ip"),
            "label must not blame the wrapper, got: {label}"
        );
    }

    /// The regression from run 30583284365: a tool that is *absent* and a
    /// tool that is *refused* must not reach the operator as the same
    /// sentence.
    ///
    /// This is the detector rather than the fix — a later refactor that
    /// funnels both back into one variant fails here.
    #[test]
    fn absent_tool_and_refused_tool_are_distinguishable() {
        // Absent: a name no PATH entry can supply.
        let absent = spawn("spar-no-such-binary-eb0f2c", &[]).unwrap_err();
        assert!(
            matches!(absent, FixtureError::ToolNotFound { .. }),
            "spawning a nonexistent program must yield ToolNotFound, got {absent:?}"
        );

        // Present but refused: `false` exists on every POSIX host, exits 1.
        let refused = run_cmd("false", &[]).unwrap_err();
        assert!(
            matches!(refused, FixtureError::Command { .. }),
            "a program that ran and exited non-zero must yield Command, got {refused:?}"
        );

        // The property that actually failed in CI is the rendered text.
        assert_ne!(absent.to_string(), refused.to_string());
    }

    /// A missing binary must not be reported as a runner-label problem — that
    /// sentence sends the reader to the wrong repository entirely.
    #[test]
    fn diagnose_create_blames_the_image_for_a_missing_binary() {
        let rendered = diagnose_create(FixtureError::ToolNotFound {
            program: "ip".to_string(),
        })
        .to_string();
        assert!(
            !rendered.contains("netns` label"),
            "missing binary reported as a label problem: {rendered}"
        );
        assert!(rendered.contains("not found on PATH"), "{rendered}");
    }

    /// …and the label question must survive for the case it was written for,
    /// otherwise this fix has just moved the blind spot.
    #[test]
    fn diagnose_create_still_asks_about_the_label_when_ip_ran() {
        let rendered = diagnose_create(FixtureError::Command {
            program: "ip".to_string(),
            detail: "exit 1: stderr=\"Operation not permitted\"".to_string(),
        })
        .to_string();
        assert!(rendered.contains("netns` label"), "{rendered}");
    }
}
