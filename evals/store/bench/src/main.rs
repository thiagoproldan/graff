//! Task 9's measurement: what keeping graff's index fresh would cost on each
//! query, and what parsing a whole repository from cold costs, on the
//! repositories graff targets.
//!
//!     cargo run --release -- NAME=PATH...
//!
//! For each repository, every step runs RUNS times and the median is kept,
//! with the page cache warm (the files were read just before). Git runs with
//! --no-optional-locks, so that `git status` never rewrites the index of a
//! repository someone is working in. The parse is tree-sitter's alone, with
//! no extraction: the floor under a cold build.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tree_sitter::{Language, Parser};

const RUNS: usize = 15;

fn git(repo: &Path, args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut child = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("git runs");
    if let Some(bytes) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(bytes)
            .expect("git reads its input");
    }
    let out = child.wait_with_output().expect("git ends");
    assert!(
        out.status.success(),
        "git {args:?} failed in {}",
        repo.display()
    );
    out.stdout
}

/// The language graff would parse a file as: by its extension, or for a file
/// with none, by its shebang.
fn language(path: &str, head: &[u8]) -> Option<(&'static str, Language)> {
    let extension = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let shebang = head.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let shebang = std::str::from_utf8(shebang).unwrap_or("");
    match extension {
        "rs" => Some(("rust", tree_sitter_rust::LANGUAGE.into())),
        "nix" => Some(("nix", tree_sitter_nix::LANGUAGE.into())),
        "sh" | "bash" => Some(("bash", tree_sitter_bash::LANGUAGE.into())),
        "py" => Some(("python", tree_sitter_python::LANGUAGE.into())),
        "c" | "h" => Some(("c", tree_sitter_c::LANGUAGE.into())),
        "md" => Some(("markdown", tree_sitter_md::LANGUAGE.into())),
        "" if shebang.starts_with("#!")
            && (shebang.contains("bash")
                || shebang.ends_with("/sh")
                || shebang.contains("/sh ")) =>
        {
            Some(("bash", tree_sitter_bash::LANGUAGE.into()))
        }
        "" if shebang.starts_with("#!") && shebang.contains("python") => {
            Some(("python", tree_sitter_python::LANGUAGE.into()))
        }
        _ => None,
    }
}

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1000.0
}

fn timed<T>(mut step: impl FnMut() -> T) -> (f64, T) {
    let mut times = Vec::with_capacity(RUNS);
    let mut last = None;
    for _ in 0..RUNS {
        let started = Instant::now();
        let value = step();
        times.push(started.elapsed());
        last = Some(value);
    }
    (median(times), last.expect("ran"))
}

fn versions() -> String {
    let lock = include_str!("../Cargo.lock");
    let mut found = Vec::new();
    let mut name = "";
    for line in lock.lines() {
        if let Some(n) = line.strip_prefix("name = ") {
            name = n.trim_matches('"');
        } else if let Some(v) = line.strip_prefix("version = ")
            && name.starts_with("tree-sitter")
        {
            found.push(format!("{name} {}", v.trim_matches('"')));
        }
    }
    found.join(", ")
}

fn main() {
    let repos: Vec<(String, String)> = std::env::args()
        .skip(1)
        .map(|arg| {
            let (name, path) = arg.split_once('=').expect("NAME=PATH");
            (name.to_string(), path.to_string())
        })
        .collect();
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .unwrap_or_default();
    let git_version = String::from_utf8_lossy(
        &Command::new("git")
            .arg("--version")
            .output()
            .expect("git")
            .stdout,
    )
    .trim()
    .to_string();
    println!("# The freshness check and a cold parse (task 9)\n");
    println!(
        "- machine: {cpu}, {} threads; one thread measured",
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0)
    );
    println!("- {git_version}, with --no-optional-locks");
    println!("- {}", versions());
    println!("- each figure the median of {RUNS} runs in ms, page cache warm\n");
    println!(
        "| Repository | Tracked | ls-files -s | status, tracked | status, untracked too | stat all | hash all | Parsed | Bytes | read | parse |"
    );
    println!(
        "| ---------- | ------- | ----------- | --------------- | --------------------- | -------- | -------- | ------ | ----- | ---- | ----- |"
    );
    let mut by_language: BTreeMap<String, (usize, usize, f64)> = BTreeMap::new();
    for (name, path) in &repos {
        let repo = Path::new(path);
        let listed = git(repo, &["ls-files", "-z"], None);
        let files: Vec<String> = listed
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        let (ls_stage, _) = timed(|| git(repo, &["ls-files", "-s", "-z"], None));
        let (status_tracked, _) = timed(|| {
            git(
                repo,
                &["status", "--porcelain=v2", "-z", "--untracked-files=no"],
                None,
            )
        });
        let (status_all, _) = timed(|| {
            git(
                repo,
                &["status", "--porcelain=v2", "-z", "--untracked-files=normal"],
                None,
            )
        });
        let (stat_all, _) = timed(|| {
            files
                .iter()
                .filter_map(|f| std::fs::symlink_metadata(repo.join(f)).ok())
                .map(|m| m.len())
                .sum::<u64>()
        });
        let paths: Vec<u8> = files
            .iter()
            .filter(|f| repo.join(f).is_file())
            .flat_map(|f| format!("{f}\n").into_bytes())
            .collect();
        let (hash_all, _) = timed(|| git(repo, &["hash-object", "--stdin-paths"], Some(&paths)));
        let sources: Vec<(String, Vec<u8>)> = files
            .iter()
            .filter_map(|f| std::fs::read(repo.join(f)).ok().map(|b| (f.clone(), b)))
            .collect();
        let parsed: Vec<(&str, &'static str, Language, &[u8])> = sources
            .iter()
            .filter_map(|(f, bytes)| {
                language(f, &bytes[..bytes.len().min(200)])
                    .map(|(lang, grammar)| (f.as_str(), lang, grammar, bytes.as_slice()))
            })
            .collect();
        let (read_ms, _) = timed(|| {
            parsed
                .iter()
                .filter_map(|(f, ..)| std::fs::read(repo.join(f)).ok())
                .map(|b| b.len())
                .sum::<usize>()
        });
        let mut parsers: BTreeMap<&str, Parser> = BTreeMap::new();
        for (_, lang, grammar, _) in &parsed {
            parsers.entry(lang).or_insert_with(|| {
                let mut parser = Parser::new();
                parser.set_language(grammar).expect("the grammar loads");
                parser
            });
        }
        let (parse_ms, errors) = timed(|| {
            parsed
                .iter()
                .filter(|(_, lang, _, bytes)| {
                    let tree = parsers
                        .get_mut(lang)
                        .expect("parser")
                        .parse(bytes, None)
                        .expect("a tree");
                    tree.root_node().has_error()
                })
                .count()
        });
        for (_, lang, _, bytes) in &parsed {
            let entry = by_language.entry(lang.to_string()).or_default();
            entry.0 += 1;
            entry.1 += bytes.len();
        }
        for lang in parsers.keys().copied().collect::<Vec<_>>() {
            let ours: Vec<&[u8]> = parsed.iter().filter(|p| p.1 == lang).map(|p| p.3).collect();
            let (ms, _) = timed(|| {
                let parser = parsers.get_mut(lang).expect("parser");
                ours.iter()
                    .map(|b| {
                        parser
                            .parse(b, None)
                            .expect("a tree")
                            .root_node()
                            .child_count() as usize
                    })
                    .sum::<usize>()
            });
            by_language.get_mut(lang).expect("counted").2 += ms;
        }
        let bytes: usize = parsed.iter().map(|p| p.3.len()).sum();
        println!(
            "| {name} | {} | {ls_stage:.1} | {status_tracked:.1} | {status_all:.1} | {stat_all:.2} | {hash_all:.1} | {} ({errors} with a syntax error) | {} | {read_ms:.2} | {parse_ms:.1} |",
            files.len(),
            parsed.len(),
            bytes
        );
    }
    println!("\nThe parse by language, over all repositories:\n");
    println!("| Language | Files | Bytes | parse, ms | MB/s |");
    println!("| -------- | ----- | ----- | --------- | ---- |");
    for (lang, (files, bytes, ms)) in &by_language {
        println!(
            "| {lang} | {files} | {bytes} | {ms:.1} | {:.0} |",
            *bytes as f64 / 1e6 / (ms / 1000.0)
        );
    }
}
