//! End-to-end tests for `serval env {list,show,set,remove}` and
//! `serval config {path,show}`. Uses `--config-file <tempfile>` to
//! isolate each test from the user's real `~/.serval/config.toml`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn tmp_config() -> tempfile::NamedTempFile {
    tempfile::Builder::new().suffix(".toml").tempfile().unwrap()
}

fn serval() -> Command {
    Command::cargo_bin("serval").unwrap()
}

#[test]
fn env_set_then_list_shows_the_new_env() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args([
            "env",
            "set",
            "local",
            "--base-url",
            "http://localhost:3000",
            "--config-file",
            path,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("set env `local`"));

    serval()
        .args(["env", "list", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("local"))
        .stdout(predicate::str::contains("http://localhost:3000"));
}

#[test]
fn env_set_make_default_marks_the_env_as_default() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args([
            "env",
            "set",
            "staging",
            "--base-url",
            "https://staging.example.com",
            "--make-default",
            "--config-file",
            path,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("set default_env = staging"));

    serval()
        .args(["env", "list", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("staging"))
        .stdout(predicate::str::contains("yes"));
}

#[test]
fn env_show_unknown_exits_3() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args(["env", "show", "missing", "--config-file", path])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no env named \"missing\""));
}

#[test]
fn env_remove_deletes_and_clears_default_when_pointing_to_it() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args([
            "env",
            "set",
            "local",
            "--base-url",
            "http://localhost:3000",
            "--make-default",
            "--config-file",
            path,
        ])
        .assert()
        .success();

    serval()
        .args(["env", "remove", "local", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed env `local`"));

    // After removal the env is gone and is no longer default.
    serval()
        .args(["env", "list", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("no environments configured"));
}

#[test]
fn env_list_json_shape() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args([
            "env",
            "set",
            "local",
            "--base-url",
            "http://localhost:3000",
            "--make-default",
            "--config-file",
            path,
        ])
        .assert()
        .success();

    let out = serval()
        .args(["env", "list", "--config-file", path, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "local");
    assert_eq!(arr[0]["base_url"], "http://localhost:3000");
    assert_eq!(arr[0]["is_default"], true);
}

#[test]
fn config_path_prints_the_resolved_path() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args(["config", "path", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains(path));
}

#[test]
fn config_show_prints_toml_after_set() {
    let cfg = tmp_config();
    let path = cfg.path().to_str().unwrap();

    serval()
        .args([
            "env",
            "set",
            "local",
            "--base-url",
            "http://localhost:3000",
            "--make-default",
            "--config-file",
            path,
        ])
        .assert()
        .success();

    serval()
        .args(["config", "show", "--config-file", path])
        .assert()
        .success()
        .stdout(predicate::str::contains("default_env = \"local\""))
        .stdout(predicate::str::contains("[envs.local]"))
        .stdout(predicate::str::contains(
            "base_url = \"http://localhost:3000\"",
        ));
}
