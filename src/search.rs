//! Lexical search: a worktree's definitions ranked by BM25 against a few
//! words. Each definition is a document of its name, the rest of its
//! qualified name, its file's path and its doc, read as words: identifiers
//! split at underscores and where their case says a word starts, each also
//! kept whole, and the plural `s` of each taken off.
//!
//! The parts weigh as Zoekt weighs a file's (Sourcegraph's BM25F, 2025): a
//! word in the name counts five times, anywhere else once, and a
//! document's length is its count of words, unweighted. Test code keeps
//! half its score.

/// BM25's saturation of a word's count, and how much a document's length
/// tempers it: Lucene's defaults, and Zoekt's.
const K1: f64 = 1.2;
const B: f64 = 0.75;

/// How many times a word in a definition's name counts: Zoekt's boost of a
/// match in a symbol or a file's name.
const NAMED: f64 = 5.0;

/// What a definition in test code keeps of its score: Zoekt ranks test code
/// below the rest, and a test's name, a sentence of the words its subject's
/// own name has, would outrank its subject (evals/search: on kmux's and
/// nixdk's commits, a change found first went from 27 and 32% to 30 and
/// 45%; a fifth of the score did no better).
const TESTED: f64 = 0.5;

/// A definition as search reads it.
pub struct Document<'a> {
    /// What the definition is called: a function's name, a heading's text; a
    /// file's path.
    pub name: &'a str,
    /// What its qualified name adds: `Storage::` of `Storage::load`.
    pub qualifier: &'a str,
    /// Its file's path, without the last extension.
    pub path: &'a str,
    pub doc: Option<&'a str>,
    /// Whether it is test code: in a test's file, or in a `tests` module.
    pub test: bool,
}

/// The documents that hold a word of the query, best first, each with its
/// score; ties in the documents' order.
pub fn rank(query: &str, documents: &[Document]) -> Vec<(usize, f64)> {
    let mut asked: Vec<String> = Vec::new();
    for word in words(query) {
        if !asked.contains(&word) {
            asked.push(word);
        }
    }
    if asked.is_empty() || documents.is_empty() {
        return Vec::new();
    }
    // Each document's length, and how many times each word asked counts in
    // those that hold one. A file's documents come one after another, so its
    // path's words are read once.
    let mut lengths = Vec::with_capacity(documents.len());
    let mut counts: Vec<(usize, Vec<f64>)> = Vec::new();
    let mut holding = vec![0usize; asked.len()];
    let read = |text: &str, weight: f64, length: &mut usize, count: &mut [f64]| {
        each_word(text, |word| {
            *length += 1;
            if let Some(a) = asked.iter().position(|asked| asked == word) {
                count[a] += weight;
            }
        });
    };
    let mut path: Option<(&str, usize, Vec<f64>)> = None;
    for (d, document) in documents.iter().enumerate() {
        if path.as_ref().is_none_or(|(p, _, _)| *p != document.path) {
            let (mut length, mut count) = (0, vec![0.0; asked.len()]);
            read(document.path, 1.0, &mut length, &mut count);
            path = Some((document.path, length, count));
        }
        let (_, mut length, mut count) = path.clone().unwrap_or_default();
        read(document.name, NAMED, &mut length, &mut count);
        read(document.qualifier, 1.0, &mut length, &mut count);
        read(
            document.doc.unwrap_or_default(),
            1.0,
            &mut length,
            &mut count,
        );
        lengths.push(length);
        if count.iter().any(|&c| c > 0.0) {
            for (a, &c) in count.iter().enumerate() {
                if c > 0.0 {
                    holding[a] += 1;
                }
            }
            counts.push((d, count));
        }
    }
    let n = documents.len() as f64;
    let average = (lengths.iter().sum::<usize>() as f64 / n).max(1.0);
    // Lucene's idf, above zero however common the word.
    let idf: Vec<f64> = holding
        .iter()
        .map(|&h| (1.0 + (n - h as f64 + 0.5) / (h as f64 + 0.5)).ln())
        .collect();
    let mut ranked: Vec<(usize, f64)> = counts
        .into_iter()
        .map(|(d, count)| {
            let norm = K1 * (1.0 - B + B * lengths[d] as f64 / average);
            let score = count
                .iter()
                .zip(&idf)
                .filter(|(c, _)| **c > 0.0)
                .map(|(c, idf)| idf * c * (K1 + 1.0) / (c + norm))
                .sum::<f64>();
            let score = if documents[d].test {
                score * TESTED
            } else {
                score
            };
            (d, score)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked
}

/// The words of a text, lower-cased: each identifier, a run of letters,
/// digits and underscores, split at its underscores and where its case says
/// a word starts -- `parse_map`, `parseMap`, `HTTPResponse`, `Utf8Error` --
/// each part with its plural `s` taken off, then the identifier whole when
/// it splits, `parsemap`, so that a name written as it is matches better
/// than its words apart.
pub fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    each_word(text, |word| out.push(word.to_string()));
    out
}

/// Calls `found` with each word of a text, as `words` gives them, written
/// in buffers of its own rather than a string each.
fn each_word(text: &str, mut found: impl FnMut(&str)) {
    let (mut word, mut whole) = (String::new(), String::new());
    for identifier in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        whole.clear();
        let mut parts = 0;
        for run in identifier.split('_') {
            each_part(run, |part| {
                parts += 1;
                word.clear();
                word.extend(part.chars().flat_map(char::to_lowercase));
                whole.push_str(&word);
                stem(&mut word);
                found(&word);
            });
        }
        if parts > 1 {
            stem(&mut whole);
            found(&whole);
        }
    }
}

/// Calls `found` with each part of a run of letters and digits, cut where a
/// word starts: at an upper-case letter after a lower-case one or a digit,
/// `parse|Map`, `Utf8|Error`, and at the last of several upper-case ones
/// when a lower-case one follows, `HTTP|Response`. A digit stays with the
/// letters before it: `utf8`, `k3`.
fn each_part(run: &str, mut found: impl FnMut(&str)) {
    let mut start = 0;
    let mut before: Option<char> = None;
    for (at, c) in run.char_indices() {
        let after = run[at + c.len_utf8()..].chars().next();
        let starts = c.is_uppercase()
            && before.is_some_and(|before| {
                before.is_lowercase()
                    || before.is_numeric()
                    || (before.is_uppercase() && after.is_some_and(char::is_lowercase))
            });
        if starts {
            found(&run[start..at]);
            start = at;
        }
        before = Some(c);
    }
    if start < run.len() {
        found(&run[start..]);
    }
}

/// Takes a word's plural `s` off, as Lucene's EnglishMinimalStemmer does
/// (Harman's S-stemmer, 1991): `queries` to `query`, `files` to `file`,
/// but `class`, `status` and `goes` as they are.
fn stem(word: &mut String) {
    let b = word.as_bytes();
    let n = b.len();
    if n < 3 || b[n - 1] != b's' || !word.is_ascii() {
        return;
    }
    match b[n - 2] {
        b'u' | b's' => {}
        b'e' if n > 3 && b[n - 3] == b'i' && b[n - 4] != b'a' && b[n - 4] != b'e' => {
            word.truncate(n - 3);
            word.push('y');
        }
        b'e' if matches!(b[n - 3], b'i' | b'a' | b'o' | b'e') => {}
        _ => word.truncate(n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document<'a>(name: &'a str, path: &'a str, doc: Option<&'a str>) -> Document<'a> {
        Document {
            name,
            qualifier: "",
            path,
            doc,
            test: false,
        }
    }

    #[test]
    fn an_identifier_splits_where_its_case_or_an_underscore_says() {
        assert_eq!(words("parseMap"), ["parse", "map", "parsemap"]);
        assert_eq!(words("parse_map"), ["parse", "map", "parsemap"]);
        assert_eq!(words("parse maps"), ["parse", "map"]);
        assert_eq!(words("HTTPResponse"), ["http", "response", "httpresponse"]);
        assert_eq!(words("Utf8Error"), ["utf8", "error", "utf8error"]);
        assert_eq!(words("k3_mmw"), ["k3", "mmw", "k3mmw"]);
        assert_eq!(words("__init__"), ["init"]);
        assert_eq!(words("MAX_ENTRIES"), ["max", "entry", "maxentry"]);
        assert_eq!(
            words("src/store.rs:Storage::load"),
            ["src", "store", "rs", "storage", "load"]
        );
        assert_eq!(
            words("services.openssh.enable"),
            ["service", "openssh", "enable"]
        );
        assert_eq!(words("--no-such-flag"), ["no", "such", "flag"]);
        assert_eq!(words("Ação"), ["ação"]);
        assert!(words(" :: _ ").is_empty());
    }

    #[test]
    fn a_plural_s_comes_off_as_lucene_s_minimal_stemmer_takes_it() {
        for (word, stemmed) in [
            ("queries", "query"),
            ("files", "file"),
            ("parses", "parse"),
            ("loads", "load"),
            ("keys", "key"),
            ("its", "it"),
            ("class", "class"),
            ("status", "status"),
            ("goes", "goes"),
            ("as", "as"),
            ("is", "is"),
        ] {
            let mut word = word.to_string();
            stem(&mut word);
            assert_eq!(word, stemmed);
        }
    }

    #[test]
    fn a_word_in_the_name_outranks_one_in_the_doc() {
        let documents = [
            document("open", "src/a", Some("Loads it.")),
            document(
                "load",
                "src/a",
                Some("Reads the store from the folder it is given."),
            ),
            document("close", "src/a", None),
        ];
        let ranked = rank("load", &documents);
        assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 0]);
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn a_rare_word_outweighs_a_common_one() {
        let documents = [
            document("store", "src/a", None),
            document("store_item", "src/a", None),
            document("store_ledger", "src/a", None),
            document("ledger", "src/a", None),
        ];
        let ranked = rank("store ledger", &documents);
        assert_eq!(ranked[0].0, 2);
        let at = |d: usize| ranked.iter().position(|r| r.0 == d);
        assert!(at(3) < at(0), "{ranked:?}");
    }

    #[test]
    fn a_name_written_whole_outranks_its_words_apart() {
        let documents = [
            document("map_parse", "src/a", None),
            document("parse_map", "src/b", None),
            document("parseMap", "src/c", None),
        ];
        let ranked = rank("parseMap", &documents);
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[2].0, 0);
        assert!(ranked[1].1 > ranked[2].1, "{ranked:?}");
    }

    #[test]
    fn the_path_and_the_doc_count_too() {
        let documents = [
            document("open", "src/store", None),
            document("open", "src/ledger", Some("Opens the store.")),
            document("open", "src/item", None),
        ];
        let ranked = rank("store", &documents);
        assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn test_code_comes_after_what_it_tests() {
        let documents = [
            Document {
                test: true,
                ..document("open_store_twice", "tests/cli", None)
            },
            document("open", "src/store", None),
        ];
        let ranked = rank("open store", &documents);
        assert_eq!(ranked.iter().map(|r| r.0).collect::<Vec<_>>(), [1, 0]);
    }

    #[test]
    fn nothing_matches_no_word_or_no_document() {
        let documents = [document("open", "src/a", None)];
        assert!(rank("close", &documents).is_empty());
        assert!(rank(" :: ", &documents).is_empty());
        assert!(rank("open", &[]).is_empty());
    }
}
