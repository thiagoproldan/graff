//! The languages graff reads, and which one a file is in.

use serde::{Deserialize, Serialize};

/// A language graff extracts, in the order they are used here (decision 34).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Nix,
    Bash,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Rust, Language::Nix, Language::Bash];

    /// The language of a file, from its path, or for a file with no extension
    /// from the shebang on its first line or, with none, what its top says
    /// (`names_shell`). None for what graff does not read.
    pub fn of(path: &str, head: &[u8]) -> Option<Language> {
        match extension(path) {
            Some("rs") => Some(Language::Rust),
            Some("nix") => Some(Language::Nix),
            Some("sh" | "bash") => Some(Language::Bash),
            Some(_) => None,
            None => match interpreter(head) {
                Some("bash" | "sh") => Some(Language::Bash),
                Some(_) => None,
                // A script that is sourced, never run, needs no shebang;
                // bash-completion's own names its mode for Emacs.
                None if !head.starts_with(b"#!") && names_shell(head) => Some(Language::Bash),
                None => None,
            },
        }
    }

    /// Whether the language of the file at `path` is told by its first line,
    /// as it is for a file with no extension: what `of` takes as `head`.
    pub fn told_by_head(path: &str) -> bool {
        extension(path).is_none()
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Nix => "nix",
            Language::Bash => "bash",
        }
    }

    /// The language `name` gives that name to.
    pub fn from_name(name: &str) -> Option<Language> {
        Language::ALL
            .into_iter()
            .find(|language| language.name() == name)
    }
}

/// A file name's extension: `rs` in `src/main.rs`, none in `bin/login`.
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map(|(_, extension)| extension)
}

/// A program's name, without the folder it is in.
fn name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The program a shebang runs the file with, by its name: `bash` for
/// `#!/bin/bash`, `#!/nix/store/..-bash-5.2/bin/bash` and
/// `#!/usr/bin/env -S bash -e`, whose program is the one env runs.
fn interpreter(head: &[u8]) -> Option<&str> {
    let line = head.strip_prefix(b"#!")?;
    let line = &line[..line.iter().position(|&b| b == b'\n').unwrap_or(line.len())];
    let mut words = std::str::from_utf8(line).ok()?.split_ascii_whitespace();
    let program = name(words.next()?);
    if program != "env" {
        return Some(program);
    }
    while let Some(word) = words.next() {
        if matches!(word, "-u" | "--unset" | "-C" | "--chdir") {
            words.next();
        } else if !word.starts_with('-') && !word.contains('=') {
            return Some(name(word));
        }
    }
    None
}

/// Whether a file with no shebang says at its top that it is for Bash or
/// sh: by the mode its first nonblank line names for Emacs,
/// `# -*- shell-script -*-` or `# -*- mode: sh; -*-`, when it is one of
/// GitHub linguist's names for the shell, or by a shellcheck directive
/// above its first command, `# shellcheck shell=bash`.
fn names_shell(head: &[u8]) -> bool {
    let lines = head
        .split(|&b| b == b'\n')
        .map(|line| std::str::from_utf8(line).unwrap_or("").trim())
        .filter(|line| !line.is_empty());
    for (n, line) in lines.enumerate() {
        if n == 0
            && emacs_mode(line).is_some_and(|mode| {
                ["sh", "shell-script", "bash"]
                    .iter()
                    .any(|shell| mode.eq_ignore_ascii_case(shell))
            })
        {
            return true;
        }
        if !line.starts_with('#') {
            return false;
        }
        if matches!(shellcheck_shell(line), Some("bash" | "sh")) {
            return true;
        }
    }
    false
}

/// The major mode an Emacs `-*- .. -*-` line names, alone, `-*- sh -*-`, or
/// among other variables, `-*- mode: sh; fill-column: 80 -*-`.
fn emacs_mode(line: &str) -> Option<&str> {
    let (_, rest) = line.split_once("-*-")?;
    let (inside, _) = rest.split_once("-*-")?;
    if !inside.contains(':') {
        return Some(inside.trim());
    }
    inside.split(';').find_map(|pair| {
        let (name, value) = pair.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("mode")
            .then_some(value.trim())
    })
}

/// The shell a shellcheck directive names: `bash` in
/// `# shellcheck disable=SC2034 shell=bash`.
fn shellcheck_shell(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix('#')?
        .trim_start()
        .strip_prefix("shellcheck")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    rest.split_ascii_whitespace()
        .find_map(|word| word.strip_prefix("shell="))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rust_file_is_rust_a_nix_file_nix_and_others_are_not_read() {
        assert_eq!(Language::of("src/main.rs", b""), Some(Language::Rust));
        assert_eq!(
            Language::of("hosts/x/default.nix", b""),
            Some(Language::Nix)
        );
        assert_eq!(Language::of("flake.nix", b""), Some(Language::Nix));
        assert_eq!(Language::of("flake.lock", b""), None);
        assert_eq!(Language::of("src/main.rs.orig", b""), None);
        assert_eq!(Language::of("Cargo.toml", b""), None);
        assert_eq!(Language::of("rs", b""), None);
    }

    #[test]
    fn a_script_is_bash_by_its_extension_or_with_none_by_its_shebang() {
        assert_eq!(Language::of("src/lib/ledger.sh", b""), Some(Language::Bash));
        assert_eq!(Language::of("x.bash", b""), Some(Language::Bash));
        for head in [
            &b"#!/usr/bin/env bash\nset -u\n"[..],
            b"#!/bin/sh",
            b"#! /bin/bash -e\n",
            b"#!/nix/store/0abc-bash-5.2p37/bin/bash\n",
            b"#!/usr/bin/env -S bash -e\n",
            b"#!/usr/bin/env -u X LC_ALL=C bash\n",
        ] {
            assert_eq!(
                Language::of("src/hooks/handoff", head),
                Some(Language::Bash),
                "{}",
                String::from_utf8_lossy(head)
            );
        }
        for head in [
            &b"#!/usr/bin/env python3\n"[..],
            b"#!/usr/bin/env\n",
            b"#!/bin/zsh\n",
            b"# bash, but no shebang\n",
            b"",
        ] {
            assert_eq!(Language::of("src/hooks/check-bash-read", head), None);
        }
        // An extension says it all: a shebang does not make a text file a script.
        assert_eq!(Language::of("notes.txt", b"#!/bin/bash\n"), None);
        assert_eq!(Language::of("notes.txt", b"# -*- shell-script -*-\n"), None);
        assert!(Language::told_by_head("src/bin/login"));
        assert!(!Language::told_by_head("src/lib/ledger.sh"));
        assert!(!Language::told_by_head(".bashrc"));
    }

    #[test]
    fn a_script_with_no_shebang_is_bash_by_its_emacs_mode_or_a_shellcheck_directive() {
        for head in [
            // bash-completion's bash_completion, which completion files call.
            &b"#                                    -*- shell-script -*-\n#\n"[..],
            b"\n# -*- mode: sh; fill-column: 80 -*-\n",
            b"# -*-Bash-*-\n",
            b"# shellcheck shell=bash\nx=1\n",
            b"# Helpers.\n#shellcheck disable=SC2034 shell=sh # sourced\n",
            b"# -*- mode: python; -*-\n# shellcheck shell=bash\n",
        ] {
            assert_eq!(
                Language::of("bash_completion", head),
                Some(Language::Bash),
                "{}",
                String::from_utf8_lossy(head)
            );
        }
        for head in [
            &b"# -*- python -*-\n"[..],
            b"# -*- mode: sh-like; -*-\n",
            // Only the first nonblank line names a mode.
            b"# Helpers.\n# -*- shell-script -*-\n",
            // A directive counts above the first command, as shellcheck's.
            b"x=1\n# shellcheck shell=bash\n",
            b"# shellcheck shell=ksh\n",
            b"# shellcheckers shell=bash\n",
            // A shebang decides, even one that names no program.
            b"#!/usr/bin/env python3\n# -*- shell-script -*-\n",
            b"#!/usr/bin/env\n# shellcheck shell=bash\n",
        ] {
            assert_eq!(
                Language::of("helpers/x", head),
                None,
                "{}",
                String::from_utf8_lossy(head)
            );
        }
    }

    #[test]
    fn each_language_is_read_back_from_its_name() {
        for language in Language::ALL {
            assert_eq!(Language::from_name(language.name()), Some(language));
        }
        assert_eq!(Language::from_name("python"), None);
    }
}
