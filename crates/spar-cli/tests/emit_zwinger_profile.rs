//! `spar emit --format zwinger-profile` against zwinger's reference mesh
//! (REQ-EMIT-ZWINGER-PROFILE-001, seam pulseengine/zwinger#17).
//!
//! The fixtures under `tests/fixtures/zwinger_profile/` are copied verbatim
//! from pulseengine/zwinger `architecture/`: the `Zwinger_Profile` property
//! set, the `Zwinger` package, the reference mesh, and zwinger's expected
//! output for each session. The comparison is semantic, as on zwinger's
//! side: both documents are parsed with rivet-yaml into a copy of zwinger's
//! `Profile` shape, restricted to `name` and `network`, with
//! `deny_unknown_fields`. An extra or misspelt key, or a value outside
//! zwinger's enums, fails to parse.

use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    name: String,
    network: Network,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Network {
    mode: Mode,
    allow: Vec<String>,
    allow_local_binding: bool,
    unix_sockets: UnixSockets,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    None,
    Unfiltered,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UnixSockets {
    Confined,
    Unfiltered,
}

const ROOT: &str = "Zwinger_Reference_Mesh::Mesh.TwoNodes";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/zwinger_profile")
        .join(name)
}

fn run(root: &str, session: &str, files: &[PathBuf]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["emit", "--format", "zwinger-profile", "--root", root])
        .args(["--session", session])
        .args(files)
        .output()
        .expect("run spar")
}

/// The reference mesh with `reference_mesh.aadl` rewritten by `edit`.
fn mesh(tag: &str, edit: impl Fn(String) -> String) -> Vec<PathBuf> {
    let src = std::fs::read_to_string(fixture("reference_mesh.aadl")).unwrap();
    let edited = edit(src.clone());
    assert_ne!(edited, src, "{tag}: the edit changed nothing");
    let dir = std::env::temp_dir().join(format!("spar_zwp_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reference_mesh.aadl");
    std::fs::write(&path, edited).unwrap();
    vec![
        fixture("zwinger_profile.aadl"),
        fixture("zwinger.aadl"),
        path,
    ]
}

fn files() -> Vec<PathBuf> {
    vec![
        fixture("zwinger_profile.aadl"),
        fixture("zwinger.aadl"),
        fixture("reference_mesh.aadl"),
    ]
}

fn parse(yaml: &str) -> Profile {
    rivet_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{e}\n{yaml}"))
}

fn ok(out: std::process::Output) -> Profile {
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse(&String::from_utf8(out.stdout).unwrap())
}

fn err(out: std::process::Output) -> String {
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn sessions_match_zwingers_expected_profiles() {
    for (session, expected) in [
        ("agent.session", "expected/agent.yaml"),
        ("devices.session", "expected/devices.yaml"),
    ] {
        let want = parse(&std::fs::read_to_string(fixture(expected)).unwrap());
        assert_eq!(ok(run(ROOT, session, &files())), want, "{session}");
    }
}

/// The values zwinger's expected files pin, stated directly, so the golden
/// comparison above cannot pass on two equally wrong documents.
#[test]
fn agent_allow_is_the_sorted_egress_hosts() {
    let p = ok(run(ROOT, "agent.session", &files()));
    assert_eq!(p.name, "agent");
    assert_eq!(p.network.allow, ["api.anthropic.com", "github.com"]);
    assert_eq!(p.network.mode, Mode::None);
}

#[test]
fn unfiltered_and_options_map_through() {
    let f = mesh("opts", |s| {
        s.replace(
            "Zwinger_Profile::Network_Mode => confined applies to devices.session;",
            "Zwinger_Profile::Network_Mode => unfiltered applies to devices.session;\n      \
             Zwinger_Profile::Unix_Sockets => unfiltered applies to devices.session;\n      \
             Zwinger_Profile::Allow_Local_Binding => true applies to agent.session;",
        )
    });
    let d = ok(run(ROOT, "devices.session", &f));
    assert_eq!(d.network.mode, Mode::Unfiltered);
    assert_eq!(d.network.unix_sockets, UnixSockets::Unfiltered);
    assert!(!d.network.allow_local_binding);
    let a = ok(run(ROOT, "agent.session", &f));
    assert!(a.network.allow_local_binding);
    assert_eq!(a.network.unix_sockets, UnixSockets::Confined);
}

/// No Network_Mode means `none`, zwinger's fail-closed default.
#[test]
fn missing_network_mode_is_none() {
    let f = mesh("nomode", |s| {
        s.replace(
            "Zwinger_Profile::Network_Mode => confined applies to devices.session;",
            "",
        )
    });
    assert_eq!(
        ok(run(ROOT, "devices.session", &f)).network.mode,
        Mode::None
    );
}

#[test]
fn egress_without_a_host_names_the_connection() {
    let f = mesh("nohost", |s| {
        s.replace(
            "Zwinger_Profile::Host => \"github.com\" applies to forge;",
            "",
        )
    });
    let e = err(run(ROOT, "agent.session", &f));
    assert!(
        e.contains("connection g_forge: egress reaches forge, and neither it nor an ancestor has Zwinger_Profile::Host"),
        "{e}"
    );
}

#[test]
fn a_host_that_is_not_exact_is_refused() {
    for (tag, bad) in [("wild", "*.github.com"), ("scheme", "https://github.com")] {
        let f = mesh(tag, |s| s.replace("\"github.com\"", &format!("\"{bad}\"")));
        let e = err(run(ROOT, "agent.session", &f));
        assert!(
            e.contains(&format!(
                "connection g_forge: Zwinger_Profile::Host \"{bad}\" is not an exact host name"
            )),
            "{e}"
        );
    }
}

#[test]
fn unfiltered_with_egress_hosts_is_refused() {
    let f = mesh("unf_hosts", |s| {
        s.replace(
            "Zwinger_Profile::Network_Mode => confined applies to agent.session;",
            "Zwinger_Profile::Network_Mode => unfiltered applies to agent.session;",
        )
    });
    let e = err(run(ROOT, "agent.session", &f));
    assert!(
        e.contains("session agent.session: Network_Mode unfiltered allows every host"),
        "{e}"
    );
}

#[test]
fn unfiltered_with_local_binding_is_refused() {
    let f = mesh("unf_bind", |s| {
        s.replace(
            "Zwinger_Profile::Network_Mode => confined applies to devices.session;",
            "Zwinger_Profile::Network_Mode => unfiltered applies to devices.session;\n      \
             Zwinger_Profile::Allow_Local_Binding => true applies to devices.session;",
        )
    });
    let e = err(run(ROOT, "devices.session", &f));
    assert!(
        e.contains("Allow_Local_Binding is implied by Network_Mode unfiltered"),
        "{e}"
    );
}

#[test]
fn missing_profile_name_is_refused() {
    let f = mesh("noname", |s| {
        s.replace(
            "Zwinger_Profile::Profile_Name => \"devices\" applies to devices.session;",
            "",
        )
    });
    let e = err(run(ROOT, "devices.session", &f));
    assert!(
        e.contains("session devices.session: Zwinger_Profile::Profile_Name is missing"),
        "{e}"
    );
}

#[test]
fn unknown_session_path_is_refused() {
    let e = err(run(ROOT, "agent.nosuch", &files()));
    assert!(
        e.contains("--session agent.nosuch: no such subcomponent below the root"),
        "{e}"
    );
}

#[test]
fn session_is_required() {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["emit", "--format", "zwinger-profile", "--root", ROOT])
        .args(files())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs --root"));
}

/// A Host on an ancestor of the destination applies, and the nearest one
/// wins. Only connections from `egress` count: `log` reaches a component
/// with a Host, and it is not in the list.
#[test]
fn host_comes_from_the_nearest_ancestor_and_only_egress_counts() {
    let model = r#"
package Anc
public
  with Zwinger_Profile;
  process Session
    features
      egress : out data port;
      log : out data port;
  end Session;
  process implementation Session.impl
  end Session.impl;
  process Svc
    features
      req : in data port;
  end Svc;
  process implementation Svc.impl
  end Svc.impl;
  system Cloud
    features
      req : in data port;
      req2 : in data port;
  end Cloud;
  system implementation Cloud.impl
    subcomponents
      front : process Svc.impl;
      back : process Svc.impl;
    connections
      c1 : port req -> front.req;
      c2 : port req2 -> back.req;
    properties
      Zwinger_Profile::Host => "back.example.org" applies to back;
  end Cloud.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      s : process Session.impl;
      cloud : system Cloud.impl;
      logs : process Svc.impl;
    connections
      g1 : port s.egress -> cloud.req;
      g2 : port s.egress -> cloud.req2;
      l1 : port s.log -> logs.req;
    properties
      Zwinger_Profile::Profile_Name => "anc" applies to s;
      Zwinger_Profile::Host => "cloud.example.org" applies to cloud;
      Zwinger_Profile::Host => "logs.example.org" applies to logs;
  end Top.impl;
end Anc;
"#;
    let dir = std::env::temp_dir().join(format!("spar_zwp_anc_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("anc.aadl");
    std::fs::write(&path, model).unwrap();
    let p = ok(run(
        "Anc::Top.impl",
        "s",
        &[fixture("zwinger_profile.aadl"), path],
    ));
    assert_eq!(p.network.allow, ["back.example.org", "cloud.example.org"]);
}

#[test]
fn empty_profile_name_is_refused() {
    let f = mesh("emptyname", |s| {
        s.replace(
            "Zwinger_Profile::Profile_Name => \"devices\"",
            "Zwinger_Profile::Profile_Name => \"\"",
        )
    });
    let e = err(run(ROOT, "devices.session", &f));
    assert!(
        e.contains(
            "session devices.session: Zwinger_Profile::Profile_Name is not a non-empty string"
        ),
        "{e}"
    );
}

/// A trailing `--session` with no value is a usage error, not a panic.
#[test]
fn session_flag_without_value_is_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["emit", "--format", "zwinger-profile", "--session"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--session requires a value"));
}
