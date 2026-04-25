//! Track A close-out: end-to-end CLI integration test for IRQ-aware RTA.
//!
//! Exercises the full pipeline (parse → instance → analyze) on a model
//! using the v0.7.0 `Spar_Timing::*` property surface. The unit/fixture
//! tests in `spar-analysis/tests/fixtures/rta/` cover the algorithm at
//! the analysis-crate level; this test is the CLI-level guard that the
//! property surface flows through `spar analyze` end-to-end.

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

const MODEL: &str = "\
package Test_Track_A_IRQ
public
  with Timing_Properties;
  with Deployment_Properties;
  with Spar_Timing;

  processor M4
  end M4;

  device Sensor_IRQ
  end Sensor_IRQ;

  device implementation Sensor_IRQ.Impl
    properties
      Spar_Timing::ISR_Priority         => 100;
      Spar_Timing::ISR_Execution_Time   => 20 us .. 30 us;
      Spar_Timing::Interrupt_Latency_Bound => 10 us;
  end Sensor_IRQ.Impl;

  thread Brake_Handler
  end Brake_Handler;

  thread implementation Brake_Handler.Impl
    properties
      Dispatch_Protocol => Sporadic;
      Period            => 1 ms;
      Compute_Execution_Time => 50 us .. 200 us;
      Deadline          => 1 ms;
  end Brake_Handler.Impl;

  process P
  end P;

  process implementation P.Impl
    subcomponents
      bh: thread Brake_Handler.Impl;
  end P.Impl;

  system Top
  end Top;

  system implementation Top.Impl
    subcomponents
      cpu: processor M4;
      irq: device Sensor_IRQ.Impl;
      app: process P.Impl;
    properties
      Actual_Processor_Binding => (reference (cpu)) applies to app.bh;
  end Top.Impl;
end Test_Track_A_IRQ;
";

#[test]
fn track_a_irq_aware_rta_runs_through_cli() {
    let path = env::temp_dir().join(format!("spar_track_a_irq_{}.aadl", std::process::id()));
    fs::write(&path, MODEL).expect("write temp AADL");

    let output = spar()
        .arg("analyze")
        .arg("--root")
        .arg("Test_Track_A_IRQ::Top.Impl")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The pipeline must complete (non-zero exit only on hard parse errors,
    // which this model doesn't have). Diagnostic content can vary as the
    // analysis matures; we only guard the must-not-crash invariant + the
    // presence of *some* RTA output, since the model is RTA-relevant.
    assert!(
        !combined.contains("panicked"),
        "spar analyze panicked on Spar_Timing::* model:\n{combined}",
    );
    assert!(
        !combined.contains("internal compiler error"),
        "spar analyze internal error:\n{combined}",
    );

    // Loose check: RTA must say *something* about the handler thread
    // since it is bound to a CPU and has a deadline.
    let mentions_handler = combined.contains("Brake_Handler") || combined.contains("bh");
    assert!(
        mentions_handler,
        "spar analyze did not surface any output mentioning the handler thread:\n{combined}",
    );

    let _ = fs::remove_file(&path);
}
