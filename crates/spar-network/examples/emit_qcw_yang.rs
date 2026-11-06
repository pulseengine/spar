//! Writes the documents `GateSchedule::to_qcw_yang` emits into a directory,
//! so CI can validate spar's REAL output against the published YANG modules
//! (REQ-TSN-YANG-DEPLOYABLE-001). Usage: `emit_qcw_yang <out-dir>`.
//!
//! Two documents: a hand-built two-window schedule (the golden test's) and a
//! `synthesize_gcl` output, so the validated shape is the synthesizer's too.

use std::path::PathBuf;

use spar_network::ArrivalCurve;
use spar_network::tsn::{
    ClassDemand, ClassOfService, GateSchedule, GateWindow, PtpTime, synthesize_gcl,
};

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: emit_qcw_yang <out-dir>"),
    );
    std::fs::create_dir_all(&dir).expect("create out dir");
    let base = PtpTime {
        seconds: 1_700_000_000,
        nanoseconds: 0,
    };

    let two_window = GateSchedule {
        windows: vec![
            GateWindow {
                offset_ps: 0,
                duration_ps: 30_000,
                allowed_cos_mask: 0x04,
            },
            GateWindow {
                offset_ps: 30_000,
                duration_ps: 70_000,
                allowed_cos_mask: 0x00,
            },
        ],
        cycle_ps: 100_000,
    };

    let cos = |c| ClassOfService::new(c).expect("CoS 0..=7");
    let synthesized = synthesize_gcl(
        &[
            ClassDemand {
                cos: cos(7),
                arrival: ArrivalCurve::affine(125, 100_000_000),
                deadline_ps: 8_000_000,
            },
            ClassDemand {
                cos: cos(3),
                arrival: ArrivalCurve::affine(250, 200_000_000),
                deadline_ps: 9_000_000,
            },
        ],
        10_000_000,
        1_000_000_000,
    )
    .expect("a feasible two-class demand");

    for (name, sched) in [("two-window", &two_window), ("synthesized", &synthesized)] {
        let doc = sched
            .to_qcw_yang("eth0", base)
            .expect("a whole-ns schedule exports");
        let path = dir.join(format!("{name}.xml"));
        std::fs::write(&path, doc).expect("write document");
        println!("{}", path.display());
    }
}
