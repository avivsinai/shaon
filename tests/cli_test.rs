use assert_cmd::Command;
use predicates::prelude::*;

fn shaon() -> Command {
    Command::cargo_bin("shaon").unwrap()
}

#[test]
fn test_help() {
    shaon()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("shaon"));
}

#[test]
fn test_version() {
    shaon().arg("--version").assert().success();
}

#[test]
fn test_unknown_command() {
    shaon().arg("nonexistent").assert().failure();
}
