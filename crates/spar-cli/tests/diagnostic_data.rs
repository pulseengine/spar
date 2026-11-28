//! `spar analyze --format json` puts structured values on the diagnostics
//! of emv2_propagation, wctt, mode_reachability and mode_check (#674), so a
//! gate can read a number or a name instead of matching `message`.
//!
//! Each case checks the `data` values against the message they restate, so
//! the two cannot drift apart, and checks that diagnostics without data
//! carry no `data` key at all.

use std::process::Command;

fn analyze(root: &str, files: &[std::path::PathBuf]) -> Vec<serde_json::Value> {
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, "--format", "json"])
        .args(files)
        .output()
        .expect("run spar");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    v["diagnostics"].as_array().unwrap().clone()
}

/// The text between the first `'` after `after` and the next `'`.
fn quoted<'a>(message: &'a str, after: &str) -> &'a str {
    let rest = &message[message.find(after).unwrap() + after.len()..];
    let rest = &rest[rest.find('\'').unwrap() + 1..];
    &rest[..rest.find('\'').unwrap()]
}

fn reference_mesh() -> Vec<serde_json::Value> {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zwinger_profile");
    analyze(
        "Zwinger_Reference_Mesh::Mesh.TwoNodes",
        &[
            dir.join("zwinger.aadl"),
            dir.join("zwinger_profile.aadl"),
            dir.join("reference_mesh.aadl"),
        ],
    )
}

#[test]
fn every_emv2_chain_carries_its_error_type_origin_and_downstream() {
    let diags = reference_mesh();
    let chains: Vec<_> = diags
        .iter()
        .filter(|d| d["analysis"] == "emv2_propagation")
        .filter(|d| {
            d["message"]
                .as_str()
                .unwrap()
                .starts_with("error propagation chain:")
        })
        .collect();
    assert_eq!(chains.len(), 12, "the reference mesh has 12 chains");
    for d in chains {
        let m = d["message"].as_str().unwrap();
        let data = &d["data"];
        let n: usize = m["error propagation chain: ".len()..]
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let downstream = data["downstream"].as_array().unwrap();
        assert_eq!(data["chain_length"].as_u64().unwrap() as usize, n, "{m}");
        assert_eq!(downstream.len(), n, "{m}");
        assert_eq!(data["error_type"], quoted(m, "error type"), "{m}");
        let origin = data["origin"].as_str().unwrap();
        assert!(origin.starts_with("Mesh.TwoNodes."), "{origin}");
        assert_eq!(
            origin.rsplit('.').next().unwrap(),
            quoted(m, "downstream of"),
            "{m}"
        );
        for p in downstream {
            assert!(p.as_str().unwrap().starts_with("Mesh.TwoNodes."), "{p}");
        }
    }
}

#[test]
fn the_wctt_bound_carries_stream_bound_and_hops() {
    let diags = reference_mesh();
    let bounds: Vec<_> = diags
        .iter()
        .filter(|d| d["message"].as_str().unwrap().starts_with("WcttBound:"))
        .collect();
    assert_eq!(bounds.len(), 1);
    let m = bounds[0]["message"].as_str().unwrap();
    let data = &bounds[0]["data"];
    let ps: u64 = m
        .split("WCTT ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let hops: u64 = m
        .split(" ps (")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    // The message names the stream as "conn (src → dst)"; data gives the
    // connection and both end stations as instance paths.
    let shown = quoted(m, "stream");
    let (conn, ends) = shown.split_once(" (").unwrap();
    let (src, dst) = ends.trim_end_matches(')').split_once(" → ").unwrap();
    assert_eq!(data["stream"], format!("Mesh.TwoNodes.{conn}"), "{m}");
    assert_eq!(data["source"], format!("Mesh.TwoNodes.{src}"), "{m}");
    assert_eq!(data["destination"], format!("Mesh.TwoNodes.{dst}"), "{m}");
    assert_eq!(data["bound_ps"].as_u64().unwrap(), ps, "{m}");
    assert_eq!(data["hops"].as_u64().unwrap(), hops, "{m}");
}

#[test]
fn only_the_four_analyses_add_data_and_the_rest_omit_the_key() {
    for d in reference_mesh() {
        let has = d.as_object().unwrap().contains_key("data");
        let m = d["message"].as_str().unwrap();
        let expected = (d["analysis"] == "emv2_propagation"
            && m.starts_with("error propagation chain:"))
            || m.starts_with("WcttBound:");
        assert_eq!(has, expected, "{d}");
    }
}

/// `transition` is the mode transition line inside Session.impl.
fn modal(tag: &str, transition: &str) -> Vec<serde_json::Value> {
    let model = format!(
        "package Trig
public
  thread Launcher
    features
      ack : in event port;
  end Launcher;
  thread implementation Launcher.impl
  end Launcher.impl;
  process Session
    features
      go : in event port;
  end Session;
  process implementation Session.impl
    subcomponents
      launcher : thread Launcher.impl;
    modes
      loading : initial mode;
      preflight : mode;
      {transition}
  end Session.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      s : process Session.impl;
  end Top.impl;
end Trig;
"
    );
    let dir = std::env::temp_dir().join(format!("spar_diag_data_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("trig.aadl");
    std::fs::write(&path, model).unwrap();
    let diags = analyze("Trig::Top.impl", &[path]);
    let _ = std::fs::remove_dir_all(&dir);
    diags
}

fn transition_data(diags: &[serde_json::Value], analysis: &str) -> Vec<serde_json::Value> {
    diags
        .iter()
        .filter(|d| d["analysis"] == analysis && d["severity"] != "info")
        .map(|d| d["data"].clone())
        .collect()
}

#[test]
fn a_dead_transition_names_its_endpoints_and_trigger() {
    // `go` is an in port of Session that nothing connects to.
    let diags = modal("dead", "t1 : loading -[ go ]-> preflight;");
    let data = transition_data(&diags, "mode_reachability");
    let want = serde_json::json!({
        "component": "Top.impl.s",
        "transition": "t1",
        "source": "loading",
        "destination": "preflight",
        "trigger": "go",
    });
    assert!(data.contains(&want), "{data:#?}");
}

#[test]
fn an_unmatched_trigger_and_an_undeclared_endpoint_name_the_transition() {
    let diags = modal("bad", "t2 : loading -[ nosuch ]-> ghost;");
    let data = transition_data(&diags, "mode_check");
    let trigger = serde_json::json!({
        "component": "Top.impl.s",
        "transition": "t2",
        "source": "loading",
        "destination": "ghost",
        "trigger": "nosuch",
    });
    let endpoint = serde_json::json!({
        "component": "Top.impl.s",
        "transition": "t2",
        "source": "loading",
        "destination": "ghost",
        "trigger": null,
    });
    assert!(data.contains(&trigger), "{data:#?}");
    assert!(data.contains(&endpoint), "{data:#?}");
}
