//! Prints each call, reference and use item graff finds in a worktree's
//! files, each type a path goes through, in Nix each path written and each
//! option a binding sets, and in Markdown each link and each code span that
//! names code, with what it ties each to and the definition each is in, one
//! JSON object per line. The files are read from stdin, one path per line,
//! from the worktree's root, the folder named, those in no language graff
//! reads left out as the index leaves them; a package's library is named as
//! its Cargo.toml says.
//!
//!     git ls-files 'src/*.rs' | cargo run --release --example resolve -- PACKAGE
//!
//! With --broken, every edge graff resolves is tied instead to the definition
//! after the right one in its file: a resolver wrong on purpose, for a check
//! of resolution to tell from the real one. With --markdown, only the edges
//! Markdown files write, which graff resolves apart from every other
//! language's: what the Markdown checks compare. On stderr, how long
//! resolution took, extraction aside.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use graff::extract;
use graff::lang::Language;
use graff::resolve::{self, Definition, File, Library, Resolution};
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .expect("usage: resolve PACKAGE [--broken] [--markdown]"),
    );
    let flags: Vec<String> = args.collect();
    let broken = flags.iter().any(|f| f == "--broken");
    let markdown = flags.iter().any(|f| f == "--markdown");
    let named: Vec<String> = io::stdin()
        .lock()
        .lines()
        .map_while(Result::ok)
        .filter(|p| !p.is_empty())
        .collect();
    // A file in no language graff reads is left out, as the index leaves it.
    let (mut paths, mut languages, mut extractions) = (Vec::new(), Vec::new(), Vec::new());
    for path in &named {
        if Language::of(path, b"").is_none() && !Language::told_by_head(path) {
            continue;
        }
        let source = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let Some(language) = Language::of(path, &source) else {
            continue;
        };
        paths.push(path.clone());
        languages.push(language);
        extractions.push(extract::extract(language, &source));
    }
    let left = named.len() - paths.len();
    let instances = {
        let files: Vec<File> = paths
            .iter()
            .zip(&extractions)
            .zip(&languages)
            .map(|((path, extraction), &language)| File {
                path,
                language,
                extraction,
            })
            .collect();
        resolve::nix::instantiate(&files, &|path| std::fs::read(root.join(path)).ok())
    };
    eprintln!(
        "{} bindings made where modules call helpers, in {} files; {} arguments",
        instances.made.len(),
        instances
            .made
            .iter()
            .map(|(file, _, _)| file)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        instances.arguments.len()
    );
    let written = instances.apply(&mut extractions.iter_mut().collect::<Vec<_>>());
    let files: Vec<File> = paths
        .iter()
        .zip(&extractions)
        .zip(&languages)
        .map(|((path, extraction), &language)| File {
            path,
            language,
            extraction,
        })
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
    let edges = if markdown {
        resolve::markdown::resolve(&files)
    } else {
        resolve::resolve(&files, &libraries)
    };
    eprintln!(
        "{} files, {left} named in no language graff reads; {} edges resolved in {:.1} ms",
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
                let mut target = json!({
                    "path": files[definition.file].path,
                    "qualified": symbol.qualified,
                    "kind": symbol.kind.name(),
                    "start": symbol.start,
                    "end": symbol.end,
                });
                let at = Definition {
                    file: definition.file,
                    symbol: index,
                };
                if let Some(at) = written.get(&at) {
                    target["written"] = json!({
                        "path": files[at.file].path, "start": at.start, "end": at.end,
                    });
                }
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
