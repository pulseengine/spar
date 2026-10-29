//! Names of generated outputs, chosen so that two components never share one
//! output path.
//!
//! Proof files (#510) and the per-process WIT, crate, config, test and design
//! doc outputs (#543) were named by the component's NAME. Two processes called
//! `app` in different subsystems therefore wrote `wit/app.wit`,
//! `crates/app/*` and `config/app.toml` twice, and the second silently
//! overwrote the first. The names here come from the instance path instead,
//! through one injective escape alphabet: [`encode_instance_path`] for proof
//! names, and [`encode_instance_path_words`] (joined as a snake-case crate name
//! or a kebab-case WIT name) for the per-process outputs.
//!
//! A model where every process name is distinct keeps its plain names
//! ([`process_names`]), so existing output does not change.

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

use crate::sanitize_ident;
use crate::wit_gen::wit_ident;

/// The instance path of `idx` below the root, as its segment names
/// (`left`, `cpu` for `Top.impl/left/cpu`). Empty for the root itself.
pub(crate) fn path_below_root(inst: &SystemInstance, idx: ComponentInstanceIdx) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = idx;
    while let Some(parent) = inst.component(current).parent {
        segments.push(inst.component(current).name.as_str().to_string());
        current = parent;
    }
    segments.reverse();
    segments
}

/// Encode an instance path as a lowercase identifier, injectively (#510).
///
/// Proof files were named by the component's NAME, so two processors called
/// `cpu` in different subsystems wrote one file and the second overwrote the
/// first. The name is now built from the whole instance path below the root:
/// `left`/`cpu` gives `left__cpu`, and a processor directly under the root
/// keeps its plain name (`cpu`).
///
/// The output uses only `[a-z0-9_]`. Two different paths therefore never give
/// names that differ only in case, which a case-insensitive filesystem would
/// fold into one file. The output is also a valid Lean and Rust identifier
/// stem: it never starts with a digit or `_`.
///
/// Injectivity comes from the escape `z`, which always starts an escape
/// (prefix-free codes):
///
/// | input                                   | output             |
/// |-----------------------------------------|--------------------|
/// | `a`–`y`, `0`–`9`                        | itself             |
/// | `z`                                     | `zz`               |
/// | uppercase letter `L`                    | `zc` + lowercase L |
/// | `_` between two non-`_` characters      | `_`                |
/// | any other `_` (leading, trailing, run)  | `zu`               |
/// | any other character                     | `zx` + 6 hex digits |
/// | empty segment                           | `ze`               |
/// | empty path (the root)                   | `zr`               |
/// | leading digit of the whole name         | `zd` before it     |
///
/// An encoded segment never contains `__` and never starts or ends with `_`,
/// so joining segments with `__` can be split back unambiguously. The test
/// decoder in `proof_gen`'s tests recovers every path from its encoding,
/// which is what makes the map injective.
pub(crate) fn encode_instance_path<S: AsRef<str>>(segments: &[S]) -> String {
    if segments.is_empty() {
        return "zr".to_string();
    }
    let mut out = String::new();
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str("__");
        }
        encode_segment(segment.as_ref(), &mut out);
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "zd");
    }
    out
}

fn encode_segment(segment: &str, out: &mut String) {
    let chars: Vec<char> = segment.chars().collect();
    if chars.is_empty() {
        out.push_str("ze");
        return;
    }
    for (i, &c) in chars.iter().enumerate() {
        match c {
            'z' => out.push_str("zz"),
            'a'..='y' | '0'..='9' => out.push(c),
            'A'..='Z' => {
                out.push_str("zc");
                out.push(c.to_ascii_lowercase());
            }
            '_' => {
                let interior =
                    i > 0 && i + 1 < chars.len() && chars[i - 1] != '_' && chars[i + 1] != '_';
                out.push_str(if interior { "_" } else { "zu" });
            }
            other => out.push_str(&format!("zx{:06x}", other as u32)),
        }
    }
}

/// Encode an instance path as words `[a-z][a-z0-9]*`, one per segment,
/// injectively (#543).
///
/// [`encode_instance_path`] joins segments with `__`. That is fine for a file
/// name or a Lean identifier, but not for the per-process outputs: a WIT
/// identifier is words joined by single `-` and has no `_` at all, and rustc
/// warns `non_snake_case` on a crate named `left__app` (the generated crate
/// must stay warning-clean). This is the same escape alphabet with three
/// changes, so that each segment becomes one word:
///
/// - every `_` is escaped as `zu`, even an interior one, so a word is
///   `[a-z0-9]` only;
/// - a segment that would start with a digit gets `zd` before it, so every
///   word starts with a letter (`zd` is produced nowhere else);
/// - the caller joins the words with one separator, `-` for WIT and `_` for a
///   crate. Neither occurs inside a word, so the words split back
///   unambiguously, and each word decodes back to its segment.
///
/// `left`/`app` gives the words `left`, `app`; `sensor_proc` gives
/// `sensorzuproc`. A word is never empty (an empty segment is `ze`), and the
/// root (no segments) is the single word `zr`.
pub(crate) fn encode_instance_path_words<S: AsRef<str>>(segments: &[S]) -> Vec<String> {
    if segments.is_empty() {
        return vec!["zr".to_string()];
    }
    segments
        .iter()
        .map(|segment| {
            let chars: Vec<char> = segment.as_ref().chars().collect();
            let mut word = String::new();
            if chars.is_empty() {
                word.push_str("ze");
            }
            if chars.first().is_some_and(|c| c.is_ascii_digit()) {
                word.push_str("zd");
            }
            for &c in &chars {
                match c {
                    'z' => word.push_str("zz"),
                    'a'..='y' | '0'..='9' => word.push(c),
                    'A'..='Z' => {
                        word.push_str("zc");
                        word.push(c.to_ascii_lowercase());
                    }
                    '_' => word.push_str("zu"),
                    other => word.push_str(&format!("zx{:06x}", other as u32)),
                }
            }
            word
        })
        .collect()
}

/// The kebab-case WIT form: the words joined with `-` (`left-app`). A name
/// that is a WIT keyword is `%`-escaped, as [`wit_ident`] does; no other name
/// starts with `%`, so the escape keeps the map injective.
pub(crate) fn encode_instance_path_kebab<S: AsRef<str>>(segments: &[S]) -> String {
    let out = encode_instance_path_words(segments).join("-");
    if crate::wit_gen::is_wit_keyword(&out) {
        format!("%{out}")
    } else {
        out
    }
}

/// The snake-case crate form: the words joined with `_` (`left_app`). A name
/// that is a Rust keyword gets `_` after it, as [`sanitize_ident`] does; no
/// other name ends with `_`, so the suffix keeps the map injective.
pub(crate) fn encode_instance_path_snake<S: AsRef<str>>(segments: &[S]) -> String {
    let mut out = encode_instance_path_words(segments).join("_");
    if crate::RUST_KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// The names one process's outputs are built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessNames {
    /// The Rust crate directory and Cargo package name (`crates/{crate_name}`),
    /// also the stem of `config/`, `tests/`, `docs/design/` and
    /// `verification/` outputs and of the `aadl-contract.toml` keys.
    pub crate_name: String,
    /// The WIT file stem, package name and `{wit_name}-ports` /
    /// `{wit_name}-world` prefix.
    pub wit_name: String,
}

/// The plain names: what codegen has always used.
fn plain_names(name: &str) -> ProcessNames {
    ProcessNames {
        crate_name: sanitize_ident(name),
        wit_name: wit_ident(name),
    }
}

/// The instance-path names, injective over all processes.
fn encoded_names(inst: &SystemInstance, idx: ComponentInstanceIdx) -> ProcessNames {
    let path = path_below_root(inst, idx);
    ProcessNames {
        crate_name: encode_instance_path_snake(&path),
        wit_name: encode_instance_path_kebab(&path),
    }
}

/// True when no two entries of `names` are equal, ignoring ASCII case (a
/// case-insensitive filesystem folds `App` and `app` into one file).
fn pairwise_distinct(names: &[&ProcessNames]) -> bool {
    let mut crates = std::collections::BTreeSet::new();
    let mut wits = std::collections::BTreeSet::new();
    names.iter().all(|n| {
        crates.insert(n.crate_name.to_ascii_lowercase())
            && wits.insert(n.wit_name.to_ascii_lowercase())
    })
}

/// The output names of every process in the model, in instance order.
///
/// The rule, chosen so a model without a name collision sees no change:
///
/// 1. A process whose plain crate name or plain WIT name equals another
///    process's gets its instance-path names ([`encode_instance_path_snake`],
///    [`encode_instance_path_kebab`]). Both of its names switch together, so
///    its crate still points at its own WIT file.
/// 2. Every other process keeps its plain names.
/// 3. If the names chosen this way still collide (a plain name that happens
///    to equal another process's encoded name), every process gets its
///    instance-path names. Those are injective, so this always succeeds.
///
/// The result depends only on the instance model, so every generator that
/// asks gets the same answer.
pub(crate) fn process_name_table(
    inst: &SystemInstance,
) -> Vec<(ComponentInstanceIdx, ProcessNames)> {
    let processes: Vec<ComponentInstanceIdx> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Process)
        .map(|(idx, _)| idx)
        .collect();
    let plain: Vec<ProcessNames> = processes
        .iter()
        .map(|&idx| plain_names(inst.component(idx).name.as_str()))
        .collect();

    let collides = |i: usize| {
        plain.iter().enumerate().any(|(j, other)| {
            j != i
                && (other.crate_name.eq_ignore_ascii_case(&plain[i].crate_name)
                    || other.wit_name.eq_ignore_ascii_case(&plain[i].wit_name))
        })
    };
    let chosen: Vec<ProcessNames> = (0..processes.len())
        .map(|i| {
            if collides(i) {
                encoded_names(inst, processes[i])
            } else {
                plain[i].clone()
            }
        })
        .collect();

    let names = if pairwise_distinct(&chosen.iter().collect::<Vec<_>>()) {
        chosen
    } else {
        processes
            .iter()
            .map(|&idx| encoded_names(inst, idx))
            .collect()
    };
    processes.into_iter().zip(names).collect()
}

/// The output names of process `proc_idx`. See [`process_name_table`].
///
/// Falls back to the plain names when `proc_idx` is not a process, so a
/// caller handing in some other component gets what it got before #543.
pub(crate) fn process_names(inst: &SystemInstance, proc_idx: ComponentInstanceIdx) -> ProcessNames {
    process_name_table(inst)
        .into_iter()
        .find(|(idx, _)| *idx == proc_idx)
        .map(|(_, names)| names)
        .unwrap_or_else(|| plain_names(inst.component(proc_idx).name.as_str()))
}

/// The crate directory a thread's per-thread outputs go under: its process's
/// [`ProcessNames::crate_name`]. A thread whose parent is not a process
/// (a thread group, say) keeps the parent's plain name, as before, through
/// [`process_names`]' fallback; a thread with no parent gets `orphan`.
pub(crate) fn parent_crate_dir(
    inst: &SystemInstance,
    thread_idx: ComponentInstanceIdx,
    orphan: &str,
) -> String {
    match inst.component(thread_idx).parent {
        Some(p) => process_names(inst, p).crate_name,
        None => orphan.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::name::Name;
    use spar_hir_def::resolver::GlobalScope;

    /// Test-only inverse of [`encode_instance_path_words`] joined with `sep`.
    /// That it recovers every path is the proof the map is injective.
    fn decode_words(encoded: &str, sep: char) -> Vec<String> {
        if encoded == "zr" {
            return Vec::new();
        }
        encoded
            .split(sep)
            .map(|word| {
                if word == "ze" {
                    return String::new();
                }
                let word = word.strip_prefix("zd").unwrap_or(word);
                let chars: Vec<char> = word.chars().collect();
                let mut out = String::new();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i] != 'z' {
                        assert!(
                            chars[i].is_ascii_lowercase() || chars[i].is_ascii_digit(),
                            "bad character {:?} in {encoded}",
                            chars[i]
                        );
                        out.push(chars[i]);
                        i += 1;
                        continue;
                    }
                    match chars[i + 1] {
                        'z' => {
                            out.push('z');
                            i += 2;
                        }
                        'u' => {
                            out.push('_');
                            i += 2;
                        }
                        'c' => {
                            out.push(chars[i + 2].to_ascii_uppercase());
                            i += 3;
                        }
                        'x' => {
                            let hex: String = chars[i + 2..i + 8].iter().collect();
                            let code = u32::from_str_radix(&hex, 16).unwrap();
                            out.push(char::from_u32(code).unwrap());
                            i += 8;
                        }
                        other => panic!("bad escape z{other} in {encoded}"),
                    }
                }
                out
            })
            .collect()
    }

    /// Every path of 1 to 3 segments over an alphabet chosen to collide under
    /// naive mangling (case, the escape letter `z`, `_`, `.`, `-`, a digit, a
    /// non-ASCII letter): one segment of up to 3 characters, two of up to 2,
    /// or three of up to 1.
    fn tricky_paths() -> Vec<Vec<String>> {
        let alphabet = ['a', 'A', 'z', 'Z', '_', '.', '-', '1', 'é'];
        let mut segments = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..3 {
            frontier = frontier
                .iter()
                .flat_map(|s| alphabet.iter().map(move |c| format!("{s}{c}")))
                .collect();
            segments.extend(frontier.iter().cloned());
        }
        let up_to = |n: usize| -> Vec<&String> {
            segments.iter().filter(|s| s.chars().count() <= n).collect()
        };
        let mut paths: Vec<Vec<String>> = segments.iter().map(|s| vec![s.clone()]).collect();
        for a in up_to(2) {
            for b in up_to(2) {
                paths.push(vec![a.clone(), b.clone()]);
            }
        }
        for a in up_to(1) {
            for b in up_to(1) {
                for c in up_to(1) {
                    paths.push(vec![a.clone(), b.clone(), c.clone()]);
                }
            }
        }
        paths
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    #[cfg_attr(miri, ignore = "exhaustive enumeration is too slow under Miri")]
    fn crate_and_wit_names_are_injective_over_tricky_paths() {
        let mut crates = std::collections::BTreeMap::new();
        let mut wits = std::collections::BTreeMap::new();
        for path in tricky_paths() {
            let snake = encode_instance_path_snake(&path);
            let kebab = encode_instance_path_kebab(&path);
            assert_eq!(decode_words(&snake, '_'), path, "{snake}");
            assert_eq!(decode_words(&kebab, '-'), path, "{kebab}");
            // Compared lowercased, so no two names fold together on a
            // case-insensitive filesystem.
            for (name, map) in [(&snake, &mut crates), (&kebab, &mut wits)] {
                if let Some(prev) = map.insert(name.to_ascii_lowercase(), path.clone()) {
                    panic!("{prev:?} and {path:?} both give {name}");
                }
            }
        }
        assert!(crates.len() > 10_000, "{}", crates.len());
    }

    /// Every word is `[a-z][a-z0-9]*`, so the crate name is a valid Cargo
    /// package name in rustc's snake case (no `__`: rustc's `non_snake_case`
    /// warns on a crate named `left__app`), and the WIT name parses as a WIT
    /// package, interface and world name.
    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    #[cfg_attr(miri, ignore = "wit-parser over many names is too slow under Miri")]
    fn crate_and_wit_names_are_valid_identifiers() {
        let paths: Vec<Vec<String>> = tricky_paths().into_iter().step_by(37).collect();
        assert!(paths.len() > 100);
        for path in paths
            .into_iter()
            .chain([vec!["left".into(), "app".into()], vec!["1st".into()]])
        {
            let snake = encode_instance_path_snake(&path);
            assert!(
                snake.starts_with(|c: char| c.is_ascii_lowercase())
                    && snake
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                    && !snake.contains("__")
                    && !snake.ends_with('_'),
                "{path:?} -> crate name {snake:?}"
            );
            let kebab = encode_instance_path_kebab(&path);
            let wit = format!(
                "package spar:{kebab};\ninterface {kebab}-ports {{}}\n\
                 world {kebab}-world {{ import {kebab}-ports; }}\n"
            );
            let mut resolve = wit_parser::Resolve::new();
            resolve
                .push_str("t.wit", &wit)
                .unwrap_or_else(|e| panic!("{path:?} -> {kebab:?} is not valid WIT: {e}\n{wit}"));
        }
        assert_eq!(encode_instance_path_kebab(&["left", "app"]), "left-app");
        assert_eq!(encode_instance_path_snake(&["left", "app"]), "left_app");
        assert_eq!(encode_instance_path_snake(&["sensor_proc"]), "sensorzuproc");
        assert_eq!(encode_instance_path_kebab(&["a", "1x"]), "a-zd1x");
        assert_eq!(encode_instance_path_kebab(&["type"]), "%type");
        assert_eq!(encode_instance_path_snake(&["fn"]), "fn_");
        assert_eq!(encode_instance_path_snake::<&str>(&[]), "zr");
    }

    fn instantiate(src: &str, pkg: &str) -> SystemInstance {
        let db = spar_hir_def::HirDefDatabase::default();
        let sf = spar_base_db::SourceFile::new(&db, "m.aadl".to_string(), src.to_string());
        let tree = spar_hir_def::file_item_tree(&db, sf);
        let scope = GlobalScope::from_trees(vec![tree]);
        SystemInstance::instantiate(
            &scope,
            &Name::new(pkg),
            &Name::new("Top"),
            &Name::new("impl"),
        )
    }

    /// `(instance path, crate name, wit name)` for every process.
    fn table(src: &str, pkg: &str) -> Vec<(String, String, String)> {
        let inst = instantiate(src, pkg);
        process_name_table(&inst)
            .into_iter()
            .map(|(idx, n)| (inst.component_instance_path(idx), n.crate_name, n.wit_name))
            .collect()
    }

    fn row(path: &str, krate: &str, wit: &str) -> (String, String, String) {
        (path.to_string(), krate.to_string(), wit.to_string())
    }

    /// `left.app` and `right.app`, plus the root-level subcomponents `extra`.
    fn subsystems(extra: &str) -> String {
        format!(
            "package N\npublic\n  process P\n  end P;\n  system Node\n  end Node;\n\
             system implementation Node.impl\n    subcomponents\n      app : process P;\n\
             end Node.impl;\n  system Top\n  end Top;\n  system implementation Top.impl\n\
             subcomponents\n      left : system Node.impl;\n      right : system Node.impl;\n\
             {extra}  end Top.impl;\nend N;\n"
        )
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn a_model_without_a_collision_keeps_its_plain_names() {
        let src = "package F\npublic\n  process P\n  end P;\n  system Top\n  end Top;\n\
                   system implementation Top.impl\n    subcomponents\n      sensor_proc : process P;\n\
                   Ctl : process P;\n  end Top.impl;\nend F;\n";
        assert_eq!(
            table(src, "F"),
            [
                row("Top.impl/sensor_proc", "sensor_proc", "sensor-proc"),
                row("Top.impl/Ctl", "ctl", "ctl"),
            ]
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn only_the_colliding_processes_are_named_by_instance_path() {
        let src = subsystems("      ctl : process P;\n");
        assert_eq!(
            table(&src, "N"),
            [
                row("Top.impl/left/app", "left_app", "left-app"),
                row("Top.impl/right/app", "right_app", "right-app"),
                row("Top.impl/ctl", "ctl", "ctl"),
            ]
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn a_plain_name_equal_to_an_encoded_name_makes_every_name_encoded() {
        // A root-level `left_app` is plain `left_app` / `left-app`: the very
        // names `left.app` encodes to. Rule 3 then encodes every process.
        let src = subsystems("      left_app : process P;\n");
        assert_eq!(
            table(&src, "N"),
            [
                row("Top.impl/left/app", "left_app", "left-app"),
                row("Top.impl/right/app", "right_app", "right-app"),
                row("Top.impl/left_app", "leftzuapp", "leftzuapp"),
            ]
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn a_plain_wit_name_equal_to_an_encoded_wit_name_makes_every_name_encoded() {
        // A root-level `leftApp` is crate `leftapp` (no clash with
        // `left_app`) but WIT `left-app`: the WIT name `left.app` encodes
        // to. The WIT clash alone triggers rule 3.
        let src = subsystems("      leftApp : process P;\n");
        assert_eq!(
            table(&src, "N"),
            [
                row("Top.impl/left/app", "left_app", "left-app"),
                row("Top.impl/right/app", "right_app", "right-app"),
                row("Top.impl/leftApp", "leftzcapp", "leftzcapp"),
            ]
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn a_thread_outside_a_process_keeps_its_parents_plain_name() {
        // Threads in a thread group go under the group's plain name, as
        // before #543; a thread in a process under the process's crate.
        let src = "package G\npublic\n  thread T\n  end T;\n  thread group TG\n  end TG;\n\
                   thread group implementation TG.impl\n    subcomponents\n      t : thread T;\n\
                   end TG.impl;\n  process P\n  end P;\n  process implementation P.impl\n\
                   subcomponents\n      Grp : thread group TG.impl;\n      u : thread T;\n\
                   end P.impl;\n  system Top\n  end Top;\n  system implementation Top.impl\n\
                   subcomponents\n      app : process P.impl;\n  end Top.impl;\nend G;\n";
        let inst = instantiate(src, "G");
        let at = |path: &str| {
            inst.all_components()
                .map(|(i, _)| i)
                .find(|&i| inst.component_instance_path(i) == path)
                .unwrap_or_else(|| panic!("no {path}"))
        };
        assert_eq!(
            parent_crate_dir(&inst, at("Top.impl/app/Grp/t"), "orphan"),
            "grp"
        );
        assert_eq!(
            parent_crate_dir(&inst, at("Top.impl/app/u"), "orphan"),
            "app"
        );
        let root = at("Top.impl");
        assert_eq!(parent_crate_dir(&inst, root, "orphan"), "orphan");
        // A non-process asks for its plain names.
        assert_eq!(
            process_names(&inst, at("Top.impl/app/Grp")).crate_name,
            "grp"
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn names_that_collide_only_in_crate_switch_alone() {
        // `fooBar` and `foobar` are one crate (`foobar`) but different WIT
        // names (`foo-bar`, `foobar`). The crate clash alone sends both to
        // their instance-path names; the bystander `sensor_proc` keeps its
        // plain names (rule 3 would make it `sensorzuproc`).
        let src = "package K\npublic\n  process P\n  end P;\n  system Top\n  end Top;\n\
                   system implementation Top.impl\n    subcomponents\n      fooBar : process P;\n\
                   foobar : process P;\n      sensor_proc : process P;\n  end Top.impl;\nend K;\n";
        assert_eq!(
            table(src, "K"),
            [
                row("Top.impl/fooBar", "foozcbar", "foozcbar"),
                row("Top.impl/foobar", "foobar", "foobar"),
                row("Top.impl/sensor_proc", "sensor_proc", "sensor-proc"),
            ]
        );
    }

    // rivet: verifies REQ-CODEGEN-OUTPUT-PATHS-UNIQUE-001
    #[test]
    fn names_that_collide_only_in_wit_are_both_encoded() {
        // `fooBar` and `foo_bar` are different crates (`foobar`, `foo_bar`)
        // but one WIT name, `foo-bar`. Both switch, crate and WIT together.
        let src = "package W\npublic\n  process P\n  end P;\n  system Top\n  end Top;\n\
                   system implementation Top.impl\n    subcomponents\n      fooBar : process P;\n\
                   foo_bar : process P;\n  end Top.impl;\nend W;\n";
        assert_eq!(
            table(src, "W"),
            [
                row("Top.impl/fooBar", "foozcbar", "foozcbar"),
                row("Top.impl/foo_bar", "foozubar", "foozubar"),
            ]
        );
    }
}
