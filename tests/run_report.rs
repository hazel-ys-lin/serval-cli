//! End-to-end tests for the JSON report writer (`serval run` →
//! `<report-dir>/<timestamp>.json`).

use assert_cmd::Command;
use httpmock::{Method, MockServer};
use serde_json::Value;
use std::fs;

const FX: &str = "tests/fixtures/run_no_frontmatter.feature";

fn list_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect()
}

#[test]
fn writes_report_with_schema_fields_and_summary() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let files = list_files(tmp.path());
    assert_eq!(files.len(), 1, "exactly one report file expected");

    let json: Value = serde_json::from_str(&fs::read_to_string(&files[0]).unwrap()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["summary"]["total"], 1);
    assert_eq!(json["summary"]["passed"], 1);
    assert_eq!(json["summary"]["failed"], 0);
    assert_eq!(json["target"]["endpoint"], "/health");
    assert_eq!(json["target"]["method"], "GET");
    assert!(json["started_at"].as_str().is_some());
    assert!(json["finished_at"].as_str().is_some());
    assert_eq!(json["results"].as_array().unwrap().len(), 1);
}

#[test]
fn no_report_flag_skips_file_creation() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--report-dir",
            tmp.path().to_str().unwrap(),
            "--no-report",
        ])
        .assert()
        .success();

    assert!(
        list_files(tmp.path()).is_empty(),
        "--no-report must not create files"
    );
}

#[test]
fn report_filename_is_filesystem_safe() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(Method::GET).path("/health");
        then.status(200);
    });

    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("serval")
        .unwrap()
        .args([
            "run",
            FX,
            "--base-url",
            &server.base_url(),
            "--endpoint",
            "/health",
            "--method",
            "GET",
            "--report-dir",
            tmp.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let files = list_files(tmp.path());
    let name = files[0].file_name().unwrap().to_string_lossy();
    assert!(
        !name.contains(':'),
        "filename must be filesystem-safe (no colons); got {name}"
    );
    assert!(
        name.ends_with(".json"),
        "filename should end with .json; got {name}"
    );
}
