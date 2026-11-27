//! Mode transition triggers written as `subcomponent.port` (AS5506 §12),
//! #670. Before the fix the lowering split `launcher.profile_loaded` into
//! two triggers, `launcher` and `profile_loaded`, and the transition was
//! reported dead and its triggers unmatched.

use std::process::Command;

/// `trigger` is spliced into `loading -[ <trigger> ]-> preflight`.
fn model(trigger: &str) -> String {
    format!(
        "package Trig
public
  thread Launcher
    features
      profile_loaded : out event port;
      ack : in event port;
      level : out data port;
  end Launcher;
  thread implementation Launcher.impl
  end Launcher.impl;
  process Session
  end Session;
  process implementation Session.impl
    subcomponents
      launcher : thread Launcher.impl;
    modes
      loading : initial mode;
      preflight : mode;
      loading -[ {trigger} ]-> preflight;
  end Session.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      s : process Session.impl;
  end Top.impl;
end Trig;
"
    )
}

fn write(tag: &str, trigger: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("spar_trig_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("trig.aadl");
    std::fs::write(&path, model(trigger)).unwrap();
    path
}

/// The mode_* diagnostics of `spar analyze` as "analysis: message" lines.
fn mode_diags(tag: &str, trigger: &str) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", "Trig::Top.impl", "--format", "json"])
        .arg(write(tag, trigger))
        .output()
        .expect("run spar");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["analysis"].as_str().unwrap().starts_with("mode"))
        .filter(|d| d["severity"] != "info")
        .map(|d| {
            format!(
                "{}: {}",
                d["analysis"].as_str().unwrap(),
                d["message"].as_str().unwrap()
            )
        })
        .collect()
}

fn modes_text(tag: &str, trigger: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["modes", "--root", "Trig::Top.impl"])
        .arg(write(tag, trigger))
        .output()
        .expect("run spar");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn subcomponent_out_event_port_is_a_live_trigger() {
    assert_eq!(
        mode_diags("ok", "launcher.profile_loaded"),
        Vec::<String>::new()
    );
    let m = modes_text("ok_m", "launcher.profile_loaded");
    assert!(!m.contains("Dead transitions"), "{m}");
}

#[test]
fn identifiers_compare_case_insensitively() {
    assert_eq!(
        mode_diags("case", "Launcher.Profile_Loaded"),
        Vec::<String>::new()
    );
}

#[test]
fn unknown_port_of_a_subcomponent_is_one_warning_naming_the_path() {
    let d = mode_diags("noport", "launcher.nope");
    assert!(
        d.iter().any(|m| m.starts_with("mode_check: ")
            && m.contains(
                "trigger 'launcher.nope' does not match an outgoing port of a subcomponent"
            )),
        "{d:?}"
    );
    assert!(!d.iter().any(|m| m.contains("trigger 'launcher'")), "{d:?}");
    let m = modes_text("noport_m", "launcher.nope");
    assert!(m.contains("trigger 'launcher.nope' not connected"), "{m}");
}

#[test]
fn unknown_subcomponent_is_a_warning() {
    let d = mode_diags("nosub", "nobody.profile_loaded");
    assert!(
        d.iter()
            .any(|m| m.contains("trigger 'nobody.profile_loaded' does not match")),
        "{d:?}"
    );
}

/// An incoming port of the subcomponent cannot raise the event.
#[test]
fn in_port_of_a_subcomponent_is_not_a_trigger() {
    let d = mode_diags("inport", "launcher.ack");
    assert!(
        d.iter()
            .any(|m| m.contains("trigger 'launcher.ack' does not match")),
        "{d:?}"
    );
}

/// mode_rules checks the kind of the port a dotted trigger resolves to.
#[test]
fn data_port_of_a_subcomponent_gets_the_kind_warning() {
    let d = mode_diags("kind", "launcher.level");
    assert!(
        d.iter().any(|m| m.starts_with("mode_rules: ")
            && m.contains("trigger 'launcher.level' is a")
            && m.contains("should be an event port")),
        "{d:?}"
    );
    assert!(!d.iter().any(|m| m.starts_with("mode_check: ")), "{d:?}");
}

/// A comma list keeps each trigger whole: only the undeclared `go` is dead.
#[test]
fn trigger_list_mixes_dotted_and_local_names() {
    let m = modes_text("list", "launcher.profile_loaded, go");
    assert!(m.contains("trigger 'go' not connected"), "{m}");
    assert!(!m.contains("trigger 'launcher"), "{m}");
}
