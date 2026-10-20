//! REQ-AADL-BINDING-CONSUMERS-001 (#498) — wrpc-binding and ai_ml decide
//! "different processor" by the processor each binding RESOLVES to, not by
//! its text.
//!
//! In the `*_same` models, `t1` is bound by Node.impl to `cpu` and `t2` by
//! Top.impl to `left.cpu`: ONE processor. On the published v0.46.0 binary:
//! - wrpc-binding compared the raw values and warned that connection `c1`
//!   "crosses processor boundary ... but has no Actual_Connection_Binding";
//! - ai_ml took the two spellings as two processors and accepted
//!   `Redundant_Model` as satisfied, when both models run on one processor.
//!
//! In the `*_two` models, `left.app.t2` is bound to `right.cpu`, a different
//! processor: the crossing is real, and so is the redundancy on `left`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LAT_SAME: &str = r#"package LatSame
public
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      spare : processor Cpu;
    properties
      Actual_Processor_Binding => (reference (left.cpu)) applies to left.app.t2;
  end Top.impl;
end LatSame;
"#;
const LAT_TWO: &str = r#"package LatTwo
public
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
    properties
      Actual_Processor_Binding => (reference (right.cpu)) applies to left.app.t2;
  end Top.impl;
end LatTwo;
"#;
const AI_SAME: &str = r#"property set AI_ML is
  Inference_Latency: Time_Range applies to (thread);
  Inference_Mode: enumeration (Synchronous, Batched, Streaming) applies to (thread);
  Max_Batch_Size: aadlinteger applies to (thread);
  Model_Version: aadlstring applies to (process);
  Model_Format: enumeration (ONNX, TFLite, PyTorch, TensorRT, WASM_Component) applies to (process);
  Model_Hash: aadlstring applies to (process);
  Training_Data_Ref: aadlstring applies to (process);
  Confidence_Threshold: aadlreal applies to (thread);
  OOD_Detection_Enabled: aadlboolean applies to (thread);
  OOD_Detection_Method: enumeration (Mahalanobis, Softmax_Entropy, Ensemble_Disagreement, Monitor_Network) applies to (thread);
  Fallback_Strategy: enumeration (Safe_Stop, Degraded_Mode, Previous_Output, Human_Takeover, Redundant_Model) applies to (thread);
  Fallback_Latency: Time applies to (thread);
  Drift_Detection_Enabled: aadlboolean applies to (thread);
  Drift_Detection_Window: Time applies to (thread);
  Retraining_Trigger: aadlstring applies to (process);
  Fusion_Strategy: enumeration (Early, Late, Decision_Level) applies to (process);
  Input_Modalities: list of aadlstring applies to (thread);
end AI_ML;

package AiSame
public
  with AI_ML;
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
      AI_ML::Fallback_Strategy => Redundant_Model;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      spare : processor Cpu;
    properties
      Actual_Processor_Binding => (reference (left.cpu)) applies to left.app.t2;
  end Top.impl;
end AiSame;
"#;
const AI_TWO: &str = r#"property set AI_ML is
  Inference_Latency: Time_Range applies to (thread);
  Inference_Mode: enumeration (Synchronous, Batched, Streaming) applies to (thread);
  Max_Batch_Size: aadlinteger applies to (thread);
  Model_Version: aadlstring applies to (process);
  Model_Format: enumeration (ONNX, TFLite, PyTorch, TensorRT, WASM_Component) applies to (process);
  Model_Hash: aadlstring applies to (process);
  Training_Data_Ref: aadlstring applies to (process);
  Confidence_Threshold: aadlreal applies to (thread);
  OOD_Detection_Enabled: aadlboolean applies to (thread);
  OOD_Detection_Method: enumeration (Mahalanobis, Softmax_Entropy, Ensemble_Disagreement, Monitor_Network) applies to (thread);
  Fallback_Strategy: enumeration (Safe_Stop, Degraded_Mode, Previous_Output, Human_Takeover, Redundant_Model) applies to (thread);
  Fallback_Latency: Time applies to (thread);
  Drift_Detection_Enabled: aadlboolean applies to (thread);
  Drift_Detection_Window: Time applies to (thread);
  Retraining_Trigger: aadlstring applies to (process);
  Fusion_Strategy: enumeration (Early, Late, Decision_Level) applies to (process);
  Input_Modalities: list of aadlstring applies to (thread);
end AI_ML;

package AiTwo
public
  with AI_ML;
  processor Cpu
  end Cpu;
  thread T
    features
      i : in data port;
      o : out data port;
    flows
      f : flow path i -> o;
    properties
      Compute_Execution_Time => 1 ms .. 2 ms;
      AI_ML::Fallback_Strategy => Redundant_Model;
  end T;
  process P
  end P;
  process implementation P.impl
    subcomponents
      t1 : thread T;
      t2 : thread T;
    connections
      c1 : port t1.o -> t2.i;
    flows
      e : end to end flow t1.f -> c1 -> t2.f;
  end P.impl;
  system Node
  end Node;
  system implementation Node.impl
    subcomponents
      cpu : processor Cpu;
      app : process P.impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.t1;
  end Node.impl;
  system Top
  end Top;
  system implementation Top.impl
    subcomponents
      left : system Node.impl;
      right : system Node.impl;
    properties
      Actual_Processor_Binding => (reference (right.cpu)) applies to left.app.t2;
  end Top.impl;
end AiTwo;
"#;

fn analyze(root: &str, name: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("spar-bind-wrpc-ai-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join(name);
    fs::write(&path, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args(["analyze", "--root", root, path.to_str().unwrap()])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        text.contains("Instantiated "),
        "the model must instantiate: {text}"
    );
    text
}

fn count(text: &str, needle: &str, path: &str) -> usize {
    text.lines()
        .filter(|l| l.contains(needle) && l.contains(path))
        .count()
}

const WRPC: &str = "wrpc-binding: connection 'c1'";
const REDUNDANCY: &str = "uses Redundant_Model fallback but no sibling";

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn wrpc_one_processor_named_two_ways_is_no_crossing() {
    assert_eq!(
        count(&analyze("LatSame::Top.impl", "ls.aadl", LAT_SAME), WRPC, ""),
        0
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn wrpc_two_processors_still_need_a_bus_binding() {
    assert_eq!(
        count(&analyze("LatTwo::Top.impl", "lt.aadl", LAT_TWO), WRPC, ""),
        1
    );
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn ai_redundancy_on_one_processor_is_not_redundancy() {
    let text = analyze("AiSame::Top.impl", "as.aadl", AI_SAME);
    assert_eq!(count(&text, REDUNDANCY, "Top.impl/left/app/"), 2, "{text}");
}

// rivet: partially-verifies REQ-AADL-BINDING-CONSUMERS-001
#[test]
fn ai_redundancy_on_two_processors_is_redundancy() {
    // Negative control on `left`, where t1 and t2 run on left/cpu and
    // right/cpu. (The `right` node's t2 has no binding and is warned about.)
    let text = analyze("AiTwo::Top.impl", "at.aadl", AI_TWO);
    assert_eq!(count(&text, REDUNDANCY, "Top.impl/left/app/"), 0, "{text}");
}
