//! The binary as a user runs it.

use std::process::{Command, Output};

fn graff(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graff"))
        .args(args)
        .output()
        .expect("graff runs")
}

#[test]
fn version_names_the_binary_and_the_crate_version() {
    let out = graff(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("graff {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_bare_call_shows_the_usage_and_fails() {
    let out = graff(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage: graff"));
}

#[test]
fn an_unknown_flag_is_refused() {
    let out = graff(&["--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--no-such-flag"));
}
