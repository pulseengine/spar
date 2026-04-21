use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// Regression test for GitHub issues #125/#126/#127 side-note:
/// `spar parse` must print 1-based `line:col` in error messages, not a raw
/// byte offset.
#[test]
fn parse_error_prints_line_and_col() {
    // AADL with a deliberate syntax error on line 3 (`let` is not valid AADL).
    let src = "package Broken\n\
public\n\
let x;\n\
end Broken;\n";

    let path = env::temp_dir().join(format!(
        "spar_parse_error_{}_{}.aadl",
        std::process::id(),
        line!()
    ));
    fs::write(&path, src).expect("write temp AADL");

    let output = spar()
        .arg("parse")
        .arg(&path)
        .output()
        .expect("failed to run spar");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Error must be reported on line 3 (where `let x;` is), with a column.
    // Format: "<path>:<line>:<col>: <msg>"
    let expected_prefix = format!("{}:3:", path.display());
    assert!(
        stderr.contains(&expected_prefix),
        "expected stderr to contain {:?} (line:col for line-3 error); got:\n{}",
        expected_prefix,
        stderr
    );

    // Guard against the pre-fix regression where a raw byte offset (>= file len)
    // was printed as the line number.
    assert!(
        !stderr.contains(":20:") && !stderr.contains(":30:") && !stderr.contains(":40:"),
        "stderr looks like it still contains a raw byte offset (file is ~38 bytes):\n{}",
        stderr
    );

    let _ = fs::remove_file(&path);
}
