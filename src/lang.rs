//! The languages graff reads, and which one a file is in.

use serde::{Deserialize, Serialize};

/// A language graff extracts, in the order they are used here (decision 34).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
}

impl Language {
    /// The language of a file, from its path, or for a file with no extension
    /// from the shebang on its first line. None for what graff does not read.
    pub fn of(path: &str, _head: &[u8]) -> Option<Language> {
        let name = path.rsplit('/').next().unwrap_or(path);
        match name.rsplit_once('.').map(|(_, extension)| extension) {
            Some("rs") => Some(Language::Rust),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_file_is_rust_and_others_are_not_read() {
        assert_eq!(Language::of("src/main.rs", b""), Some(Language::Rust));
        assert_eq!(Language::of("src/main.rs.orig", b""), None);
        assert_eq!(Language::of("Cargo.toml", b""), None);
        assert_eq!(Language::of("rs", b""), None);
    }
}
