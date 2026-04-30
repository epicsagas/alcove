//! Integration test for `alcove bench` CLI command.
//!
//! Verifies the benchmark CLI builds and responds correctly.
//! Pure logic tests (precision, recall, formatting) live in `src/bench.rs` unit tests.
//!
//! Run: `cargo test --test bench`

use std::process::Command;

fn alcove_bin() -> String {
    format!("{}/debug/alcove", std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_string()))
}

#[test]
fn bench_help_succeeds() {
    let output = Command::new(alcove_bin())
        .arg("bench")
        .arg("--help")
        .output()
        .expect("failed to run alcove bench --help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "alcove bench --help should succeed");
    assert!(stdout.contains("--metrics"), "help should mention --metrics");
    assert!(stdout.contains("--output"), "help should mention --output");
    assert!(stdout.contains("--queries"), "help should mention --queries");
    assert!(stdout.contains("--scope"), "help should mention --scope");
}

#[test]
fn bench_without_docs_root_errors() {
    // Run with HOME set to a temp dir so there's no ~/.alcove/config.toml
    let output = Command::new(alcove_bin())
        .env("HOME", "/tmp/alcove-bench-test-nohome")
        .arg("bench")
        .output()
        .expect("failed to run alcove bench");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "bench without docs should fail");
    assert!(
        stderr.contains("setup") || stderr.contains("not found") || stderr.contains("not configured"),
        "error should mention setup or missing config, got: {stderr}"
    );
}

#[test]
fn bench_with_custom_queries_path_errors_when_missing() {
    // With no docs root AND a custom (missing) queries path, the command fails.
    // The docs root check runs first, so the error is about setup, not the queries file.
    let output = Command::new(alcove_bin())
        .env("HOME", "/tmp/alcove-bench-test-nohome")
        .arg("bench")
        .arg("--queries")
        .arg("/tmp/nonexistent_ground_truth.toml")
        .output()
        .expect("failed to run alcove bench");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("setup") || stderr.contains("not found") || stderr.contains("not configured"),
        "should report config error, got: {stderr}"
    );
}
