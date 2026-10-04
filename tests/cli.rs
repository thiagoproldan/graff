//! The binary as a user runs it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn graff(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graff"))
        .args(args)
        .output()
        .expect("graff runs")
}

/// graff with its cache in `cache`, from the folder `at`.
fn graff_in(at: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graff"))
        .args(args)
        .current_dir(at)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .expect("graff runs")
}

/// A folder of its own under the system's temporary one, emptied first.
fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("graff-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
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

#[test]
fn index_reads_a_worktree_once_and_then_only_what_changed() {
    let top = folder("index");
    let repo = top.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let cache = top.join("cache");

    let first = graff_in(&repo.join("src"), &cache, &["index"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let said = String::from_utf8_lossy(&first.stdout);
    assert!(
        said.contains("1 file, 1 in a language graff reads; 1 hashed, 1 extracted, 0 gone"),
        "{said}"
    );
    assert!(cache.join("graff/index.db").is_file());

    fs::remove_file(repo.join("src/main.rs")).unwrap();
    let second = graff_in(&repo, &cache, &["index"]);
    let said = String::from_utf8_lossy(&second.stdout);
    assert!(
        said.contains("0 files, 0 in a language graff reads; 0 hashed, 0 extracted, 1 gone"),
        "{said}"
    );
    fs::remove_dir_all(&top).unwrap();
}

#[test]
fn index_outside_a_git_worktree_fails_and_says_why() {
    let top = folder("nowhere");
    let out = graff_in(&top, &top.join("cache"), &["index"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("graff: git: "),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_dir_all(&top).unwrap();
}
