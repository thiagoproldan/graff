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
//!
//! A query answers only when its best match holds enough of its words
//! (`answer`): otherwise it finds nothing, rather than the definitions that
//! share a common word with it.

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

/// Words too common to tell a definition that answers from one that does
/// not: Lucene's English stop words, and the words a question is asked in.
/// They rank as any other word, but weigh nothing in whether the best
/// match answers.
const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "no", "not", "of", "on", "or", "such", "that", "the", "their", "then", "there", "these",
    "they", "this", "to", "was", "will", "with", "what", "where", "which", "who", "whom", "whose",
    "why", "how", "when", "do", "does", "did", "can", "could", "i", "we", "you", "my", "our",
    "your", "from", "has", "have", "had", "should", "would",
];

/// What a query found: the documents that answer it, best first, each with
/// its score; none when the best of those that hold a word of it holds too
/// little of it (`answer`).
pub struct Answer {
    pub ranked: Vec<(usize, f64)>,
    /// How many documents hold a word of the query, whether they answer or
    /// not.
    pub holding: usize,
    /// When none answers though some hold a word: the words the best holds,
    /// those that weigh in first.
    pub best_holds: Vec<String>,
    /// When none answers: the words that weigh in that no document holds.
    pub absent: Vec<String>,
}

/// The documents that hold a word of the query, best first, each with its
/// score; ties in the documents' order.
pub fn rank(query: &str, documents: &[Document]) -> Vec<(usize, f64)> {
    scored(query, documents)
        .ranked
        .into_iter()
        .map(|(d, score, _)| (d, score))
        .collect()
}

/// The documents that answer a query, ranked as `rank` ranks them, or none
/// when the best of them holds too little of it: it must hold two of the
/// query's words, or words that weigh, summed by idf, as much as one that a
/// document alone holds. A query of one word needs only that word, and one
/// of a few common words, all of them: as Zoekt and Sourcegraph match every
/// word, and Elasticsearch's minimum_should_match counts how many must
/// match. Stop words count for neither, and a word no document holds weighs
/// as the rarest. A match on one common word of a question is how BM25
/// answers what the code does not hold, and two words of it in one
/// definition by chance are rarer than one; a rare word alone still answers,
/// as a name the question asks for does. Raw BM25 scores are not comparable
/// across queries (Lucene's wiki, ScoresAsPercentages), so the bar is the
/// query's own. On evals/search's probes it said nothing on 34 and 39 of 60
/// subjects asked of the other repository, where it never had, and cost the
/// MRR of those asked of their own 1% on kmux and 8% on nixdk.
pub fn answer(query: &str, documents: &[Document]) -> Answer {
    let scored = scored(query, documents);
    let holding = scored.ranked.len();
    let Some((_, _, held)) = scored.ranked.first() else {
        return Answer {
            ranked: Vec::new(),
            holding,
            best_holds: Vec::new(),
            absent: Vec::new(),
        };
    };
    let mut weighs: Vec<bool> = scored.asked.iter().map(|word| !stop(word)).collect();
    if !weighs.contains(&true) {
        weighs = vec![true; scored.asked.len()];
    }
    let n = documents.len() as f64;
    let alone = (1.0 + (n - 0.5) / 1.5).ln();
    let all: f64 = (0..scored.asked.len())
        .filter(|&a| weighs[a])
        .map(|a| scored.idf[a])
        .sum();
    let got: f64 = (0..scored.asked.len())
        .filter(|&a| weighs[a] && held[a])
        .map(|a| scored.idf[a])
        .sum();
    let two = (0..scored.asked.len())
        .filter(|&a| weighs[a] && held[a])
        .count()
        >= weighs.iter().filter(|&&w| w).count().min(2);
    if two || got >= alone.min(all) - 1e-9 {
        return Answer {
            ranked: scored.ranked.into_iter().map(|(d, s, _)| (d, s)).collect(),
            holding,
            best_holds: Vec::new(),
            absent: Vec::new(),
        };
    }
    let words = |keep: &dyn Fn(usize) -> bool| -> Vec<String> {
        (0..scored.asked.len())
            .filter(|&a| keep(a))
            .map(|a| scored.asked[a].clone())
            .collect()
    };
    let mut best_holds = words(&|a| weighs[a] && held[a]);
    if best_holds.is_empty() {
        best_holds = words(&|a| held[a]);
    }
    Answer {
        ranked: Vec::new(),
        holding,
        best_holds,
        absent: words(&|a| weighs[a] && scored.holding[a] == 0),
    }
}

/// Whether a word, as `words` gives it, is a stop word.
fn stop(word: &str) -> bool {
    STOP.iter().any(|s| {
        let mut s = s.to_string();
        stem(&mut s);
        s == word
    })
}

/// A query's distinct words, each with its idf and how many documents hold
/// it, and the documents that hold one, best first: each with its score and
/// which of the words it holds.
struct Scored {
    asked: Vec<String>,
    idf: Vec<f64>,
    holding: Vec<usize>,
    ranked: Vec<(usize, f64, Vec<bool>)>,
}

fn scored(query: &str, documents: &[Document]) -> Scored {
    let mut asked: Vec<String> = Vec::new();
    for word in words(query) {
        if !asked.contains(&word) {
            asked.push(word);
        }
    }
    if asked.is_empty() || documents.is_empty() {
        return Scored {
            idf: vec![0.0; asked.len()],
            holding: vec![0; asked.len()],
            asked,
            ranked: Vec::new(),
        };
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
    let mut ranked: Vec<(usize, f64, Vec<bool>)> = counts
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
            (d, score, count.iter().map(|&c| c > 0.0).collect())
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Scored {
        asked,
        idf,
        holding,
        ranked,
    }
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

    /// Ten documents, the words of a question in some: `parse` in five,
    /// `config` in two, `store` in three, `ledger` in one.
    fn ten() -> Vec<Document<'static>> {
        let mut documents = vec![
            document("parse_config", "src/config", None),
            document("parse_args", "src/args", None),
            document("parse_line", "src/lines", None),
            document("parse_key", "src/keys", None),
            document("parse_value", "src/values", None),
            document("load_config", "src/config", None),
            document("ledger", "src/ledger", None),
        ];
        for name in ["open", "close", "write"] {
            documents.push(document(name, "src/store", None));
        }
        documents
    }

    #[test]
    fn a_question_answers_when_its_best_match_holds_a_word_only_it_holds() {
        let documents = ten();
        let found = answer("where is the ledger kept", &documents);
        assert_eq!(found.ranked.iter().map(|r| r.0).collect::<Vec<_>>(), [6]);
        // Two words held by several weigh as much as one held alone.
        let found = answer("how do we parse the config file", &documents);
        assert_eq!(found.ranked.first().map(|r| r.0), Some(0));
        assert_eq!(found.ranked.len(), 6);
        assert_eq!(found.holding, 6);
    }

    #[test]
    fn a_question_whose_best_match_holds_a_common_word_alone_finds_nothing() {
        let documents = ten();
        let question = "where does the pager parse its keymap";
        let found = answer(question, &documents);
        assert!(found.ranked.is_empty());
        assert_eq!(found.holding, 5);
        assert_eq!(found.best_holds, ["parse"]);
        assert_eq!(found.absent, ["pager", "keymap"]);
        // rank still gives every document that holds a word.
        assert_eq!(rank(question, &documents).len(), 5);
    }

    #[test]
    fn a_few_common_words_answer_when_the_best_holds_them_all() {
        let documents = ten();
        // One word: every document that holds it answers, however common.
        assert_eq!(answer("parse", &documents).ranked.len(), 5);
        // Two that weigh more than one held alone: enough of them, together.
        assert_eq!(answer("parse config", &documents).ranked[0].0, 0);
        let found = answer("config store", &documents);
        assert!(found.ranked.is_empty());
        assert_eq!(found.holding, 5);
        assert!(found.absent.is_empty());
    }

    #[test]
    fn two_words_of_a_question_in_one_definition_answer_however_common() {
        // `pane` and `border` in four documents each, one of them both: they
        // weigh less than a word one document alone holds.
        let documents = [
            document("pane_border", "src/draw", None),
            document("pane", "src/a", None),
            document("pane_size", "src/b", None),
            document("pane_list", "src/c", None),
            document("border", "src/d", None),
            document("border_style", "src/e", None),
            document("border_width", "src/f", None),
            document("open", "src/g", None),
            document("close", "src/h", None),
            document("write", "src/i", None),
        ];
        let found = answer("where are pane borders drawn", &documents);
        assert_eq!(found.ranked.first().map(|r| r.0), Some(0));
        assert_eq!(found.ranked.len(), 7);
        assert!(
            answer("where are panes drawn", &documents)
                .ranked
                .is_empty()
        );
    }

    #[test]
    fn stop_words_weigh_nothing_unless_they_are_all_there_is() {
        let documents = [
            document("the_store", "src/a", None),
            document("open", "src/a", Some("Opens the store.")),
            document("close", "src/a", Some("Closes it.")),
            document("write", "src/a", None),
        ];
        // `it` is held by one document alone, but answers nothing.
        let found = answer("is it the pager", &documents);
        assert!(found.ranked.is_empty());
        assert_eq!(found.absent, ["pager"]);
        assert_eq!(answer("the", &documents).ranked.len(), 2);
        for word in STOP {
            assert!(stop(&words(word)[0]), "{word}");
        }
        assert!(stop("thi") && stop("wa") && stop("ha") && !stop("store"));
    }

    #[test]
    fn nothing_matches_no_word_or_no_document() {
        let documents = [document("open", "src/a", None)];
        assert!(rank("close", &documents).is_empty());
        assert!(rank(" :: ", &documents).is_empty());
        assert!(rank("open", &[]).is_empty());
    }
}
