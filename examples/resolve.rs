//! Prints each call, reference and use item graff finds in a worktree's
//! files, each type a path goes through, and in Nix each path written and
//! each option a binding sets, with what it ties each to and the definition
//! each is in, one JSON object per line. The files are read from stdin, one
//! path per line, from the worktree's root, the folder named; a package's
//! library is named as its Cargo.toml says.
//!
//!     git ls-files 'src/*.rs' | cargo run --release --example resolve -- PACKAGE
//!
//! With --broken, every edge graff resolves is tied instead to the definition
//! after the right one in its file: a resolver wrong on purpose, for a check
//! of resolution to tell from the real one. On stderr, how long resolution
//! took, extraction aside.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use graff::extract::{self, Extraction};
use graff::lang::Language;
use graff::resolve::{self, File, Library, Resolution};
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("usage: resolve PACKAGE [--broken]"));
    let broken = args.next().as_deref() == Some("--broken");
    let paths: Vec<String> = io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .filter(|p| !p.is_empty())
        .collect();
    let extractions: Vec<Extraction> = paths
        .iter()
        .map(|path| {
            let source = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
            let language = Language::of(path, &source)
                .unwrap_or_else(|| panic!("{path}: in no language graff reads"));
            extract::extract(language, &source)
        })
        .collect();
    let files: Vec<File> = paths
        .iter()
        .zip(&extractions)
        .map(|(path, extraction)| File { path, extraction })
        .collect();
    let names: Vec<(&str, String)> = paths
        .iter()
        .filter_map(|path| path.strip_suffix("src/lib.rs"))
        .filter_map(|package| {
            let manifest = std::fs::read_to_string(root.join(package).join("Cargo.toml")).ok()?;
            Some((
                package.trim_end_matches('/'),
                resolve::library_name(&manifest)?,
            ))
        })
        .collect();
    let libraries: Vec<Library> = names
        .iter()
        .map(|(package, name)| Library { package, name })
        .collect();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let started = Instant::now();
    let edges = resolve::resolve(&files, &libraries);
    eprintln!(
        "{} files, {} edges resolved in {:.1} ms",
        files.len(),
        edges.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    for edge in edges {
        let (resolution, rule, candidates, target) = match &edge.resolution {
            Resolution::Resolved(definition, rule) => {
                let symbols = &files[definition.file].extraction.symbols;
                let index = if broken {
                    (definition.symbol + 1) % symbols.len()
                } else {
                    definition.symbol
                };
                let symbol = &symbols[index];
                let target = json!({
                    "path": files[definition.file].path,
                    "qualified": symbol.qualified,
                    "kind": symbol.kind.name(),
                    "start": symbol.start,
                    "end": symbol.end,
                });
                ("resolved", Some(rule.name()), 1, Some(target))
            }
            Resolution::Ambiguous(candidates) => ("ambiguous", None, candidates.len(), None),
            Resolution::External => ("external", None, 0, None),
        };
        let line = json!({
            "path": files[edge.file].path,
            "line": edge.line,
            "name": edge.name,
            "written": edge.path,
            "use": edge.used.name(),
            "from": edge.from,
            "resolution": resolution,
            "rule": rule,
            "candidates": candidates,
            "target": target,
        });
        writeln!(out, "{line}").expect("stdout takes the line");
    }
}
