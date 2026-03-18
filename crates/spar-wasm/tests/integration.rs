//! End-to-end integration tests for spar-wasm.
//!
//! These tests validate the full AADL -> SVG pipeline without a browser
//! by parsing real AADL source, instantiating the model, building the
//! graph, and rendering SVG, then asserting on the SVG content.

use spar_wasm::render_aadl;

const FLIGHT_CONTROL: &str = r#"
package FlightControl
public
  system Controller
    features
      sensorIn: in data port;
      cmdOut: out data port;
  end Controller;

  system implementation Controller.Basic
    subcomponents
      nav: process NavProcess;
      actuator: process ActuatorProcess;
    connections
      c1: port sensorIn -> nav.input;
      c2: port nav.output -> actuator.cmd;
      c3: port actuator.response -> cmdOut;
  end Controller.Basic;

  process NavProcess
    features
      input: in data port;
      output: out data port;
  end NavProcess;

  process ActuatorProcess
    features
      cmd: in data port;
      response: out data port;
  end ActuatorProcess;
end FlightControl;
"#;

#[test]
fn e2e_renders_valid_html() {
    let html = render_aadl(FLIGHT_CONTROL, "FlightControl::Controller.Basic", &[]).unwrap();

    // Valid HTML structure (now interactive HTML, not bare SVG)
    assert!(html.contains("<!DOCTYPE html>"), "should be HTML document");
    assert!(html.contains("<svg"), "should contain SVG");
    assert!(html.contains("</svg>"), "should close SVG");
    assert!(
        html.contains("<script>"),
        "should have interactivity script"
    );

    // Contains expected components
    assert!(
        html.contains("AADL-FlightControl"),
        "should contain AADL component IDs"
    );

    // Has style and defs
    assert!(html.contains("<defs>"));
    assert!(html.contains("<style>"));
    assert!(html.contains("arrowhead"));
}

#[test]
fn e2e_nodes_have_category_classes() {
    let html = render_aadl(FLIGHT_CONTROL, "FlightControl::Controller.Basic", &[]).unwrap();

    // Root is a system
    assert!(html.contains("type-system"));
    // Subcomponents are processes
    assert!(html.contains("type-process"));
}

#[test]
fn e2e_highlight_changes_stroke() {
    let plain = render_aadl(FLIGHT_CONTROL, "FlightControl::Controller.Basic", &[]).unwrap();
    let highlighted = render_aadl(
        FLIGHT_CONTROL,
        "FlightControl::Controller.Basic",
        &["AADL-FlightControl-nav".into()],
    )
    .unwrap();

    // Highlighted version should have the orange stroke
    assert!(highlighted.contains("#ff6600"));
    // Both should be valid SVG
    assert!(plain.contains("<svg"));
    assert!(highlighted.contains("<svg"));
}

#[test]
fn e2e_has_edges() {
    let html = render_aadl(FLIGHT_CONTROL, "FlightControl::Controller.Basic", &[]).unwrap();

    // Should have edge elements
    assert!(
        html.contains("class=\"edge\"") || html.contains("class=\"edges\""),
        "should have edge elements"
    );
    assert!(html.contains("<path"), "should have edge paths");
}

#[test]
fn e2e_invalid_root_returns_error() {
    let result = render_aadl(FLIGHT_CONTROL, "FlightControl::Nonexistent.Impl", &[]);
    assert!(result.is_err());
}

#[test]
fn e2e_empty_source_returns_error() {
    let result = render_aadl("", "Pkg::S.I", &[]);
    assert!(result.is_err());
}

#[test]
fn e2e_svg_write_to_file() {
    // This test writes SVG to a file for manual inspection.
    // Run with: cargo test -p spar-wasm --test integration e2e_svg_write_to_file -- --nocapture
    let svg = render_aadl(FLIGHT_CONTROL, "FlightControl::Controller.Basic", &[]).unwrap();

    let out_dir = std::env::temp_dir().join("spar-wasm-test");
    std::fs::create_dir_all(&out_dir).ok();
    let path = out_dir.join("flight-control.svg");
    std::fs::write(&path, &svg).unwrap();
    println!("SVG written to: {}", path.display());

    // Basic structural validation
    let line_count = svg.lines().count();
    assert!(
        line_count > 10,
        "SVG should have substantial content, got {} lines",
        line_count
    );
}
