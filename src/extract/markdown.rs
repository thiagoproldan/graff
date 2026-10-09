//! Markdown, read with pulldown-cmark as GitHub reads it: CommonMark with
//! tables, strikethrough, task lists and footnotes, and YAML front matter
//! left out of the text. On the four corpora of task 16, it reads every
//! heading, link and code span cmark-gfm, GitHub's parser, reads, at the
//! same line (note 125).
//!
//! The file is a definition, and so is each heading's section, which runs
//! to the next heading of its level or a higher one. A section is named by
//! its heading's text and, as its qualified name, by the anchor GitHub
//! gives it, which is unique in the file and which links name: the text its
//! page shows, lower-cased, its punctuation and symbols gone, its spaces
//! hyphens, and a second `Usage` `usage-1`. An element's `id`, or an
//! `<a name>`, is an anchor too.
//!
//! A link to a file or an anchor is an import, `via` `link`. A path a code
//! span or the prose writes is one too, `via` `mention`: `src/store.rs`,
//! `guard.rs`, `src/store.rs:120`. A code span written as code names code
//! -- a qualified name, a call, or a name with a capital, an underscore or
//! a digit, `Storage::load`, `k3_mmw()`, `CTX_MIN_LINES` -- is a reference
//! of kind `mention`; a bare lower-case word, `kind`, is prose as often as
//! code, and is none (decision 128). Code blocks, HTML and a link's own
//! text mention nothing.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

use super::{Extraction, Import, Kind, RefKind, Reference, Symbol};

/// The extensions of the files graff reads, which a path in prose or a
/// file name alone in a code span must end with to name one.
const READ: &[&str] = &["rs", "nix", "sh", "bash", "py", "c", "h", "md", "markdown"];

/// Extensions of files graff does not read: `serve.json` names a file, not
/// code, even with no `/`.
const UNREAD: &[&str] = &[
    "json",
    "jsonl",
    "toml",
    "txt",
    "yml",
    "yaml",
    "lock",
    "log",
    "tsv",
    "csv",
    "html",
    "css",
    "js",
    "ts",
    "png",
    "svg",
    "pdf",
    "xml",
    "ini",
    "cfg",
    "conf",
    "gz",
    "zip",
    "tar",
    "so",
    "o",
    "a",
    "exe",
    "dll",
    "jpg",
    "jpeg",
    "gif",
    "webp",
    "wasm",
    "patch",
    "diff",
    "pem",
    "sqlite",
    "db",
    "bin",
    "out",
    "cpp",
    "hpp",
    "cc",
    "go",
    "java",
    "rb",
    "lua",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "mk",
    "cmake",
    "service",
    "desktop",
    "env",
    "example",
    "sample",
    "safetensors",
];

pub fn extract(source: &[u8]) -> Extraction {
    let text = String::from_utf8_lossy(source);
    let lines = Lines::new(&text);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let markdown = without_front_matter(&text);
    let mut read = Read::default();
    for (event, range) in Parser::new_ext(&markdown, options).into_offset_iter() {
        read.event(event, range, &lines);
    }
    read.finish(&lines)
}

/// The text with its YAML front matter blanked, each line and offset kept:
/// a first line `---`, a line with something on it, and the lines up to the
/// next `---` or `...`, which GitHub shows apart, as a table. pulldown-cmark's
/// own option takes such a block anywhere in a file, as it took a changelog's
/// `---` between two releases, and the headings in it.
fn without_front_matter(text: &str) -> std::borrow::Cow<'_, str> {
    let delimiter = |line: &str, ends: &[&str]| ends.contains(&line.trim_end());
    let mut lines = text.split_inclusive('\n');
    let opens = lines.next().is_some_and(|line| delimiter(line, &["---"]))
        && lines
            .next()
            .is_some_and(|line| !line.trim().is_empty() && !delimiter(line, &["---", "..."]));
    let closed = lines.position(|line| delimiter(line, &["---", "..."]));
    let (true, Some(closed)) = (opens, closed) else {
        return text.into();
    };
    let end: usize = text
        .split_inclusive('\n')
        .take(closed + 3)
        .map(str::len)
        .sum();
    let mut blanked = String::with_capacity(text.len());
    for c in text[..end].chars() {
        match c {
            '\n' | '\r' => blanked.push(c),
            // As many spaces as the character's bytes keep each offset.
            _ => blanked.extend(std::iter::repeat_n(' ', c.len_utf8())),
        }
    }
    blanked.push_str(&text[end..]);
    blanked.into()
}

/// The byte offset each line starts at, and which lines are blank.
struct Lines {
    starts: Vec<usize>,
    blank: Vec<bool>,
    /// The last line with anything on it, or 1.
    last: u32,
}

impl Lines {
    fn new(text: &str) -> Lines {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let blank: Vec<bool> = text
            .split('\n')
            .map(|line| line.trim().is_empty())
            .collect();
        let last = blank.iter().rposition(|&b| !b).map_or(1, |i| i + 1) as u32;
        Lines {
            starts,
            blank,
            last,
        }
    }

    /// The line, from 1, an offset is on.
    fn of(&self, offset: usize) -> u32 {
        self.starts.partition_point(|&start| start <= offset) as u32
    }

    /// Whether a line holds nothing but blanks.
    fn blank(&self, line: u32) -> bool {
        self.blank.get(line as usize - 1).copied().unwrap_or(true)
    }
}

/// A heading as read: its level, its line and its text.
struct Heading {
    level: u8,
    line: u32,
    /// What its page shows of it, which its anchor is made of: no image's
    /// text, no HTML, a line break kept.
    text: String,
    /// Its text with its images', which names a heading of images alone.
    alt: String,
    /// The paragraph right under it.
    doc: Option<String>,
}

/// What a pass over the events gathers, each at its line.
#[derive(Default)]
struct Read {
    headings: Vec<Heading>,
    /// Each element's `id`, and each `<a name>`.
    anchors: Vec<(u32, String)>,
    links: Vec<(u32, String)>,
    paths: Vec<(u32, String)>,
    /// A code span that names code: its last segment, and the span when it
    /// has more than one.
    names: Vec<(u32, String, Option<String>)>,
    /// The file's first paragraph.
    first: Option<String>,
    /// The heading being read.
    heading: Option<Heading>,
    /// The paragraph being read, and the heading it is the doc of, if any.
    paragraph: Option<(String, Option<usize>)>,
    /// The last heading read, while no block has opened since.
    under: Option<usize>,
    /// How deep in links and images: their text names nothing more.
    linked: usize,
    /// How deep in images, whose text the page does not show.
    image: usize,
    /// Inside a code block, whose text is no prose.
    verbatim: bool,
    /// The prose read on one line since the last event of another kind:
    /// pulldown-cmark may cut a run of text in several.
    run: Option<(u32, String)>,
}

impl Read {
    fn event(&mut self, event: Event, range: Range<usize>, lines: &Lines) {
        let line = lines.of(range.start);
        if let Event::Text(text) = &event {
            if !self.verbatim {
                self.prose(text);
                if self.linked == 0 {
                    match &mut self.run {
                        Some((at, run)) if *at == line => run.push_str(text),
                        _ => {
                            self.flush();
                            self.run = Some((line, text.to_string()));
                        }
                    }
                }
            }
            return;
        }
        self.flush();
        match event {
            Event::Start(tag) => self.start(tag, line),
            Event::End(tag) => self.end(tag),
            Event::Code(code) => {
                self.prose(&code);
                if self.linked > 0 {
                    return;
                }
                match named(&code) {
                    Named::Path(path) => self.paths.push((line, path)),
                    Named::Code { name, path } => self.names.push((line, name, path)),
                    Named::Nothing => {}
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                for id in ids(&html) {
                    self.anchors.push((line, id));
                }
            }
            Event::SoftBreak | Event::HardBreak => self.line_break(),
            _ => {}
        }
    }

    /// The paths the prose run read last writes.
    fn flush(&mut self) {
        if let Some((line, run)) = self.run.take() {
            for path in prose_paths(&run) {
                self.paths.push((line, path));
            }
        }
    }

    fn start(&mut self, tag: Tag, line: u32) {
        // What opens inside a paragraph or a heading is no block of its own.
        let inline = self.paragraph.is_some() || self.heading.is_some();
        let under = if inline { None } else { self.under.take() };
        match tag {
            Tag::Heading { level, .. } => {
                self.heading = Some(Heading {
                    level: level as u8,
                    line,
                    text: String::new(),
                    alt: String::new(),
                    doc: None,
                });
            }
            Tag::Paragraph if !inline => self.paragraph = Some((String::new(), under)),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                self.linked += 1;
                if let Some(destination) = relative(&dest_url)
                    && link_type != LinkType::Email
                {
                    self.links.push((line, destination));
                }
            }
            Tag::Image { .. } => {
                self.linked += 1;
                self.image += 1;
            }
            Tag::CodeBlock(_) => self.verbatim = true,
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                if let Some(heading) = self.heading.take() {
                    self.headings.push(heading);
                    self.under = Some(self.headings.len() - 1);
                }
            }
            TagEnd::Paragraph => {
                if let Some((text, under)) = self.paragraph.take() {
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if text.is_empty() {
                        return;
                    }
                    if let Some(h) = under {
                        self.headings[h].doc = Some(text.clone());
                    }
                    self.first.get_or_insert(text);
                }
            }
            TagEnd::Link => self.linked = self.linked.saturating_sub(1),
            TagEnd::Image => {
                self.linked = self.linked.saturating_sub(1);
                self.image = self.image.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.verbatim = false,
            _ => {}
        }
    }

    /// Text a heading or a paragraph holds, as GitHub shows it: an image
    /// shows none.
    fn prose(&mut self, text: &str) {
        if let Some(heading) = &mut self.heading {
            heading.alt.push_str(text);
            if self.image == 0 {
                heading.text.push_str(text);
            }
        } else if let Some((paragraph, _)) = &mut self.paragraph
            && self.image == 0
        {
            paragraph.push_str(text);
        }
    }

    /// A line break: in a heading's text one its anchor drops, as GitHub's
    /// page holds it; in a paragraph's, a space.
    fn line_break(&mut self) {
        if let Some(heading) = &mut self.heading {
            heading.text.push('\n');
            heading.alt.push('\n');
        } else if let Some((paragraph, _)) = &mut self.paragraph {
            paragraph.push(' ');
        }
    }

    fn finish(mut self, lines: &Lines) -> Extraction {
        self.flush();
        let mut out = Extraction::default();
        let text_last = lines.last;
        out.symbols.push(Symbol {
            name: String::new(),
            qualified: String::new(),
            kind: Kind::File,
            start: 1,
            end: text_last,
            doc: self.first.clone(),
            internal: false,
            typed: None,
        });
        // A section runs to the next heading of its level or a higher one.
        let mut sections: Vec<(u32, u32, String)> = Vec::new();
        let mut slugs = Slugs::default();
        for (i, heading) in self.headings.iter().enumerate() {
            let next = self.headings[i + 1..]
                .iter()
                .find(|h| h.level <= heading.level)
                .map_or(text_last + 1, |h| h.line);
            let mut end = (next - 1).max(heading.line);
            while end > heading.line && lines.blank(end) {
                end -= 1;
            }
            let qualified = slugs.next(&heading.text);
            sections.push((heading.line, end, qualified.clone()));
            let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
            let shown = words(&heading.text);
            out.symbols.push(Symbol {
                name: if shown.is_empty() {
                    words(&heading.alt)
                } else {
                    shown
                },
                qualified,
                kind: Kind::Section,
                start: heading.line,
                end,
                doc: heading.doc.clone(),
                internal: false,
                typed: None,
            });
        }
        let mut seen: Vec<&str> = Vec::new();
        for (line, id) in &self.anchors {
            if seen.contains(&id.as_str()) {
                continue;
            }
            seen.push(id);
            out.symbols.push(Symbol {
                name: id.clone(),
                qualified: format!("#{id}"),
                kind: Kind::Anchor,
                start: *line,
                end: *line,
                doc: None,
                internal: false,
                typed: None,
            });
        }
        // A use is in the innermost section around its line.
        let from = |line: u32| {
            sections
                .iter()
                .rev()
                .find(|(start, end, _)| *start <= line && line <= *end)
                .map(|(_, _, qualified)| qualified.clone())
        };
        let import = |line: u32, path: &str, via: &str| Import {
            path: path.to_string(),
            alias: None,
            glob: false,
            public: false,
            line,
            from: from(line),
            via: Some(via.to_string()),
        };
        for (line, destination) in &self.links {
            out.imports.push(import(*line, destination, "link"));
        }
        for (line, path) in &self.paths {
            out.imports.push(import(*line, path, "mention"));
        }
        for (line, name, path) in &self.names {
            out.references.push(Reference {
                name: name.clone(),
                path: path.clone(),
                kind: RefKind::Mention,
                line: *line,
                from: from(*line),
                local: None,
                typed: None,
            });
        }
        out
    }
}

/// The anchors GitHub gives a file's headings, in order: a second heading of
/// the same text gets `-1`, a third `-2`, skipping any taken (github-slugger).
#[derive(Default)]
struct Slugs {
    taken: HashMap<String, usize>,
}

impl Slugs {
    fn next(&mut self, heading: &str) -> String {
        let base = slug(heading);
        let mut name = base.clone();
        while self.taken.contains_key(&name) {
            let count = self.taken.get_mut(&base).expect("the base is taken first");
            *count += 1;
            name = format!("{base}-{count}");
        }
        self.taken.insert(name.clone(), 0);
        name
    }
}

/// A heading's anchor as GitHub makes it of the text its page shows,
/// spaces at its ends kept: lower-cased, with all but letters and what
/// Unicode calls alphabetic, marks, decimal digits, connectors such as `_`,
/// `-` and spaces removed, then each space a hyphen. This is
/// github-slugger's rule, whose tests hold the anchors GitHub gave.
pub fn slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter(|&c| {
            c == ' '
                || c == '-'
                || c.is_alphabetic()
                || c.general_category_group() == GeneralCategoryGroup::Mark
                || matches!(
                    c.general_category(),
                    GeneralCategory::DecimalNumber | GeneralCategory::ConnectorPunctuation
                )
        })
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// A link's destination when it names a place of the worktree: a path or a
/// fragment, not a URL with a scheme or an address.
fn relative(destination: &str) -> Option<String> {
    let destination = destination.trim();
    let scheme = destination.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    });
    if destination.is_empty() || destination == "#" || scheme || destination.starts_with("//") {
        return None;
    }
    // A query, `?plain=1`, names no other place.
    let (path, fragment) = destination
        .split_once('#')
        .map_or((destination, None), |(path, fragment)| {
            (path, Some(fragment))
        });
    let path = path.split('?').next().unwrap_or(path);
    Some(match fragment {
        Some(fragment) => format!("{path}#{fragment}"),
        None => path.to_string(),
    })
}

/// What a code span names, by how it is written.
#[derive(Debug, PartialEq)]
enum Named {
    /// A file, maybe with a `:line` or a `:Name` after it.
    Path(String),
    /// Code: its last segment, and the whole when it has more than one.
    Code {
        name: String,
        path: Option<String>,
    },
    Nothing,
}

/// A code span's text read as decision 128 has it: a path, or code written
/// as code names it -- `Storage::load`, `os.path.join`, `k3_mmw()`, a name
/// with a capital, an underscore or a digit -- or nothing: a bare lower-case
/// word, a command, a flag, a value.
fn named(span: &str) -> Named {
    let span = span.trim();
    if span.is_empty() || span.chars().any(char::is_whitespace) || span.contains("://") {
        return Named::Nothing;
    }
    if let Some(path) = path_named(span) {
        return Named::Path(path);
    }
    let bare = span.strip_prefix('$').unwrap_or(span);
    let bare = bare.strip_suffix('!').unwrap_or(bare);
    // `f()`, `f(x, y)`: a call, its arguments left out.
    let (bare, call) = match bare.find('(') {
        Some(open) if bare.ends_with(')') => (&bare[..open], true),
        Some(_) => return Named::Nothing,
        None => (bare, false),
    };
    // `::f` names a function as Doxygen's links do.
    let rooted = bare.starts_with("::");
    let bare = bare.strip_prefix("::").unwrap_or(bare);
    let segments: Vec<&str> = if bare.contains("::") {
        bare.split("::").collect()
    } else {
        bare.split('.').collect()
    };
    if segments.iter().any(|s| !identifier(s)) {
        return Named::Nothing;
    }
    let name = segments[segments.len() - 1];
    if segments.len() > 1 {
        // `serve.json` names a file graff does not read.
        if !bare.contains("::") && UNREAD.contains(&name.to_ascii_lowercase().as_str()) {
            return Named::Nothing;
        }
        return Named::Code {
            name: name.to_string(),
            path: Some(bare.to_string()),
        };
    }
    let marked = name
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if call || rooted || marked {
        Named::Code {
            name: name.to_string(),
            path: None,
        }
    } else {
        Named::Nothing
    }
}

/// An identifier as most languages write one: a letter or `_`, then
/// letters, digits and `_`.
fn identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A code span's path, with a `:line`, `:line-line` or `:Name` after it:
/// one with a `/`, relative, or a file name that ends in an extension graff
/// reads. An absolute path, `~/x` or `$HOME/x` is outside the worktree.
fn path_named(span: &str) -> Option<String> {
    let (path, after) = match span.split_once(':') {
        Some((path, after)) => (path, Some(after)),
        None => (span, None),
    };
    if !in_worktree(path) {
        return None;
    }
    let chars = |text: &str| {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_alphanumeric() || "._-/+@".contains(c))
    };
    if !chars(path) || path.contains("//") {
        return None;
    }
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase());
    let named = if path.contains('/') {
        extension.as_deref().is_none_or(|e| !UNREAD.contains(&e))
    } else {
        extension.as_deref().is_some_and(|e| READ.contains(&e))
    };
    if !named {
        return None;
    }
    match after {
        None => Some(path.to_string()),
        Some(after)
            if lines_named(after)
                || after.split("::").all(identifier)
                || after.split('.').all(identifier) =>
        {
            Some(format!("{path}:{after}"))
        }
        Some(_) => None,
    }
}

/// Whether a path may name a place of the worktree: not an absolute one,
/// `~/x`, `$HOME/x` or `<dir>/x`. A leading dot is `./`, `../` or a hidden
/// folder's, `.github/`.
fn in_worktree(path: &str) -> bool {
    if path.starts_with(['/', '~', '$', '<']) {
        return false;
    }
    match path.strip_prefix('.') {
        Some(rest) => {
            rest.starts_with('/')
                || rest.starts_with("./")
                || rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        }
        None => true,
    }
}

/// `120` or `120-140`.
fn lines_named(text: &str) -> bool {
    let mut parts = text.splitn(2, '-');
    parts.all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The paths a run of prose writes: with a `/` or as a file name alone, each
/// ending in an extension graff reads, and maybe a `:line` after it.
fn prose_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || "()[]{}<>\"'`,;|*".contains(c)) {
        let word = word.trim_end_matches(['.', ':', '!', '?']);
        if word.is_empty() || word.contains("://") || word.starts_with("www.") {
            continue;
        }
        let (path, line) = match word.split_once(':') {
            Some((path, line)) if lines_named(line) => (path, Some(line)),
            Some(_) => continue,
            None => (word, None),
        };
        // `github.com/x/y.md` is an address, written with no scheme, and
        // `security@tokio.rs` one GitHub links as an email's.
        let first = path.split('/').next().unwrap_or(path);
        let domain = path.contains('/') && first.contains('.') && first != "." && first != "..";
        let email = first
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'));
        if !in_worktree(path) || domain || email {
            continue;
        }
        let extension = path
            .rsplit('/')
            .next()
            .and_then(|file| file.rsplit_once('.'))
            .map(|(stem, extension)| (stem, extension.to_ascii_lowercase()));
        let Some((stem, extension)) = extension else {
            continue;
        };
        let file_ok = !stem.is_empty()
            && path
                .chars()
                .all(|c| c.is_alphanumeric() || "._-/+@".contains(c));
        if file_ok && READ.contains(&extension.as_str()) {
            found.push(match line {
                Some(line) => format!("{path}:{line}"),
                None => path.to_string(),
            });
        }
    }
    found
}

/// The anchors an HTML fragment names: each element's `id`, each `<a>`'s
/// `name`.
fn ids(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let element: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if element.is_empty() {
            continue;
        }
        for (key, value) in attributes(&tag[element.len()..]) {
            if (key == "id" || (key == "name" && element == "a")) && !value.is_empty() {
                found.push(value);
            }
        }
    }
    found
}

/// An HTML tag's attributes after its name, each lower-cased, with its value.
fn attributes(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if !(c.is_ascii_alphabetic()) {
            chars.next();
            continue;
        }
        let start = i;
        while chars
            .peek()
            .is_some_and(|&(_, c)| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
        {
            chars.next();
        }
        let end = chars.peek().map_or(text.len(), |&(i, _)| i);
        let key = text[start..end].to_ascii_lowercase();
        while chars.peek().is_some_and(|&(_, c)| c == ' ') {
            chars.next();
        }
        if chars.peek().is_none_or(|&(_, c)| c != '=') {
            found.push((key, String::new()));
            continue;
        }
        chars.next();
        while chars.peek().is_some_and(|&(_, c)| c == ' ') {
            chars.next();
        }
        let value = match chars.peek() {
            Some(&(i, quote @ ('"' | '\''))) => {
                chars.next();
                let close = text[i + 1..]
                    .find(quote)
                    .map_or(text.len(), |at| i + 1 + at);
                while chars.peek().is_some_and(|&(j, _)| j <= close) {
                    chars.next();
                }
                text[i + 1..close].to_string()
            }
            Some(&(i, _)) => {
                let close = text[i..]
                    .find(|c: char| c.is_whitespace() || c == '/')
                    .map_or(text.len(), |at| i + at);
                while chars.peek().is_some_and(|&(j, _)| j < close) {
                    chars.next();
                }
                text[i..close].to_string()
            }
            None => String::new(),
        };
        found.push((key, value));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(out: &Extraction) -> Vec<(&str, &str, u32, u32)> {
        out.symbols
            .iter()
            .filter(|s| s.kind == Kind::Section)
            .map(|s| (s.name.as_str(), s.qualified.as_str(), s.start, s.end))
            .collect()
    }

    fn imports<'a>(out: &'a Extraction, via: &str) -> Vec<(u32, &'a str, Option<&'a str>)> {
        out.imports
            .iter()
            .filter(|i| i.via.as_deref() == Some(via))
            .map(|i| (i.line, i.path.as_str(), i.from.as_deref()))
            .collect()
    }

    #[test]
    fn a_section_runs_to_the_next_heading_of_its_level_or_a_higher_one() {
        let out = extract(
            b"# Title\n\
              \n\
              Intro.\n\
              \n\
              ## Usage\n\
              text\n\
              ### Stable ids\n\
              more\n\
              \n\
              Setext\n\
              ------\n\
              \n\
              ```sh\n\
              # not a heading\n\
              ```\n\
              \n\
              #not a heading either\n\
              \n\
              # Last\n\
              end\n\
              \n",
        );
        assert_eq!(
            sections(&out),
            vec![
                ("Title", "title", 1, 17),
                ("Usage", "usage", 5, 8),
                ("Stable ids", "stable-ids", 7, 8),
                ("Setext", "setext", 10, 17),
                ("Last", "last", 19, 20),
            ]
        );
        let file = &out.symbols[0];
        assert_eq!((file.kind, file.start, file.end), (Kind::File, 1, 20));
        assert_eq!(file.doc.as_deref(), Some("Intro."));
    }

    #[test]
    fn front_matter_holds_no_heading() {
        let out = extract(b"---\nname: handoff\ndescription: x\n---\n\n# Handoff\n");
        assert_eq!(sections(&out), vec![("Handoff", "handoff", 6, 6)]);
        let out = extract("---\ntitle: é\n...\n# After\n".as_bytes());
        assert_eq!(sections(&out), vec![("After", "after", 4, 4)]);
        // Front matter is at a file's top alone: a changelog's rules hold
        // headings between them, and two rules in a row are no front matter.
        let out = extract(b"# Changelog\n\n---\n## [2.0.1](x) - 2025-06-09\n\n### Other\n\n---\n");
        assert_eq!(
            sections(&out),
            vec![
                ("Changelog", "changelog", 1, 8),
                ("2.0.1 - 2025-06-09", "201---2025-06-09", 4, 8),
                ("Other", "other", 6, 8),
            ]
        );
        let out = extract(b"---\n---\n# Title\n---\n");
        assert_eq!(sections(&out), vec![("Title", "title", 3, 4)]);
    }

    #[test]
    fn a_heading_s_anchor_is_the_one_github_gives_it() {
        assert_eq!(slug("Stable ids"), "stable-ids");
        assert_eq!(slug("Result, 2026-10-09"), "result-2026-10-09");
        assert_eq!(
            slug("def, a prototype and its definition"),
            "def-a-prototype-and-its-definition"
        );
        assert_eq!(
            slug("What tree-sitter-c misreads"),
            "what-tree-sitter-c-misreads"
        );
        assert_eq!(slug("A & B"), "a--b");
        assert_eq!(slug("Configuração rápida"), "configuração-rápida");
        assert_eq!(slug("😄 emoji"), "-emoji");
        assert_eq!(slug("x² and ½"), "x-and-");
        assert_eq!(slug("snake_case.rs"), "snake_casers");
        // From github-slugger's tests, which hold the anchors GitHub gave: a
        // space at an end stays, a mark and what is alphabetic stay, the
        // other spaces and numbers go.
        assert_eq!(slug(" a "), "-a-");
        assert_eq!(slug("⚠\u{fe0f} Semver"), "\u{fe0f}-semver");
        assert_eq!(slug("a\u{488} \u{591}b"), "a\u{488}-\u{591}b");
        assert_eq!(slug("Ⓔ \u{1f130}"), "ⓔ-\u{1f130}");
        assert_eq!(slug("a\u{2028}b\u{3000}c\u{200d}d"), "abcd");
        assert_eq!(slug("a\u{9f8}\u{661}"), "a\u{661}");
        assert_eq!(slug("a\u{203f}b"), "a\u{203f}b");
        // Markup is gone, its text kept; a second heading of the same text
        // gets -1, a third -2, past one a heading already holds.
        let out = extract(
            b"# The `graff` *tool*\n## Usage\n## Usage\n## Usage 1\n## Usage\n## Usage-1\n\
              ## <a id=\"1030\"></a>1030. The prime's lines\n\
              # ab [![crates.io](b.svg)](https://c) [![docs](d.svg)](https://e)\n\
              # ![Logo](logo.png)\n# &#x20;a\n",
        );
        assert_eq!(
            sections(&out)
                .iter()
                .map(|(name, qualified, _, _)| (*name, *qualified))
                .collect::<Vec<_>>(),
            vec![
                ("The graff tool", "the-graff-tool"),
                ("Usage", "usage"),
                ("Usage", "usage-1"),
                ("Usage 1", "usage-1-1"),
                ("Usage", "usage-2"),
                ("Usage-1", "usage-1-2"),
                ("1030. The prime's lines", "1030-the-primes-lines"),
                // An image shows no text: the spaces around badges stay in
                // the anchor, and a heading of an image alone is named by it.
                ("ab", "ab--"),
                ("Logo", ""),
                ("a", "-a"),
            ]
        );
        let anchors: Vec<(&str, &str, u32)> = out
            .symbols
            .iter()
            .filter(|s| s.kind == Kind::Anchor)
            .map(|s| (s.name.as_str(), s.qualified.as_str(), s.start))
            .collect();
        assert_eq!(anchors, vec![("1030", "#1030", 7)]);
    }

    #[test]
    fn an_element_s_id_and_an_a_s_name_are_anchors() {
        assert_eq!(ids(r#"<a id="x"></a>"#), vec!["x"]);
        assert_eq!(ids(r#"<a name='top'>"#), vec!["top"]);
        assert_eq!(ids(r#"<div class="c" id=plain>"#), vec!["plain"]);
        assert_eq!(ids(r#"<span name="n">"#), Vec::<String>::new());
        // GitHub shows no comment, nor an anchor inside one.
        assert_eq!(ids(r#"<!-- <a id="hidden"> -->"#), Vec::<String>::new());
        assert_eq!(
            ids(r#"</a><br/><img src="x.png" alt="id=y">"#),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_link_to_the_worktree_is_an_import_of_the_section_it_is_in() {
        let out = extract(
            b"See [the store](src/store.rs#L10) and [ref][r].\n\
              \n\
              ## Links\n\
              \n\
              - [up](../decisions.md#372), [here](#links), [q](x.md?plain=1#a)\n\
              - [site](https://x.y), <https://x.y>, [mail](mailto:a@b), <a@b.c>, [top](#)\n\
              - ![shot](media/x.png), [proto](//cdn.x/y)\n\
              \n\
              [r]: other.md#part\n",
        );
        assert_eq!(
            imports(&out, "link"),
            vec![
                (1, "src/store.rs#L10", None),
                (1, "other.md#part", None),
                (5, "../decisions.md#372", Some("links")),
                (5, "#links", Some("links")),
                (5, "x.md#a", Some("links")),
            ]
        );
    }

    #[test]
    fn a_code_span_names_code_when_written_as_code_names_it() {
        let code = |name: &str, path: Option<&str>| Named::Code {
            name: name.to_string(),
            path: path.map(String::from),
        };
        let path = |p: &str| Named::Path(p.to_string());
        for (span, want) in [
            ("Storage::load", code("load", Some("Storage::load"))),
            ("::f", code("f", None)),
            ("os.path.join", code("join", Some("os.path.join"))),
            ("packages.plugin", code("plugin", Some("packages.plugin"))),
            ("k3_mmw()", code("k3_mmw", None)),
            ("prime()", code("prime", None)),
            ("K3_ROPE_AT(s)", code("K3_ROPE_AT", None)),
            ("self.f()", code("f", Some("self.f"))),
            ("CTX_MIN_LINES", code("CTX_MIN_LINES", None)),
            ("$TERMINAL", code("TERMINAL", None)),
            ("K3KdaW", code("K3KdaW", None)),
            ("set_state", code("set_state", None)),
            ("i32", code("i32", None)),
            ("include_str!", code("include_str", None)),
            ("src/store.rs", path("src/store.rs")),
            ("src/store.rs:120", path("src/store.rs:120")),
            ("src/store.rs:120-140", path("src/store.rs:120-140")),
            (
                "src/store.rs:Storage::load",
                path("src/store.rs:Storage::load"),
            ),
            ("./scripts/k3-doctor.sh", path("./scripts/k3-doctor.sh")),
            ("bin/auto-reset", path("bin/auto-reset")),
            ("docs/data/", path("docs/data/")),
            (".github/workflows", path(".github/workflows")),
            ("guard.rs", path("guard.rs")),
            ("README.md", path("README.md")),
            ("flake.nix", path("flake.nix")),
            // Prose as often as code, a value, a command, a flag, a file
            // graff does not read, a place outside the worktree.
            ("kind", Named::Nothing),
            ("prime", Named::Nothing),
            ("ekko --mcp", Named::Nothing),
            ("--json", Named::Nothing),
            ("-p", Named::Nothing),
            ("0.27.0", Named::Nothing),
            ("1,2,3", Named::Nothing),
            ("serve.json", Named::Nothing),
            ("Cargo.toml", Named::Nothing),
            ("expert_hist.json", Named::Nothing),
            ("docs/data/x.tsv", Named::Nothing),
            ("~/.ekko.json", Named::Nothing),
            ("/proc", Named::Nothing),
            ("/clear", Named::Nothing),
            ("$XDG_STATE_HOME/ekko/told", Named::Nothing),
            ("<model_dir>/config.json", Named::Nothing),
            ("https://x.y/a.md", Named::Nothing),
            ("Vec<u8>", Named::Nothing),
            ("#[serde(flatten)]", Named::Nothing),
            ("$1", Named::Nothing),
            ("e^-5", Named::Nothing),
        ] {
            assert_eq!(named(span), want, "{span}");
        }
    }

    #[test]
    fn a_mention_is_in_the_section_around_it_and_nothing_in_code_or_a_link_is() {
        let out = extract(
            br#"Uses `Storage::load` and `kind`.

# Store

| name | what |
| ---- | ---- |
| `K3KdaW` | `src/io/k3_st.h:38` |

```rust
let x = `Hidden::name`;
```

    `Indented::code`

See [`Linked::name`](src/a.rs) and <code>`html`</code>.
"#,
        );
        let names: Vec<(u32, &str, Option<&str>, Option<&str>)> = out
            .references
            .iter()
            .map(|r| {
                (
                    r.line,
                    r.name.as_str(),
                    r.path.as_deref(),
                    r.from.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![
                (1, "load", Some("Storage::load"), None),
                (7, "K3KdaW", None, Some("store")),
            ]
        );
        assert!(out.references.iter().all(|r| r.kind == RefKind::Mention));
        assert_eq!(
            imports(&out, "mention"),
            vec![(7, "src/io/k3_st.h:38", Some("store"))]
        );
        assert_eq!(imports(&out, "link"), vec![(15, "src/a.rs", Some("store"))]);
    }

    #[test]
    fn a_path_in_the_prose_is_a_mention_when_it_names_a_file_graff_reads() {
        assert_eq!(
            prose_paths(
                "See src/agent.rs, guard.rs (and evals/paired/harness.py:43). \
                 Not I/O, macOS/arm64, e.g. this, docs/data/x.tsv, CLAUDE.md, \
                 github.com/x/y/blob/main/a.rs, https://x.y/b.md, ~/x.rs, /etc/y.sh, \
                 security@tokio.rs."
            ),
            vec![
                "src/agent.rs",
                "guard.rs",
                "evals/paired/harness.py:43",
                "CLAUDE.md"
            ]
        );
        // A run of prose cut in several pieces is read whole.
        let out = extract(b"# A\n\nRead src/my_file.rs and *not_this*.rs here.\n");
        assert_eq!(
            imports(&out, "mention"),
            vec![(3, "src/my_file.rs", Some("a"))]
        );
    }

    #[test]
    fn a_section_s_doc_is_the_paragraph_right_under_its_heading() {
        let out = extract(
            b"<!-- generated -->\n\
              # Decisions\n\
              \n\
              What was settled, and why.\n\
              \n\
              ## One\n\
              \n\
              - a list first\n\
              \n\
              Then a paragraph.\n\
              \n\
              ## Two\n\
              Soft\n\
              wrapped.\n",
        );
        let docs: Vec<(&str, Option<&str>)> = out
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.doc.as_deref()))
            .collect();
        assert_eq!(
            docs,
            vec![
                ("", Some("What was settled, and why.")),
                ("Decisions", Some("What was settled, and why.")),
                ("One", None),
                ("Two", Some("Soft wrapped.")),
            ]
        );
    }
}
