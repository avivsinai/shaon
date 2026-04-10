use assert_cmd::Command;
use predicates::prelude::*;

fn hilan() -> Command {
    Command::cargo_bin("hilan").unwrap()
}

#[test]
fn test_help() {
    hilan()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("hilan"));
}

#[test]
fn test_version() {
    hilan().arg("--version").assert().success();
}

#[test]
fn test_unknown_command() {
    hilan().arg("nonexistent").assert().failure();
}
