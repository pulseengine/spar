//! Integration tests for `spar emit --format constants` and
//! `spar emit --format linker` — the hardware-target artifact path
//! (REQ-CODEGEN-TARGET-ARTIFACTS-001, #338).

use std::env;
use std::fs;
use std::process::Command;

fn spar() -> Command {
    Command::new(env!("CARGO_BIN_EXE_spar"))
}

/// A small STM32-shaped hardware target: two `memory` regions and one
/// memory-mapped `device` peripheral.
const MODEL: &str = "\
package Target_Board
public
  memory Flash_Region
  end Flash_Region;

  memory Sram_Region
  end Sram_Region;

  device Uart
  end Uart;

  system Board
  end Board;

  system implementation Board.Impl
    subcomponents
      flash: memory Flash_Region {
        Base_Address => 16#08000000#;
        Memory_Size => 1024 KByte;
      };
      sram: memory Sram_Region {
        Base_Address => 16#20000000#;
        Byte_Count => 131072;
      };
      uart1: device Uart {
        Base_Address => 16#40011000#;
      };
  end Board.Impl;
end Target_Board;
";

fn write_model(tag: &str) -> std::path::PathBuf {
    let path = env::temp_dir().join(format!(
        "spar_emit_target_{}_{}.aadl",
        std::process::id(),
        tag
    ));
    fs::write(&path, MODEL).expect("write temp AADL");
    path
}

#[test]
fn emit_constants_produces_module() {
    let model = write_model("constants");
    let out = spar()
        .args([
            "emit",
            "--format",
            "constants",
            "--root",
            "Target_Board::Board.Impl",
            model.to_str().unwrap(),
        ])
        .output()
        .expect("run spar emit");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("pub const FLASH_BASE: usize = 0x08000000;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("pub const FLASH_SIZE: usize = 0x00100000;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("pub const SRAM_BASE: usize = 0x20000000;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("pub const UART1_BASE: usize = 0x40011000;"),
        "{stdout}"
    );
    let _ = fs::remove_file(&model);
}

#[test]
fn emit_linker_produces_memory_block() {
    let model = write_model("linker");
    let out = spar()
        .args([
            "emit",
            "--format",
            "linker",
            "--root",
            "Target_Board::Board.Impl",
            model.to_str().unwrap(),
        ])
        .output()
        .expect("run spar emit");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("MEMORY"), "{stdout}");
    assert!(
        stdout.contains("FLASH : ORIGIN = 0x08000000, LENGTH = 1048576"),
        "{stdout}"
    );
    assert!(
        stdout.contains("SRAM : ORIGIN = 0x20000000, LENGTH = 131072"),
        "{stdout}"
    );
    let _ = fs::remove_file(&model);
}

#[test]
fn emit_constants_non_hardware_model_exits_nonzero() {
    let path = env::temp_dir().join(format!(
        "spar_emit_target_empty_{}.aadl",
        std::process::id()
    ));
    fs::write(
        &path,
        "package P\npublic\n system S\n end S;\n system implementation S.Impl\n end S.Impl;\nend P;\n",
    )
    .unwrap();
    let out = spar()
        .args([
            "emit",
            "--format",
            "constants",
            "--root",
            "P::S.Impl",
            path.to_str().unwrap(),
        ])
        .output()
        .expect("run spar emit");
    assert!(!out.status.success(), "should fail on a non-hardware model");
    let _ = fs::remove_file(&path);
}
