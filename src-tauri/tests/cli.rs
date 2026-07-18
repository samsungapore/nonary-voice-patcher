use std::{fs, process::Command};

use serde_json::Value;

const CLI: &str = env!("CARGO_BIN_EXE_nonary-voice-patcher-cli");

#[test]
fn help_exposes_the_integration_commands() {
    let output = Command::new(CLI).arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("inspect"));
    assert!(stdout.contains("apply"));
    assert!(stdout.contains("reset"));
    assert!(stdout.contains("--resources-dir"));
    assert!(stdout.contains("--json"));
}

#[test]
fn json_error_is_machine_readable_and_same_input_output_exits_four() {
    let temporary = tempfile::tempdir().unwrap();
    let rom = temporary.path().join("same.nds");
    fs::write(&rom, vec![0xff; 0x200]).unwrap();

    let output = Command::new(CLI)
        .arg("--json")
        .arg("reset")
        .arg(&rom)
        .arg(&rom)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["schemaVersion"], 1);
    assert_eq!(stdout["ok"], false);
    assert_eq!(stdout["command"], "reset");
    assert_eq!(stdout["error"]["code"], "input_error");
    assert_eq!(stdout["error"]["exitCode"], 4);
}

#[test]
fn json_usage_error_keeps_stdout_machine_readable_and_stderr_jsonl_clean() {
    let output = Command::new(CLI)
        .args([
            "--json",
            "apply",
            "input.nds",
            "output.nds",
            "--language",
            "fr",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stdout: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["schemaVersion"], 1);
    assert_eq!(stdout["ok"], false);
    assert_eq!(stdout["command"], "cli");
    assert_eq!(stdout["error"]["code"], "usage_error");
    assert_eq!(stdout["error"]["exitCode"], 2);

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
}
