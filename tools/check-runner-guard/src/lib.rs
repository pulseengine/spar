//! Every self-hosted `runs-on` a pull request can reach carries the fail-closed
//! fork guard.
//!
//! A fork's pull request, once a maintainer approves its run, executes the
//! fork's code on whatever runner the job names. On a self-hosted runner that is
//! our machine, our network and whatever a previous job left behind. The guard
//! sends such a run to a GitHub-hosted image instead:
//!
//! ```text
//! ${{ (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && fromJSON('[...labels...]') || 'ubuntu-24.04' }}
//! ```
//!
//! It is fail-closed: the self-hosted branch is taken only when the event is not
//! a pull request, or the head repository is provably this one.
//!
//! What this checks, per workflow under `.github/workflows/`:
//!
//! * **PR-reachable** (`on:` names `pull_request` or `pull_request_target`):
//!   every `runs-on` naming `self-hosted` must be exactly the guard above; a
//!   `runs-on` expression that is neither the guard nor `${{ matrix.<key> }}` is
//!   a finding, because the checker cannot tell where it resolves; and
//!   `self-hosted` anywhere else outside a comment (a matrix value, say) is a
//!   finding, because a matrix `runs-on` would carry it unguarded.
//! * **Not PR-reachable**: a workflow that names `self-hosted` must be listed in
//!   [`EXEMPT`] with its reason, and every entry there must exist, name
//!   `self-hosted`, and not be PR-reachable. The list is exact both ways, so a
//!   workflow that gains a `pull_request` trigger leaves it loudly.
//!
//! The reader is line-based (std only) and fails closed: a workflow with no
//! readable `on:`, or a `runs-on` expression split over lines, is a finding,
//! not a pass. Exit 0 = no finding, 1 = findings, 2 = could not read the tree.

use std::io::Write;
use std::path::Path;

pub const GUARD_HEAD: &str = "${{ (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository) && ";
pub const GUARD_TAIL: &str = " || 'ubuntu-24.04' }}";

/// Workflows that run on self-hosted runners and that no pull request can
/// trigger, with the triggers that make that so.
pub const EXEMPT: &[(&str, &str)] = &[
    ("fixture-vm-flake-lock.yml", "workflow_dispatch only"),
    (
        "mutants-weekly.yml",
        "schedule, push to main, workflow_dispatch",
    ),
    ("trace-fixtures.yml", "schedule, workflow_dispatch"),
    ("verification-weekly.yml", "schedule, workflow_dispatch"),
];

const PR_EVENTS: &[&str] = &["pull_request", "pull_request_target"];

#[derive(Debug, PartialEq)]
pub struct Site {
    pub line: usize,
    pub value: String,
}

fn is_comment(l: &str) -> bool {
    l.trim_start().starts_with('#')
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// Drop a trailing ` # comment`. None of the values read here hold a `#`.
fn strip_comment(v: &str) -> &str {
    v.split(" #").next().unwrap_or(v).trim()
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

/// The workflow's trigger names, or an error if `on:` cannot be read.
pub fn triggers(text: &str) -> Result<Vec<String>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let pos = lines.iter().position(|l| {
        indent(l) == 0
            && matches!(l.split(':').next().map(unquote), Some("on") | Some("true"))
            && l.contains(':')
    });
    let Some(i) = pos else {
        return Err("no top-level `on:`".into());
    };
    let value = strip_comment(lines[i].split_once(':').unwrap().1);
    if let Some(inner) = value.strip_prefix('[') {
        let inner = inner
            .strip_suffix(']')
            .ok_or("`on: [` is not closed on its line")?;
        return Ok(inner
            .split(',')
            .map(|t| unquote(t).to_owned())
            .filter(|t| !t.is_empty())
            .collect());
    }
    if !value.is_empty() {
        return Ok(vec![unquote(value).to_owned()]);
    }
    let mut out = vec![];
    let mut key_indent = None;
    for l in &lines[i + 1..] {
        if l.trim().is_empty() || is_comment(l) {
            continue;
        }
        let ind = indent(l);
        if ind == 0 {
            break;
        }
        let ki = *key_indent.get_or_insert(ind);
        if ind != ki {
            continue;
        }
        let t = l.trim();
        let name = t.strip_prefix("- ").unwrap_or(t);
        out.push(unquote(name.split(':').next().unwrap()).to_owned());
    }
    if out.is_empty() {
        return Err("`on:` names no trigger".into());
    }
    Ok(out)
}

/// Every `runs-on` in the file, with its value joined onto one line, and the
/// line numbers it covered.
pub fn sites(text: &str) -> (Vec<Site>, Vec<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let (mut out, mut covered) = (vec![], vec![]);
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if !is_comment(l)
            && let Some(rest) = l.trim_start().strip_prefix("runs-on:")
        {
            let mut value = strip_comment(rest).to_owned();
            let start = i + 1;
            covered.push(start);
            if value.is_empty() {
                let base = indent(l);
                while i + 1 < lines.len()
                    && (lines[i + 1].trim().is_empty() || indent(lines[i + 1]) > base)
                {
                    i += 1;
                    covered.push(i + 1);
                    if !is_comment(lines[i]) && !lines[i].trim().is_empty() {
                        value.push(' ');
                        value.push_str(strip_comment(lines[i]));
                    }
                }
            }
            out.push(Site {
                line: start,
                value: value.trim().to_owned(),
            });
        }
        i += 1;
    }
    (out, covered)
}

pub fn is_guarded(v: &str) -> bool {
    v.starts_with(GUARD_HEAD) && v.ends_with(GUARD_TAIL) && v.contains("self-hosted")
}

fn is_matrix_ref(v: &str) -> bool {
    v.strip_prefix("${{ matrix.")
        .and_then(|r| r.strip_suffix(" }}"))
        .is_some_and(|k| {
            !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// Findings for one workflow file.
pub fn check_workflow(name: &str, text: &str) -> (bool, usize, usize, Vec<String>) {
    let mut findings = vec![];
    let trig = match triggers(text) {
        Ok(t) => t,
        Err(e) => {
            return (
                true,
                0,
                0,
                vec![format!(
                    "UNREADABLE {name}: {e}; treated as PR-reachable and failed"
                )],
            );
        }
    };
    let pr = trig.iter().any(|t| PR_EVENTS.contains(&t.as_str()));
    let (sites, covered) = sites(text);
    let mut n_self = 0;
    let mut n_guarded = 0;
    for s in &sites {
        let self_hosted = s.value.contains("self-hosted");
        if self_hosted {
            n_self += 1;
        }
        if is_guarded(&s.value) {
            n_guarded += 1;
        }
        if !pr {
            continue;
        }
        if s.value.starts_with("${{") && !s.value.ends_with("}}") {
            findings.push(format!(
                "UNREADABLE {name}:{} runs-on expression does not close on its line: {}",
                s.line, s.value
            ));
        } else if is_guarded(&s.value) || is_matrix_ref(&s.value) {
            // guarded, or a matrix key whose values the OUTSIDE scan reads
        } else if s.value.contains("${{") {
            findings.push(format!(
                "UNRESOLVED {name}:{} runs-on expression is neither the guard nor a matrix key: {}",
                s.line, s.value
            ));
        } else if self_hosted {
            findings.push(format!("UNGUARDED {name}:{} runs-on: {}", s.line, s.value));
        }
    }
    if pr {
        for (n, l) in text.lines().enumerate() {
            if !is_comment(l) && l.contains("self-hosted") && !covered.contains(&(n + 1)) {
                findings.push(format!(
                    "OUTSIDE   {name}:{} `self-hosted` outside a runs-on: {}",
                    n + 1,
                    l.trim()
                ));
            }
        }
    }
    (pr, n_self, n_guarded, findings)
}

/// Check every workflow under `root/.github/workflows`; returns the exit code.
pub fn run(root: &Path, out: &mut dyn Write) -> i32 {
    let dir = root.join(".github/workflows");
    let mut files: Vec<_> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
            .collect(),
        Err(e) => {
            let _ = writeln!(out, "cannot read {}: {e}", dir.display());
            return 2;
        }
    };
    files.sort();
    if files.is_empty() {
        let _ = writeln!(
            out,
            "no workflow files under {}: nothing was checked",
            dir.display()
        );
        return 2;
    }
    let _ = writeln!(out, "== self-hosted runner fork guard ==");
    let mut findings = vec![];
    let (mut tot_self, mut tot_guarded) = (0, 0);
    let mut exempt_seen = vec![];
    for f in &files {
        let text = match std::fs::read_to_string(dir.join(f)) {
            Ok(t) => t,
            Err(e) => {
                let _ = writeln!(out, "cannot read {f}: {e}");
                return 2;
            }
        };
        let (pr, n_self, n_guarded, mut fs) = check_workflow(f, &text);
        let mentions_self_hosted = text
            .lines()
            .any(|l| !is_comment(l) && l.contains("self-hosted"));
        let exempt = EXEMPT.iter().find(|(n, _)| n == f);
        let kind = match (pr, exempt) {
            (true, Some(_)) => {
                fs.push(format!(
                    "EXEMPT    {f} is listed as exempt but a pull request can trigger it"
                ));
                "PR "
            }
            (true, None) => "PR ",
            (false, Some(_)) => {
                exempt_seen.push(f.clone());
                if !mentions_self_hosted {
                    fs.push(format!("EXEMPT    {f} is listed as exempt but names no self-hosted runner; drop the entry"));
                }
                "exempt"
            }
            (false, None) => {
                if mentions_self_hosted {
                    fs.push(format!("EXEMPT    {f} uses a self-hosted runner, is not PR-reachable, and is missing from EXEMPT"));
                }
                "-  "
            }
        };
        if pr {
            tot_self += n_self;
            tot_guarded += n_guarded;
        }
        let _ = writeln!(
            out,
            "  {kind:6} {f}: {n_self} self-hosted runs-on, {n_guarded} guarded"
        );
        findings.extend(fs);
    }
    for (n, _) in EXEMPT {
        if !files.iter().any(|f| f == n) {
            findings.push(format!(
                "EXEMPT    {n} is listed as exempt but does not exist"
            ));
        }
    }
    let _ = writeln!(
        out,
        "\n{} workflow(s); PR-reachable self-hosted runs-on: {tot_self}, guarded: {tot_guarded}; exempt: {}",
        files.len(),
        exempt_seen.join(", ")
    );
    for f in &findings {
        let _ = writeln!(out, "  {f}");
    }
    let _ = writeln!(out, "{} finding(s)", findings.len());
    i32::from(!findings.is_empty())
}

#[cfg(test)]
mod tests;
