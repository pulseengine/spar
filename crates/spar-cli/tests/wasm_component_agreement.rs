//! The spar-wasm component, run headless under wasmtime, against native
//! `spar analyze` (REQ-WASM-COMPONENT-HEADLESS-001).
//!
//! spar-wasm ships every release as a wasm32-wasip2 component (release.yml,
//! "Build spar-wasm"), and rivet's dashboard runs it transpiled in a browser.
//! Until this test nothing ran the component itself: a build that compiled
//! and analysed differently on wasm32 would have shipped. Here every system
//! implementation in an accepted first-party model (the `ACCEPT` rows of
//! `test-data/interop/baseline/osate-first-party.tsv`) is a root. For each
//! root the file is copied alone into a fresh directory, and:
//!
//! - the component's `analyze(root)` export runs under
//!   `wasmtime run --dir .::. --invoke`, which prints the result as WAVE;
//! - native `spar analyze --format json --root <root> <file>` runs on the
//!   same file.
//!
//! The component registers a subset of `register_all` (no solver-backed
//! passes on wasm32). `spar_wasm::component_runner()` is that subset, so the
//! native diagnostics are restricted to its analysis names, not to a list
//! copied here. Per root, the two multisets of (severity, analysis, path,
//! message) must be equal. A component error (`no-root`, `parse-error`) or
//! a wasmtime failure is a disagreement.
//!
//! A planted model must produce a named error through the component, so the
//! comparison cannot pass on two empty outputs.
//!
//! The test is `#[ignore]`d: it needs `SPAR_WASM_COMPONENT` (the built
//! `spar_wasm.wasm`) and `wasmtime` on PATH. The CI job "Wasm component
//! oracle" provides both and runs it with `--ignored`; without them it
//! fails rather than skips.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The fewest roots whose diagnostics both sides produced and compared. At
/// enrolment 76 roots were enumerated (48 accepted files with a system
/// implementation): 74 compared, and 2 refused by both sides because the
/// file `with`s packages from files beside it and is analysed alone. A drop
/// below this means the enumeration or a run broke, not that the corpus
/// shrank on purpose.
const MIN_ROOTS: usize = 70;

type Diag = (String, String, String, String); // severity, analysis, path, message

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn component() -> PathBuf {
    let p = std::env::var_os("SPAR_WASM_COMPONENT")
        .map(PathBuf::from)
        .expect(
            "SPAR_WASM_COMPONENT must name the built spar_wasm.wasm \
         (cargo build --release --target wasm32-wasip2 -p spar-wasm)",
        );
    assert!(
        p.is_file(),
        "SPAR_WASM_COMPONENT={} is not a file",
        p.display()
    );
    p
}

/// `Package::Type.Impl` for every `system implementation` in `src`, paired
/// with the nearest `package` above it.
fn roots(src: &str) -> Vec<String> {
    let mut pkg = None;
    let mut out = Vec::new();
    for line in src.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [p, name, ..] if p.eq_ignore_ascii_case("package") => pkg = Some(name.to_string()),
            [s, i, name, ..]
                if s.eq_ignore_ascii_case("system") && i.eq_ignore_ascii_case("implementation") =>
            {
                if let Some(p) = &pkg {
                    out.push(format!("{p}::{name}"));
                }
            }
            _ => {}
        }
    }
    out
}

fn accepted_first_party() -> Vec<PathBuf> {
    let tsv = repo().join("test-data/interop/baseline/osate-first-party.tsv");
    std::fs::read_to_string(&tsv)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let mut cols = l.split('\t');
            let path = cols.next()?;
            (cols.next()? == "ACCEPT").then(|| repo().join(path))
        })
        .collect()
}

/// A fresh directory holding only `file`, under the test's temp root.
fn alone(file: &Path, n: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("spar_wasm_oracle_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(file, dir.join(file.file_name().unwrap())).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// WAVE: the subset wasmtime prints for result<list<analysis-diagnostic>, _>
// ---------------------------------------------------------------------------

struct Wave<'a> {
    s: &'a [u8],
    i: usize,
}

impl Wave<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &str) -> Result<(), String> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(format!("expected {t:?} at byte {}", self.i))
        }
    }
    fn ident(&mut self) -> String {
        self.ws();
        let start = self.i;
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'-')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect("\"")?;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i).ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'u' => {
                            self.expect("{")?;
                            let start = self.i;
                            while self.s[self.i] != b'}' {
                                self.i += 1;
                            }
                            let hex = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                            self.i += 1;
                            let ch = char::from_u32(
                                u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?,
                            )
                            .ok_or("bad code point")?;
                            out.extend(ch.to_string().as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        String::from_utf8(out).map_err(|e| e.to_string())
    }
    fn diagnostic(&mut self) -> Result<Diag, String> {
        self.expect("{")?;
        let mut f = BTreeMap::new();
        loop {
            let k = self.ident();
            self.expect(":")?;
            f.insert(k, self.string()?);
            if !self.eat(",") {
                break;
            }
            if self.eat("}") {
                self.i -= 1;
                break;
            }
        }
        self.expect("}")?;
        let mut get = |k: &str| f.remove(k).ok_or(format!("record lacks {k}"));
        Ok((
            get("severity")?,
            get("analysis-name")?,
            get("component-path")?,
            get("message")?,
        ))
    }
}

/// `Ok(diagnostics)` for `ok([...])`, `Err(text)` for anything else.
fn parse_wave(out: &str) -> Result<Vec<Diag>, String> {
    let mut w = Wave {
        s: out.trim().as_bytes(),
        i: 0,
    };
    if !w.eat("ok(") {
        return Err(out.trim().to_string());
    }
    w.expect("[")?;
    let mut v = Vec::new();
    if !w.eat("]") {
        loop {
            v.push(w.diagnostic()?);
            if w.eat("]") {
                break;
            }
            w.expect(",")?;
            if w.eat("]") {
                break;
            }
        }
    }
    w.expect(")")?;
    Ok(v)
}

#[test]
fn wave_parser_reads_wasmtime_output() {
    let out = r#"ok([{severity: "error", message: "it\'s \"q\" \u{e9}", component-path: "A.impl.b", analysis-name: "hierarchy"}])"#;
    assert_eq!(
        parse_wave(out).unwrap(),
        vec![(
            "error".into(),
            "hierarchy".into(),
            "A.impl.b".into(),
            "it's \"q\" é".into()
        )]
    );
    assert_eq!(parse_wave("ok([])").unwrap(), vec![]);
    assert!(parse_wave(r#"err(no-root("x"))"#).is_err());
}

#[test]
fn roots_pair_each_system_implementation_with_its_package() {
    let src = "package A\npublic\n  system implementation S.i\n  end S.i;\n  process implementation P.i\nend A;\n\
               package B::C\npublic\n  SYSTEM IMPLEMENTATION T.impl\nend B::C;\n";
    assert_eq!(roots(src), ["A::S.i", "B::C::T.impl"]);
}

// ---------------------------------------------------------------------------
// The two runs
// ---------------------------------------------------------------------------

fn run_component(wasm: &Path, dir: &Path, root: &str) -> Result<Vec<Diag>, String> {
    let out = Command::new("wasmtime")
        .current_dir(dir)
        .args([
            "run",
            "--dir",
            ".::.",
            "--invoke",
            &format!("analyze({root:?})"),
        ])
        .arg(wasm)
        .output()
        .map_err(|e| format!("cannot run wasmtime: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "wasmtime exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    parse_wave(&String::from_utf8_lossy(&out.stdout))
}

fn run_native(file: &Path, root: &str, names: &[&str]) -> Result<Vec<Diag>, String> {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--format", "json", "--root", root])
        .arg(file)
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "native exit {:?}, no JSON ({e}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    })?;
    let diags = v["diagnostics"]
        .as_array()
        .ok_or("native JSON lacks diagnostics")?;
    Ok(diags
        .iter()
        .filter(|d| names.contains(&d["analysis"].as_str().unwrap_or("")))
        .map(|d| {
            let path: Vec<&str> = d["path"]
                .as_array()
                .map(|p| p.iter().filter_map(|s| s.as_str()).collect())
                .unwrap_or_default();
            (
                d["severity"].as_str().unwrap_or("").to_string(),
                d["analysis"].as_str().unwrap_or("").to_string(),
                path.join("."),
                d["message"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect())
}

fn sorted(mut v: Vec<Diag>) -> Vec<Diag> {
    v.sort();
    v
}

#[test]
#[ignore = "needs SPAR_WASM_COMPONENT and wasmtime; the CI job \"Wasm component oracle\" runs it"]
fn component_agrees_with_native_analyze() {
    let wasm = component();
    let runner = spar_wasm::component_runner();
    let names = runner.names();
    let mut compared = 0;
    let mut both_refused = Vec::new();
    let mut disagreements = Vec::new();

    for (n, file) in accepted_first_party().iter().enumerate() {
        let src = std::fs::read_to_string(file).unwrap();
        for root in roots(&src) {
            let dir = alone(file, n);
            let local = dir.join(file.file_name().unwrap());
            let rel = file
                .strip_prefix(repo())
                .unwrap_or(file)
                .display()
                .to_string();
            let comp = run_component(&wasm, &dir, &root);
            let native = run_native(&local, &root, &names);
            match (comp, native) {
                (Ok(c), Ok(nat)) => {
                    compared += 1;
                    if let Some(stray) = c.iter().find(|d| !names.contains(&d.1.as_str())) {
                        disagreements.push(format!(
                            "{rel} {root}: component emitted analysis {:?}, which component_runner() does not name",
                            stray.1
                        ));
                    }
                    let (c, nat) = (sorted(c), sorted(nat));
                    if c != nat {
                        let only_c: Vec<_> = c.iter().filter(|d| !nat.contains(d)).collect();
                        let only_n: Vec<_> = nat.iter().filter(|d| !c.contains(d)).collect();
                        disagreements.push(format!(
                            "{rel} {root}: component {} vs native {} diagnostics\n  only component: {only_c:?}\n  only native: {only_n:?}",
                            c.len(),
                            nat.len()
                        ));
                    }
                }
                // Both refuse to instantiate the root: the same verdict.
                (Err(c), Err(nat))
                    if c.starts_with("err(no-root(") && nat.contains("cannot instantiate root") =>
                {
                    both_refused.push(format!("{rel} {root}"));
                }
                (c, nat) => disagreements.push(format!(
                    "{rel} {root}: component {:?} / native {:?}",
                    c.map(|v| v.len()),
                    nat.map(|v| v.len())
                )),
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    eprintln!(
        "compared {compared} roots, {} refused by both, {} disagreements",
        both_refused.len(),
        disagreements.len()
    );
    for r in &both_refused {
        eprintln!("  refused by both: {r}");
    }
    assert!(
        compared >= MIN_ROOTS,
        "only {compared} roots compared, fewer than {MIN_ROOTS}"
    );
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

/// Connection `c` joins two `in` ports across siblings, so its source has
/// the wrong direction; direction_rules must report it on Top.impl. It is
/// written to a temp dir, not kept as a fixture, so the first-party corpus
/// (first_party_legality, osate_agreement) does not count it.
const PLANTED: &str = "package Planted
public
  process Node
    features
      p : in data port;
  end Node;
  process implementation Node.impl
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      a : process Node.impl;
      b : process Node.impl;
    connections
      c : port a.p -> b.p;
  end Top.impl;
end Planted;
";

/// A model with a known defect must produce that error through the
/// component, so agreement cannot come from two empty outputs.
#[test]
#[ignore = "needs SPAR_WASM_COMPONENT and wasmtime; the CI job \"Wasm component oracle\" runs it"]
fn planted_defect_is_reported_by_the_component() {
    let wasm = component();
    let dir = std::env::temp_dir().join(format!("spar_wasm_planted_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("planted.aadl"), PLANTED).unwrap();
    let diags = run_component(&wasm, &dir, "Planted::Top.impl").unwrap_or_else(|e| panic!("{e}"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        diags.iter().any(|(sev, analysis, path, _)| sev == "error"
            && analysis == "direction_rules"
            && path == "Top.impl"),
        "the planted direction error is missing: {diags:#?}"
    );
}
