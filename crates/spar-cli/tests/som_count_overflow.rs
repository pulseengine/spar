//! The system-operation-mode count is the product of every modal component's
//! mode count, and on real models it overflows u64.
//! `test-data/osate2/multiModalPingPong.aadl` (root `global_sys.impl`) made a
//! debug build panic ("attempt to multiply with overflow" in
//! `SystemInstance::compute_soms`) and a release build report a WRAPPED count:
//! "system operation mode count (4137309267128563259) exceeds limit". The
//! product is now checked; an overflow reads "more than 2^64".

use std::process::Command;

// rivet: verifies REQ-HIR-SOM-COUNT-CHECKED-001
#[test]
fn an_overflowing_mode_product_is_reported_not_wrapped() {
    let model = format!(
        "{}/../../test-data/osate2/multiModalPingPong.aadl",
        env!("CARGO_MANIFEST_DIR")
    );
    let out = Command::new(env!("CARGO_BIN_EXE_spar"))
        .args([
            "analyze",
            "--root",
            "MultiModalPingPong::global_sys.impl",
            &model,
        ])
        .output()
        .expect("spar binary runs");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !text.contains("panicked"),
        "no overflow panic: {}",
        &text[..text.len().min(2000)]
    );
    assert!(
        text.contains(
            "system operation mode count (more than 2^64) exceeds limit (10000); truncated"
        ),
        "the overflow is named, not wrapped"
    );
    assert!(!text.contains("4137309267128563259"), "no wrapped count");
}
