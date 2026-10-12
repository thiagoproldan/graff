//! Tells this build's extractions apart from another's at one VERSION.
//!
//! A release, which the flake's package builds with GRAFF_RELEASE set, keeps
//! its extractions under VERSION alone, so that rebuilding it reads nothing
//! again. Any other build keeps them under VERSION and a digest of the
//! sources it is built from, uncommitted edits included, as mypy's dev builds
//! key their cache by the git revision: a branch that changes an extractor
//! without a VERSION of its own, or takes one main later takes for other
//! code, then never serves its extractions to main's build, nor main's to
//! it, through the one cache they share.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=GRAFF_RELEASE");
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let mut files = vec![
        PathBuf::from("Cargo.toml"),
        PathBuf::from("Cargo.lock"),
        PathBuf::from("build.rs"),
    ];
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    // A folder, which cargo scans whole for a change.
    println!("cargo:rerun-if-changed=src");
    walk(&root, Path::new("src"), &mut files);
    files.sort();

    let release = env::var_os("GRAFF_RELEASE").is_some_and(|value| !value.is_empty());
    let source = if release {
        String::new()
    } else {
        let mut sha = sha1_smol::Sha1::new();
        for file in &files {
            let bytes = fs::read(root.join(file)).expect("a file of the build's sources");
            sha.update(file.to_string_lossy().as_bytes());
            sha.update(b"\0");
            sha.update(&(bytes.len() as u64).to_le_bytes());
            sha.update(&bytes);
        }
        sha.digest().to_string()[..16].to_string()
    };
    println!("cargo:rustc-env=GRAFF_SOURCE={source}");
}

/// Every file under `folder`, relative to `root`.
fn walk(root: &Path, folder: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root.join(folder)).expect("a folder of the build's sources") {
        let entry = entry.expect("an entry of a folder of the build's sources");
        let path = folder.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            walk(root, &path, files);
        } else {
            files.push(path);
        }
    }
}
