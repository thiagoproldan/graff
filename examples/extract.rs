//! Prints what graff extracts from each file, one JSON object per line,
//! `{"path": .., "extraction": ..}`, for the files named or, with none named,
//! the paths read from stdin one per line. On stderr, the first syntax error
//! of each file that has one, and at the end how many files and bytes it
//! read, in how long, how many had a syntax error, how many nest deeper than
//! the extractor reads, and the deepest syntax tree met.
//!
//!     cargo run --release --example extract -- src/main.rs
//!     git ls-files '*.rs' | cargo run --release --example extract > out.jsonl

use std::io::{self, BufRead, Write};
use std::panic;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use graff::extract;
use graff::lang::Language;

fn main() -> ExitCode {
    let named: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<String> = if named.is_empty() {
        io::stdin().lock().lines().map_while(Result::ok).collect()
    } else {
        named
    };
    let mut out = io::BufWriter::new(io::stdout().lock());
    let (mut files, mut bytes, mut errors, mut cut, mut failed) = (0, 0, 0, 0, 0);
    let mut spent = Duration::ZERO;
    let mut deepest = (0, String::new());
    for path in &paths {
        let source = match std::fs::read(path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{path}: {error}");
                failed += 1;
                continue;
            }
        };
        let Some(language) = Language::of(path, &source) else {
            eprintln!("{path}: in no language graff reads");
            failed += 1;
            continue;
        };
        let started = Instant::now();
        let extraction = match panic::catch_unwind(|| extract::extract(language, &source)) {
            Ok(extraction) => extraction,
            Err(_) => {
                eprintln!("{path}: the extractor panicked");
                failed += 1;
                continue;
            }
        };
        spent += started.elapsed();
        files += 1;
        bytes += source.len();
        errors += usize::from(extraction.syntax_error);
        cut += usize::from(extraction.too_deep);
        let (depth, error) = walk(language, &source);
        if depth > deepest.0 {
            deepest = (depth, path.clone());
        }
        if let Some((line, text)) = error {
            eprintln!("{path}:{line}: syntax error: {text}");
        }
        let line = serde_json::json!({ "path": path, "extraction": extraction });
        writeln!(out, "{line}").expect("stdout takes the line");
    }
    out.flush().expect("stdout takes the lines");
    eprintln!(
        "{files} files, {bytes} bytes, extracted in {:.1} ms; {errors} with a syntax error; {cut} cut for depth; {failed} not read; deepest tree {} levels, in {}",
        spent.as_secs_f64() * 1000.0,
        deepest.0,
        deepest.1
    );
    if failed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// How many levels the file's syntax tree has, and the first syntax error in
/// it: its line and what it holds. A C file is read as its extractor reads
/// it, with what a C compiler never reads blanked.
fn walk(language: Language, source: &[u8]) -> (usize, Option<(usize, String)>) {
    let mended;
    let source = if language == Language::C {
        mended = extract::c::mend(source);
        &mended[..]
    } else {
        source
    };
    let grammar = match language {
        Language::Rust => tree_sitter_rust::LANGUAGE,
        Language::Nix => tree_sitter_nix::LANGUAGE,
        Language::Bash => tree_sitter_bash::LANGUAGE,
        Language::Python => tree_sitter_python::LANGUAGE,
        Language::C => tree_sitter_c::LANGUAGE,
    };
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&grammar.into())
        .expect("the grammar loads");
    let tree = parser.parse(source, None).expect("a tree");
    let mut cursor = tree.walk();
    let (mut level, mut deepest, mut error) = (0, 0, None);
    loop {
        let node = cursor.node();
        if error.is_none() && (node.is_error() || node.is_missing()) {
            let text = String::from_utf8_lossy(&source[node.byte_range()]);
            let text: String = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(80)
                .collect();
            let what = if node.is_missing() {
                format!("missing {}", node.kind())
            } else {
                text
            };
            error = Some((node.start_position().row + 1, what));
        }
        if cursor.goto_first_child() {
            level += 1;
            deepest = deepest.max(level);
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return (deepest, error);
            }
            level -= 1;
        }
    }
}
