//! The binary as a user runs it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn graff(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graff"))
        .args(args)
        .output()
        .expect("graff runs")
}

/// graff with its cache in `cache`, from the folder `at`.
fn graff_in(at: &Path, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_graff"))
        .args(args)
        .current_dir(at)
        .env("XDG_CACHE_HOME", cache)
        .output()
        .expect("graff runs")
}

/// A folder of its own under the system's temporary one, emptied first.
fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("graff-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn version_names_the_binary_and_the_crate_version() {
    let out = graff(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("graff {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_bare_call_shows_the_usage_and_fails() {
    let out = graff(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Usage: graff"));
}

#[test]
fn an_unknown_flag_is_refused() {
    let out = graff(&["--no-such-flag"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--no-such-flag"));
}

#[test]
fn index_reads_a_worktree_once_and_then_only_what_changed() {
    let top = folder("index");
    let repo = top.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let cache = top.join("cache");

    let first = graff_in(&repo.join("src"), &cache, &["index"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let said = String::from_utf8_lossy(&first.stdout);
    assert!(
        said.contains("1 file, 1 in a language graff reads; 1 hashed, 1 extracted, 0 gone"),
        "{said}"
    );
    assert!(cache.join("graff/index.db").is_file());

    fs::remove_file(repo.join("src/main.rs")).unwrap();
    let second = graff_in(&repo, &cache, &["index"]);
    let said = String::from_utf8_lossy(&second.stdout);
    assert!(
        said.contains("0 files, 0 in a language graff reads; 0 hashed, 0 extracted, 1 gone"),
        "{said}"
    );
    fs::remove_dir_all(&top).unwrap();
}

#[test]
fn index_outside_a_git_worktree_fails_and_says_why() {
    let top = folder("nowhere");
    let out = graff_in(&top, &top.join("cache"), &["index"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).starts_with("graff: git: "),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::remove_dir_all(&top).unwrap();
}

const DEMO_MANIFEST: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

const DEMO_LIB: &str = r#"pub mod store;

use store::Storage;

/// Opens the store and reads one key.
pub fn run(path: &str) -> u32 {
    let storage = Storage::open(path);
    storage.fetch("key") + helper()
}

fn helper() -> u32 {
    store::MAX
}

use store::Storage as Db;

pub fn other() -> u32 {
    Db::open("x").fetch("y")
}
"#;

const DEMO_STORE: &str = r#"/// The most a store holds.
pub const MAX: u32 = 10;

/// Where values live. One per folder.
pub struct Storage {
    path: String,
}

impl Storage {
    /// Opens the store at a path.
    pub fn open(path: &str) -> Storage {
        Storage { path: path.to_string() }
    }

    pub fn fetch(&self, key: &str) -> u32 {
        self.size(key)
    }

    fn size(&self, key: &str) -> u32 {
        key.len() as u32 + self.path.len() as u32
    }
}

impl std::fmt::Display for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

pub enum Kind {
    Task,
    Note,
}

pub struct Item;

impl Item {
    pub fn get(&self) -> Kind {
        Kind::Task
    }
}

pub fn first(items: &[Item]) -> Kind {
    items[0].get()
}

#[cfg(test)]
mod tests {
    use super::Storage;
}
"#;

/// A crate of two files in a worktree of its own, its cache beside it.
struct Demo {
    top: PathBuf,
    repo: PathBuf,
}

impl Demo {
    fn new(name: &str) -> Demo {
        Demo::of(
            name,
            &[
                ("Cargo.toml", DEMO_MANIFEST),
                ("src/lib.rs", DEMO_LIB),
                ("src/store.rs", DEMO_STORE),
            ],
        )
    }

    /// A worktree of these files.
    fn of(name: &str, files: &[(&str, &str)]) -> Demo {
        let top = folder(name);
        let repo = top.join("repo");
        fs::create_dir_all(&repo).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        for (path, text) in files {
            let path = repo.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let repo = fs::canonicalize(&repo).unwrap();
        Demo { top, repo }
    }

    /// graff run from `at`.
    fn run_in(&self, at: &Path, args: &[&str]) -> Output {
        graff_in(at, &self.top.join("cache"), args)
    }

    /// graff's answer to a question asked from the worktree's top, which
    /// must be given.
    fn ask(&self, args: &[&str]) -> String {
        let out = self.run_in(&self.repo, args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// What graff says on stderr, refusing a question.
    fn refused(&self, args: &[&str]) -> String {
        let out = self.run_in(&self.repo, args);
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
        String::from_utf8(out.stderr).unwrap()
    }

    /// `text` with the worktree's top for `ROOT`.
    fn rooted(&self, text: &str) -> String {
        text.replace("ROOT", &self.repo.display().to_string())
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.top);
    }
}

#[test]
fn def_gives_the_place_the_doc_the_signature_and_the_impl_blocks() {
    let demo = Demo::new("def");
    assert_eq!(
        demo.ask(&["def", "Storage"]),
        demo.rooted(
            "def Storage in ROOT
src/store.rs:5-7 struct Storage, 2 impl blocks
  /// Where values live.
  pub struct Storage
  src/store.rs:9-22 impl Storage
  src/store.rs:24-28 impl std::fmt::Display for Storage
"
        )
    );
    // By its module path, from the crate's top or the library's name, or by a line.
    for name in [
        "Storage::open",
        "store::Storage::open",
        "crate::store::Storage::open",
        "demo::store::Storage::open",
        "src/store.rs:Storage::open",
        "src/store.rs:12",
    ] {
        assert_eq!(
            demo.ask(&["def", name]),
            demo.rooted(&format!(
                "def {name} in ROOT
src/store.rs:11-13 function Storage::open
  /// Opens the store at a path.
  pub fn open(path: &str) -> Storage
"
            ))
        );
    }
    // From the crate's top, the path is the whole of the name.
    assert_eq!(
        demo.refused(&["def", "crate::Storage::open"]),
        demo.rooted(
            "graff: no definition named crate::Storage::open in ROOT; nearest: store::Storage::open (src/store.rs:11-13)\n"
        )
    );
}

#[test]
fn callers_give_every_use_by_what_it_is_in() {
    let demo = Demo::new("callers");
    assert_eq!(
        demo.ask(&["callers", "Storage"]),
        demo.rooted(
            "callers Storage in ROOT
src/store.rs:5-7 struct Storage: 7 callers, 0 possible
  src/lib.rs (top level): use 3, 15
  src/lib.rs:6-9 function run: ref 7
  src/lib.rs:17-19 function other: ref 18
  src/store.rs:9-22 impl Storage: ref 9
  src/store.rs:11-13 function Storage::open: ref 11, 12
  src/store.rs:24-28 impl std::fmt::Display for Storage: ref 24
  src/store.rs:48-50 module tests: use 49
"
        )
    );
    // A method of a name std's types have too, called on what graff cannot tell the type of.
    assert_eq!(
        demo.ask(&["callers", "Item::get"]),
        demo.rooted(
            "callers Item::get in ROOT
src/store.rs:38-40 method Item::get: 0 callers, 1 possible
  possible: src/store.rs:43-45 function first: call 44 (or outside the crate)
"
        )
    );
}

#[test]
fn callees_give_what_a_definition_reaches() {
    let demo = Demo::new("callees");
    assert_eq!(
        demo.ask(&["callees", "run"]),
        demo.rooted(
            "callees run in ROOT
src/lib.rs:6-9 function run: 4 callees, 0 possible, 0 outside the crate
  src/lib.rs:11-13 function helper: call 8
  src/store.rs:5-7 struct Storage: ref 7
  src/store.rs:11-13 function Storage::open: call 7
  src/store.rs:15-17 method Storage::fetch: call 8
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "Storage::fmt"]),
        demo.rooted(
            "callees Storage::fmt in ROOT
src/store.rs:25-27 method <Storage as std::fmt::Display>::fmt: 0 callees, 0 possible, 3 outside the crate
  outside the crate: write!, std::fmt::Formatter, std::fmt::Result
"
        )
    );
}

#[test]
fn outline_nests_what_a_file_defines() {
    let demo = Demo::new("outline");
    let outline = demo.rooted(
        "outline src/store.rs in ROOT
src/store.rs: 50 lines, 16 symbols
  2 const MAX
  5-7 struct Storage
  9-22 impl Storage
    11-13 function open
    15-17 method fetch
    19-21 method size
  24-28 impl std::fmt::Display for Storage
    25-27 method fmt
  30-33 enum Kind: Task, Note
  35 struct Item
  37-41 impl Item
    38-40 method get
  43-45 function first
  48-50 module tests
",
    );
    assert_eq!(demo.ask(&["outline", "src/store.rs"]), outline);
    // From another folder of the worktree, by a path from there, or from elsewhere with -C.
    let out = demo.run_in(&demo.repo.join("src"), &["outline", "store.rs"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        outline.replacen("src/store.rs", "store.rs", 1)
    );
    let out = demo.run_in(&demo.top, &["outline", "src/store.rs", "-C", "repo"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), outline);
    assert_eq!(
        demo.refused(&["outline", "Cargo.toml"]),
        demo.rooted("graff: Cargo.toml is no file graff reads in ROOT\n")
    );
}

#[test]
fn impact_follows_callers_level_by_level() {
    let demo = Demo::new("impact");
    assert_eq!(
        demo.ask(&["impact", "size"]),
        demo.rooted(
            "impact size in ROOT
src/store.rs:19-21 method Storage::size
depth 1: 1 caller
  src/store.rs:15-17 method Storage::fetch
depth 2: 2 callers
  src/lib.rs:6-9 function run
  src/lib.rs:17-19 function other
depth 3: 0 callers
3 callers in 2 files, to depth 3
"
        )
    );
    assert_eq!(
        demo.ask(&["impact", "size", "--depth", "1"]),
        demo.rooted(
            "impact size in ROOT
src/store.rs:19-21 method Storage::size
depth 1: 1 caller
  src/store.rs:15-17 method Storage::fetch
1 caller in 1 file, to depth 1
"
        )
    );
    // What may reach it by name alone is counted, not followed.
    assert_eq!(
        demo.ask(&["impact", "Item::get"]),
        demo.rooted(
            "impact Item::get in ROOT
src/store.rs:38-40 method Item::get
depth 1: 0 callers, 1 possible not followed
0 callers in 0 files, to depth 1; nothing reaches depth 2
"
        )
    );
    let out = demo.run_in(&demo.repo, &["impact", "size", "--depth", "6"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn an_answer_fits_its_budget_and_says_what_it_left_out() {
    let demo = Demo::new("budget");
    let whole = demo.ask(&["callers", "Storage"]);
    let lines: Vec<&str> = whole.lines().collect();
    let cut = demo.ask(&["callers", "Storage", "--budget", "1"]);
    let needed: usize = cut
        .trim_end()
        .strip_suffix(" holds it all")
        .and_then(|rest| rest.rsplit_once("--budget "))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or_else(|| panic!("no budget named: {cut}"));
    assert_eq!(needed, whole.len().div_ceil(4));
    assert_eq!(
        demo.ask(&["callers", "Storage", "--budget", &needed.to_string()]),
        whole
    );
    for budget in (1..needed).step_by(7).chain([needed - 1]) {
        let out = demo.ask(&["callers", "Storage", "--budget", &budget.to_string()]);
        let (kept, last) = out.trim_end().rsplit_once('\n').unwrap();
        assert!(
            last.starts_with(&format!("cut to {budget} token"))
                && last.ends_with(&format!("; --budget {needed} holds it all")),
            "{out}"
        );
        // What is kept comes first in the whole answer, and fits, but for the
        // first line and the cut's when nothing else does.
        let kept: Vec<&str> = kept.lines().collect();
        assert_eq!(kept, lines[..kept.len()], "{out}");
        assert!(out.len() <= budget * 4 || kept.len() == 1, "{out}");
    }
}

#[test]
fn json_gives_the_same_answer() {
    let demo = Demo::new("json");
    let answer: serde_json::Value =
        serde_json::from_str(&demo.ask(&["callers", "Storage::open", "--json"])).unwrap();
    assert_eq!(
        answer,
        serde_json::json!({
            "query": "callers Storage::open",
            "root": demo.repo.display().to_string(),
            "results": [
                {
                    "target": {"path": "src/store.rs", "start": 11, "end": 13, "kind": "function", "qualified": "Storage::open"},
                    "callers": 2,
                    "possible": 0,
                },
                {
                    "caller": {"path": "src/lib.rs", "start": 6, "end": 9, "kind": "function", "qualified": "run"},
                    "uses": [{"use": "call", "line": 7}],
                    "of": "Storage::open",
                },
                {
                    "caller": {"path": "src/lib.rs", "start": 17, "end": 19, "kind": "function", "qualified": "other"},
                    "uses": [{"use": "call", "line": 18}],
                    "of": "Storage::open",
                },
            ],
            "cut": null,
        })
    );
}

#[test]
fn a_name_graff_does_not_know_is_refused_with_the_nearest() {
    let demo = Demo::new("unknown");
    assert_eq!(
        demo.refused(&["def", "Storag"]),
        demo.rooted(
            "graff: no definition named Storag in ROOT; nearest: store::Storage (src/store.rs:5-7), store (src/lib.rs:1)\n"
        )
    );
    assert_eq!(
        demo.refused(&["callees", "store"]),
        "graff: store names a module (src/lib.rs:1 module store), whose code is what it holds: graff outline FILE lists that\n"
    );
    assert_eq!(
        demo.refused(&["callers", "src/lib.rs"]),
        "graff: no `mod` item declares src/lib.rs, as none does a crate's root: graff outline src/lib.rs lists what it holds\n"
    );
    assert_eq!(
        demo.refused(&["def", "src/store.rs:"]),
        "graff: src/store.rs: names no symbol: write load, Storage::load, src/store.rs:Storage::load or src/store.rs:120\n"
    );
    // A path that names no file is refused as outline refuses it, not with
    // the names nearest its text.
    for written in ["src/stor.rs", "tools/draw.py", "docs/nowhere.md"] {
        assert_eq!(
            demo.refused(&["callers", written]),
            demo.rooted(&format!(
                "graff: {written} is no file graff reads in ROOT\n"
            )),
            "{written}"
        );
    }
}

/// A flake with a host, a folder of modules and functions of its own.
const NIX_DEMO: &[(&str, &str)] = &[
    (
        "flake.nix",
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs, ... }@inputs: {
    nixosConfigurations.box = nixpkgs.lib.nixosSystem {
      specialArgs = { myLib = import ./lib { inherit (nixpkgs) lib; }; };
      modules = [ ./hosts/box ./modules ];
    };
  };
}
"#,
    ),
    (
        "lib/default.nix",
        r#"{ lib }:
{
  importDir = import ./importDir.nix { inherit lib; };
  mkSys = import ./mkSys.nix { inherit lib; };
}
"#,
    ),
    (
        "lib/importDir.nix",
        r#"# Every module of a folder.
{ lib }:
dir: map (name: dir + "/${name}") (builtins.attrNames (builtins.readDir dir))
"#,
    ),
    (
        "lib/mkSys.nix",
        r#"{ lib }:
{ config, name, body ? { } }:
{
  # Turns the module on.
  options.sys.${name}.enable = lib.mkEnableOption name;
  config = lib.mkIf config.sys.${name}.enable body;
}
"#,
    ),
    (
        "hosts/box/default.nix",
        r#"{ ... }:
{
  sys.audio.enable = true;
  sys.video = {
    enable = false;
  };
  services.openssh.enable = true;
  home-manager.users.alice = _: { };
}
"#,
    ),
    (
        "modules/default.nix",
        r#"{ myLib, ... }:
{
  imports = myLib.importDir ./.;
}
"#,
    ),
    (
        "modules/audio.nix",
        r#"{ config, myLib, pkgs, ... }:
myLib.mkSys {
  inherit config;
  name = "audio";
  body.services.pipewire.enable = true;
  body.environment.systemPackages = [ pkgs.pavucontrol ];
}
"#,
    ),
    (
        "modules/video.nix",
        r#"{ config, myLib, ... }:
myLib.mkSys {
  inherit config;
  name = "video";
}
"#,
    ),
    (
        "modules/theme.nix",
        r#"{ config, myLib, ... }:
myLib.mkSys {
  inherit config;
  name = "theme";
  body.theme.enable = true;
}
"#,
    ),
];

#[test]
fn nix_def_gives_an_options_declaration_before_what_sets_it() {
    let demo = Demo::of("nix-def", NIX_DEMO);
    assert_eq!(
        demo.ask(&["def", "sys.audio.enable"]),
        demo.rooted(
            "def sys.audio.enable in ROOT
modules/audio.nix:2-7 option options.sys.audio.enable, written at lib/mkSys.nix:5
  /// Turns the module on.
  myLib.mkSys
"
        )
    );
    // A binding the name names more closely than any option sets another
    // option, declared outside: the worktree's is named only by its end.
    assert_eq!(
        demo.ask(&["def", "theme.enable"]),
        demo.rooted(
            "def theme.enable in ROOT
modules/theme.nix:2-6 option options.sys.theme.enable, written at lib/mkSys.nix:5
  /// Turns the module on.
  myLib.mkSys
modules/theme.nix:5 attribute config.theme.enable
  body.theme.enable = true;
"
        )
    );
    // No module calls the helper with `radio`: its own declaration, which
    // `${name}` names any name in.
    assert_eq!(
        demo.ask(&["def", "sys.radio.enable"]),
        demo.rooted(
            "def sys.radio.enable in ROOT
lib/mkSys.nix:5 option options.sys.${name}.enable
  /// Turns the module on.
  options.sys.${name}.enable = lib.mkEnableOption name;
"
        )
    );
    // What the worktree declares nowhere: the bindings that set it.
    assert_eq!(
        demo.ask(&["def", "services.openssh.enable"]),
        demo.rooted(
            "def services.openssh.enable in ROOT
hosts/box/default.nix:7 attribute services.openssh.enable
  services.openssh.enable = true;
"
        )
    );
    assert_eq!(
        demo.ask(&["def", "nixpkgs"]),
        demo.rooted(
            "def nixpkgs in ROOT
flake.nix:2 input inputs.nixpkgs
  inputs.nixpkgs.url = \"github:NixOS/nixpkgs/nixos-unstable\";
"
        )
    );
    // A binding defines each attrset its own path passes through; one the
    // helper's `body` is, where the helper puts it.
    assert_eq!(
        demo.ask(&["def", "services.pipewire"]),
        demo.rooted(
            "def services.pipewire in ROOT
modules/audio.nix:5 attribute config.services.pipewire.enable
  body.services.pipewire.enable = true;
"
        )
    );
    // A function's binding too.
    assert_eq!(
        demo.ask(&["def", "home-manager.users"]),
        demo.rooted(
            "def home-manager.users in ROOT
hosts/box/default.nix:8 function home-manager.users.alice
  home-manager.users.alice = _: { };
"
        )
    );
    // `${name}` stands for a name written between two named as they are:
    // `pipewire.enable` names no `sys.${name}.enable`.
    assert_eq!(
        demo.ask(&["def", "pipewire.enable"]),
        demo.rooted(
            "def pipewire.enable in ROOT
modules/audio.nix:5 attribute config.services.pipewire.enable
  body.services.pipewire.enable = true;
"
        )
    );
}

#[test]
fn nix_callers_give_where_an_option_is_set_and_who_imports_a_file() {
    let demo = Demo::of("nix-callers", NIX_DEMO);
    // The module that calls the helper declares the option.
    assert_eq!(
        demo.ask(&["callers", "sys.video.enable"]),
        demo.rooted(
            "callers sys.video.enable in ROOT
modules/video.nix:2-5 option options.sys.video.enable, written at lib/mkSys.nix:5: 1 caller, 0 possible
  hosts/box/default.nix:4-6 attribute sys.video: set 5
"
        )
    );
    // A folder `importDir` lists with builtins.readDir imports each module in it.
    assert_eq!(
        demo.ask(&["callers", "modules/audio.nix"]),
        demo.rooted(
            "callers modules/audio.nix in ROOT
modules/audio.nix:1-7 file: 1 caller, 0 possible
  modules/default.nix:3 attribute imports: path 3
"
        )
    );
    assert_eq!(
        demo.ask(&["callers", "nixpkgs"]),
        demo.rooted(
            "callers nixpkgs in ROOT
flake.nix:2 input inputs.nixpkgs: 2 callers, 0 possible
  flake.nix:4-7 attribute outputs.nixosConfigurations.box: call 4
  flake.nix:5 attribute outputs.nixosConfigurations.box.specialArgs.myLib: ref 5
"
        )
    );
}

#[test]
fn nix_outline_callees_and_impact() {
    let demo = Demo::of("nix-outline", NIX_DEMO);
    assert_eq!(
        demo.ask(&["outline", "hosts/box/default.nix"]),
        demo.rooted(
            "outline hosts/box/default.nix in ROOT
hosts/box/default.nix: 9 lines, 5 symbols
  3 attribute sys.audio.enable
  4-6 attribute sys.video
    5 attribute enable
  7 attribute services.openssh.enable
  8 function home-manager.users.alice
"
        )
    );
    // A helper's module: what it declares and sets, its arguments where
    // nothing else stands for them.
    assert_eq!(
        demo.ask(&["outline", "modules/audio.nix"]),
        demo.rooted(
            "outline modules/audio.nix in ROOT
modules/audio.nix: 7 lines, 4 symbols
  2-7 option options.sys.audio.enable, written at lib/mkSys.nix:5
  4 argument name
  5 attribute config.services.pipewire.enable
  6 attribute config.environment.systemPackages
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "modules/audio.nix"]),
        demo.rooted(
            "callees modules/audio.nix in ROOT
modules/audio.nix:1-7 file: 1 callee, 0 possible, 1 outside the worktree
  lib/default.nix:4 attribute mkSys: call 2
  outside the worktree: pkgs.pavucontrol
"
        )
    );
    // Through the files that import what reaches it, up to the flake.
    assert_eq!(
        demo.ask(&["impact", "lib/importDir.nix"]),
        demo.rooted(
            "impact lib/importDir.nix in ROOT
lib/importDir.nix:1-3 file
depth 1: 1 caller
  lib/default.nix:3 attribute importDir
depth 2: 2 callers
  flake.nix:5 attribute outputs.nixosConfigurations.box.specialArgs.myLib
  modules/default.nix:3 attribute imports
depth 3: 1 caller
  flake.nix:6 attribute outputs.nixosConfigurations.box.modules
4 callers in 3 files, to depth 3
"
        )
    );
}

#[test]
fn nix_outline_gives_the_names_one_inherit_binds_side_by_side() {
    let demo = Demo::of(
        "nix-inherit",
        &[(
            "m.nix",
            "{ lib, cfg, ... }:\nlet\n  inherit (lib) mkIf types;\nin\n{\n  services.x = { inherit (cfg) port user; };\n  services.y = { a = 1; };\n}\n",
        )],
    );
    assert_eq!(
        demo.ask(&["outline", "m.nix"]),
        demo.rooted(
            "outline m.nix in ROOT
m.nix: 8 lines, 7 symbols
  3 variable mkIf
  3 variable types
  6 attribute services.x
    6 attribute port
    6 attribute user
  7 attribute services.y
    7 attribute a
"
        )
    );
}

#[test]
fn nix_callees_leave_out_what_a_package_is_made_of() {
    let demo = Demo::of(
        "nix-consumed",
        &[
            (
                "foo.nix",
                "{ lib, ... }:\n{\n  options.services.foo.instances = lib.mkOption {\n    type = lib.types.attrsOf (lib.types.submodule { options.name = lib.mkOption { }; });\n  };\n}\n",
            ),
            (
                "host.nix",
                "{ lib, pkgs, ... }:\nlet\n  env = kbd: pkgs.buildEnv {\n    name = \"console-env\";\n  };\n  named = kbd: lib.mapAttrs (n: v: {\n    name = n;\n  }) { };\nin\n{ }\n",
            ),
        ],
    );
    // buildEnv's `name` is the package's; one mapAttrs makes may be an option's.
    assert_eq!(
        demo.ask(&["callees", "host.nix:3"]),
        demo.rooted(
            "callees host.nix:3 in ROOT
host.nix:3-5 function env: 0 callees, 0 possible, 1 outside the worktree
  outside the worktree: pkgs.buildEnv
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "host.nix:6"]),
        demo.rooted(
            "callees host.nix:6 in ROOT
host.nix:6-8 function named: 1 callee, 0 possible, 1 outside the worktree
  foo.nix:4 option options.services.foo.instances.type.options.name: set 7
  outside the worktree: lib.mapAttrs
"
        )
    );
}

#[test]
fn nix_instances_are_made_again_when_a_file_they_come_from_changes() {
    let demo = Demo::of("nix-kept", NIX_DEMO);
    let audio = "def sys.audio.enable in ROOT
modules/audio.nix:2-7 option options.sys.audio.enable, written at lib/mkSys.nix:5
  /// Turns the module on.
  myLib.mkSys
";
    // Made by the first question, kept for the second.
    assert_eq!(demo.ask(&["def", "sys.audio.enable"]), demo.rooted(audio));
    assert_eq!(demo.ask(&["def", "sys.audio.enable"]), demo.rooted(audio));
    // The helper, not the file that calls it, changes what the call declares.
    let helper = demo.repo.join("lib/mkSys.nix");
    let text = fs::read_to_string(&helper).unwrap();
    fs::write(&helper, text.replace(".enable", ".on")).unwrap();
    assert_eq!(
        demo.ask(&["def", "sys.audio.on"]),
        demo.rooted(
            "def sys.audio.on in ROOT
modules/audio.nix:2-7 option options.sys.audio.on, written at lib/mkSys.nix:5
  /// Turns the module on.
  myLib.mkSys
"
        )
    );
}

/// Scripts as ctx writes them: hooks and bins with no extension, told by
/// their shebang, a library they source, and a test that runs a hook.
const BASH_DEMO: &[(&str, &str)] = &[
    (
        "src/lib/ledger.sh",
        r#"# Appends events to the funnel.

# Writes one event: its kind, the session, then key-value pairs.
ctx_log() {
  local kind=$1
  printf '%s %s\n' "$kind" "$*" >>"${CTX_FUNNEL:-/tmp/funnel}"
}
"#,
    ),
    (
        "src/hooks/handoff",
        r#"#!/usr/bin/env bash
# Writes a handoff when the window fills.
set -u
TOP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../lib/ledger.sh
. "$TOP/lib/ledger.sh"

# --- the window ---
# How full the window is, in percent.
used() {
  echo 50
}
pct=$(used) max=40

# --- the handoff ---
if [ "$pct" -ge "$max" ]; then
  CTX_FUNNEL="$TOP/state/funnel" ctx_log handoff "$pct"
  setsid -f "$TOP/bin/reset" "$pct"
fi
"#,
    ),
    (
        "src/bin/reset",
        r#"#!/bin/bash
# Resets a session the handoff left.
here=$(dirname "$0")
. "$here/../lib/ledger.sh"
ctx_log reset "$1"
"#,
    ),
    (
        "src/test.sh",
        r#"#!/bin/bash
HOOKS="$(dirname "$0")/hooks"
out=$(env -u CTX_FUNNEL "$HOOKS/handoff")
echo "$out"
"#,
    ),
    ("notes.txt", "#!/bin/bash\nnot_a_call\n"),
];

#[test]
fn bash_def_and_callers_reach_through_what_scripts_source_and_run() {
    let demo = Demo::of("bash-callers", BASH_DEMO);
    assert_eq!(
        demo.ask(&["def", "ctx_log"]),
        demo.rooted(
            "def ctx_log in ROOT
src/lib/ledger.sh:4-7 function ctx_log
  /// Writes one event: its kind, the session, then key-value pairs.
  ctx_log()
"
        )
    );
    // A call at a script's top is in the section its banner opens, or in
    // none; the variable that line exports for the call holds none of it.
    assert_eq!(
        demo.ask(&["callers", "ctx_log"]),
        demo.rooted(
            "callers ctx_log in ROOT
src/lib/ledger.sh:4-7 function ctx_log: 2 callers, 0 possible
  src/bin/reset (top level): call 5
  src/hooks/handoff:15-19 section the handoff: call 17
"
        )
    );
    assert_eq!(
        demo.ask(&["callers", "src/lib/ledger.sh"]),
        demo.rooted(
            "callers src/lib/ledger.sh in ROOT
src/lib/ledger.sh:1-7 file: 2 callers, 0 possible
  src/bin/reset (top level): path 4
  src/hooks/handoff (top level): path 6
"
        )
    );
    // The library reads what the hook that sources it exports; the test
    // takes it out of what it runs the hook with.
    assert_eq!(
        demo.ask(&["callers", "CTX_FUNNEL"]),
        demo.rooted(
            "callers CTX_FUNNEL in ROOT
src/hooks/handoff:17 environment CTX_FUNNEL: 2 callers, 0 possible
  src/lib/ledger.sh:4-7 function ctx_log: ref 6
  src/test.sh (top level): set 3
"
        )
    );
}

#[test]
fn bash_outline_callees_and_impact() {
    let demo = Demo::of("bash-outline", BASH_DEMO);
    assert_eq!(
        demo.ask(&["outline", "src/hooks/handoff"]),
        demo.rooted(
            "outline src/hooks/handoff in ROOT
src/hooks/handoff: 19 lines, 7 symbols
  4 variable TOP
  8-13 section the window
    10-12 function used
    13 variable pct
    13 variable max
  15-19 section the handoff
    17 environment CTX_FUNNEL
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "src/hooks/handoff"]),
        demo.rooted(
            "callees src/hooks/handoff in ROOT
src/hooks/handoff:1-19 file: 7 callees, 0 possible, 6 outside the worktree
  src/bin/reset:1-5 file: path 18
  src/hooks/handoff:4 variable TOP: ref 6, 17, 18
  src/hooks/handoff:10-12 function used: call 13
  src/hooks/handoff:13 variable pct: ref 16, 17, 18
  src/hooks/handoff:13 variable max: ref 16
  src/lib/ledger.sh:1-7 file: path 6
  src/lib/ledger.sh:4-7 function ctx_log: call 17
  outside the worktree: set, cd, dirname, pwd, echo, setsid
"
        )
    );
    // Through the scripts that run the one that reaches it: the hook's
    // section runs the bin, and the test runs the hook.
    assert_eq!(
        demo.ask(&["impact", "ctx_log"]),
        demo.rooted(
            "impact ctx_log in ROOT
src/lib/ledger.sh:4-7 function ctx_log
depth 1: 2 callers
  src/bin/reset (top level)
  src/hooks/handoff:15-19 section the handoff
depth 2: 1 caller
  src/test.sh (top level)
depth 3: 0 callers
3 callers in 3 files, to depth 3
"
        )
    );
    // A text file with a shebang is no script.
    let refused = demo.run_in(&demo.repo, &["outline", "notes.txt"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("notes.txt is no file graff reads"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

/// Python as kimi-k3-in-c's tools/ write it: a script that puts its own
/// folder on sys.path to import its neighbours, a module of classes, and
/// one of paths.
const PYTHON_DEMO: &[(&str, &str)] = &[
    (
        "tools/_paths.py",
        r#""""Where the fixtures live."""
import os

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURES = os.path.join(HERE, "fixtures")
"#,
    ),
    (
        "tools/k3_ref.py",
        r#""""A reference model."""
from dataclasses import dataclass


@dataclass
class K3Config:
    """The model's sizes."""

    dim: int = 8


class Base:
    def reset(self):
        """Forgets the state."""
        self.state = None


class K3Model(Base):
    """The whole model."""

    def __init__(self, cfg: K3Config):
        self.cfg = cfg
        self.reset()

    def forward(self, ids,
                states=None):
        return self.step(ids)

    def step(self, ids):
        return ids


def tiny_config(**kw) -> K3Config:
    """A small model. It has every mechanism."""
    return K3Config(**kw)
"#,
    ),
    (
        "tools/make_oracle",
        r#"#!/usr/bin/env python3
"""Writes the oracle."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from k3_ref import K3Model, tiny_config
from _paths import FIXTURES


def build():
    model = K3Model(tiny_config())
    return model.forward([1, 2]), model.items()


def main():
    print(FIXTURES, build())


if __name__ == "__main__":
    main()
"#,
    ),
];

#[test]
fn python_def_and_callers_reach_through_imports_and_instances() {
    let demo = Demo::of("python-callers", PYTHON_DEMO);
    // The signature runs to the colon, past the decorator.
    assert_eq!(
        demo.ask(&["def", "forward"]),
        demo.rooted(
            "def forward in ROOT
tools/k3_ref.py:25-27 method K3Model.forward
  def forward(self, ids, states=None)
"
        )
    );
    assert_eq!(
        demo.ask(&["def", "K3Config"]),
        demo.rooted(
            "def K3Config in ROOT
tools/k3_ref.py:5-9 class K3Config
  /// The model's sizes.
  class K3Config
"
        )
    );
    // A method called on what a constructor made, and one called through
    // self that a base defines.
    assert_eq!(
        demo.ask(&["callers", "K3Model.forward"]),
        demo.rooted(
            "callers K3Model.forward in ROOT
tools/k3_ref.py:25-27 method K3Model.forward: 1 caller, 0 possible
  tools/make_oracle:11-13 function build: call 13
"
        )
    );
    assert_eq!(
        demo.ask(&["callers", "reset"]),
        demo.rooted(
            "callers reset in ROOT
tools/k3_ref.py:13-15 method Base.reset: 1 caller, 0 possible
  tools/k3_ref.py:21-23 method K3Model.__init__: call 23
"
        )
    );
    // A module is reached by the `from` imports that name it, found in the
    // folder the script puts on sys.path.
    assert_eq!(
        demo.ask(&["callers", "k3_ref.py"]),
        demo.rooted(
            "callers k3_ref.py in ROOT
tools/k3_ref.py:1-35 file: 1 caller, 0 possible
  tools/make_oracle (top level): use 7
"
        )
    );
}

#[test]
fn python_outline_callees_and_impact() {
    let demo = Demo::of("python-outline", PYTHON_DEMO);
    // What a method sets through self is its class's.
    assert_eq!(
        demo.ask(&["outline", "tools/k3_ref.py"]),
        demo.rooted(
            "outline tools/k3_ref.py in ROOT
tools/k3_ref.py: 35 lines, 11 symbols
  5-9 class K3Config
    9 variable dim
  12-15 class Base
    13-15 method reset
    15 variable state
  18-30 class K3Model
    21-23 method __init__
    22 variable cfg
    25-27 method forward
    29-30 method step
  33-35 function tiny_config
"
        )
    );
    // A call of a class makes an instance; a method the worktree does not
    // define is outside it.
    assert_eq!(
        demo.ask(&["callees", "build"]),
        demo.rooted(
            "callees build in ROOT
tools/make_oracle:11-13 function build: 3 callees, 0 possible, 1 outside the worktree
  tools/k3_ref.py:18-30 class K3Model: call 12
  tools/k3_ref.py:25-27 method K3Model.forward: call 13
  tools/k3_ref.py:33-35 function tiny_config: call 12
  outside the worktree: .items
"
        )
    );
    assert_eq!(
        demo.ask(&["impact", "tiny_config"]),
        demo.rooted(
            "impact tiny_config in ROOT
tools/k3_ref.py:33-35 function tiny_config
depth 1: 1 caller
  tools/make_oracle:11-13 function build
depth 2: 1 caller
  tools/make_oracle:16-17 function main
depth 3: 1 caller
  tools/make_oracle (top level)
3 callers in 1 file, to depth 3
"
        )
    );
}

/// C as kimi-k3-in-c writes it: a header of prototypes, each with its
/// comment, that includes a neighbour; a file that defines what the header
/// declares; and one that calls it, both including the header by the end
/// of its path.
const C_DEMO: &[(&str, &str)] = &[
    (
        "include/k3/k3.h",
        r#"#ifndef K3_H
#define K3_H
#include "types.h"

/* Multiplies a matrix by a vector. See the definition. */
void k3_matmul(float *y, const k3_tensor *w,
               const float *x);

enum { K3_WF32 = 0, K3_WBF16 = 1 };

#endif
"#,
    ),
    (
        "include/k3/types.h",
        r#"/* A tensor: its rows and its data. */
typedef struct k3_tensor {
    int rows;
    float *data;
} k3_tensor;
"#,
    ),
    (
        "src/core/k3_ops.c",
        r#"#include "k3/k3.h"

/* The dot product of two rows. */
static float dot(const float *a, const float *b, int n) {
    #define AT(i) (a[i] * b[i])
    float s = 0;
    for (int i = 0; i < n; i++) s += AT(i);
    return s;
}

/* y = W . x, row by row. */
void k3_matmul(float *y, const k3_tensor *w, const float *x)
{
    for (int r = 0; r < w->rows; r++)
        y[r] = dot(w->data + r, x, w->rows);
}
"#,
    ),
    (
        "src/cli/k3_run.c",
        r#"#include "k3/k3.h"
#include <stdio.h>

int main(void) {
    k3_tensor w = { 1, 0 };
    float y[1], x[1] = { 1 };
    k3_matmul(y, &w, x);
    printf("%f\n", y[0]);
    return K3_WF32 + (int)sizeof(k3_tensor);
}
"#,
    ),
];

#[test]
fn c_def_gives_the_definition_before_its_prototype() {
    let demo = Demo::of("c-def", C_DEMO);
    assert_eq!(
        demo.ask(&["def", "k3_matmul"]),
        demo.rooted(
            "def k3_matmul in ROOT
src/core/k3_ops.c:12-16 function k3_matmul
  /// y = W . x, row by row.
  void k3_matmul(float *y, const k3_tensor *w, const float *x)
include/k3/k3.h:6-7 declaration k3_matmul
  /// Multiplies a matrix by a vector.
  void k3_matmul(float *y, const k3_tensor *w, const float *x);
"
        )
    );
    // The prototype is the first a budget cuts.
    let whole = demo.ask(&["def", "k3_matmul"]);
    let needed = whole.len().div_ceil(4);
    let budget = (needed - 1).to_string();
    let lines: Vec<&str> = whole.lines().collect();
    assert_eq!(
        demo.ask(&["def", "k3_matmul", "--budget", &budget]),
        format!(
            "{}\ncut to {budget} tokens, leaving out declarations 1; --budget {needed} holds it all\n",
            lines[..4].join("\n")
        )
    );
}

#[test]
fn c_callers_reach_the_definition_and_a_header_its_includers() {
    let demo = Demo::of("c-callers", C_DEMO);
    // The call reaches the definition behind the prototype it sees, which
    // stands for no use of its own.
    assert_eq!(
        demo.ask(&["callers", "k3_matmul"]),
        demo.rooted(
            "callers k3_matmul in ROOT
src/core/k3_ops.c:12-16 function k3_matmul: 1 caller, 0 possible
  src/cli/k3_run.c:4-10 function main: call 7
"
        )
    );
    assert_eq!(
        demo.ask(&["callers", "k3.h"]),
        demo.rooted(
            "callers k3.h in ROOT
include/k3/k3.h:1-11 file: 2 callers, 0 possible
  src/cli/k3_run.c (top level): use 1
  src/core/k3_ops.c (top level): use 1
"
        )
    );
    // Through the header the header includes; `sizeof(k3_tensor)` names
    // the type too.
    assert_eq!(
        demo.ask(&["callers", "k3_tensor"]),
        demo.rooted(
            "callers k3_tensor in ROOT
include/k3/types.h:2-5 struct k3_tensor: 3 callers, 0 possible
  include/k3/k3.h:6-7 declaration k3_matmul: ref 6
  src/cli/k3_run.c:4-10 function main: ref 5, 9
  src/core/k3_ops.c:12-16 function k3_matmul: ref 12
"
        )
    );
}

#[test]
fn c_outline_callees_and_impact() {
    let demo = Demo::of("c-outline", C_DEMO);
    // The enumerators of an enum with no name hold none of each other.
    assert_eq!(
        demo.ask(&["outline", "include/k3/k3.h"]),
        demo.rooted(
            "outline include/k3/k3.h in ROOT
include/k3/k3.h: 11 lines, 3 symbols
  6-7 declaration k3_matmul
  9 variant K3_WF32
  9 variant K3_WBF16
"
        )
    );
    // A macro a function defines for itself is the function's callee.
    assert_eq!(
        demo.ask(&["outline", "src/core/k3_ops.c"]),
        demo.rooted(
            "outline src/core/k3_ops.c in ROOT
src/core/k3_ops.c: 16 lines, 3 symbols
  4-9 function dot
    5 macro AT
  12-16 function k3_matmul
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "main"]),
        demo.rooted(
            "callees main in ROOT
src/cli/k3_run.c:4-10 function main: 3 callees, 0 possible, 1 outside the worktree
  include/k3/k3.h:9 variant K3_WF32: ref 9
  include/k3/types.h:2-5 struct k3_tensor: ref 5, 9
  src/core/k3_ops.c:12-16 function k3_matmul: call 7
  outside the worktree: printf
"
        )
    );
    assert_eq!(
        demo.ask(&["impact", "dot"]),
        demo.rooted(
            "impact dot in ROOT
src/core/k3_ops.c:4-9 function dot
depth 1: 1 caller
  src/core/k3_ops.c:12-16 function k3_matmul
depth 2: 1 caller
  src/cli/k3_run.c:4-10 function main
depth 3: 0 callers
2 callers in 2 files, to depth 3
"
        )
    );
}

const MARKDOWN_DEMO: &[(&str, &str)] = &[
    (
        "readme.md",
        r#"# K3

A small engine. See [the guide](docs/guide.md).

## Code

`k3_matmul` multiplies, at [its line](src/k3_ops.c#L4); `src/k3_ops.c:8` runs it,
and so does src/k3_run.c. `kind` is a word, and `K3_MAX` is no name here.

## Usage

[Why](docs/guide.md#why-it-works), [the 30th](docs/guide.md#30).

## Usage

Again.
"#,
    ),
    (
        "docs/guide.md",
        r#"# Guide

## Why it works

Because [the readme](../readme.md#code) says so.

## <a id="30"></a>30. Thirty

Thirty.
"#,
    ),
    (
        "src/k3_ops.c",
        r#"/* y = W . x */
void k3_matmul(float *y, const float *x);

void k3_matmul(float *y, const float *x) {
    y[0] = x[0];
}

int main(void) {
    float y[1], x[1] = { 1 };
    k3_matmul(y, x);
    return 0;
}
"#,
    ),
    ("src/k3_run.c", "int run(void) { return 0; }\n"),
];

#[test]
fn markdown_outline_and_def_name_a_section_by_its_heading_or_its_anchor() {
    let demo = Demo::of("md-outline", MARKDOWN_DEMO);
    assert_eq!(
        demo.ask(&["outline", "readme.md"]),
        demo.rooted(
            "outline readme.md in ROOT
readme.md: 16 lines, 4 symbols
  1-16 section K3
    5-8 section Code
    10-12 section Usage
    14-16 section Usage
"
        )
    );
    for written in ["Why it works", "why-it-works", "docs/guide.md#why-it-works"] {
        assert_eq!(
            demo.ask(&["def", written]),
            demo.rooted(&format!(
                "def {written} in ROOT
docs/guide.md:3-5 section Why it works #why-it-works
  /// Because the readme says so.
  ## Why it works
"
            )),
            "{written}"
        );
    }
    // A custom anchor names the section it is in; a second heading of one
    // text is named by its anchor's number.
    assert_eq!(
        demo.ask(&["def", "docs/guide.md#30"]),
        demo.rooted(
            "def docs/guide.md#30 in ROOT
docs/guide.md:7-9 section 30. Thirty #30
  /// Thirty.
  ## <a id=\"30\"></a>30. Thirty
"
        )
    );
    assert_eq!(
        demo.ask(&["def", "usage-1"]),
        demo.rooted(
            "def usage-1 in ROOT
readme.md:14-16 section Usage #usage-1
  /// Again.
  ## Usage
"
        )
    );
}

/// A crate whose readme names its files and code, and a Python tool whose
/// test sets a value through the module it imports.
const DOCS_DEMO: &[(&str, &str)] = &[
    ("Cargo.toml", DEMO_MANIFEST),
    ("src/lib.rs", DEMO_LIB),
    ("src/store.rs", DEMO_STORE),
    (
        "readme.md",
        r#"# Demo

## Storage

`src/store.rs` keeps values, which `Storage::open` opens.

## Reloads in silence

Nothing.
"#,
    ),
    ("tools/draw.py", "WORK = 'w'\n"),
    ("tools/test_draw.py", "import draw\n\ndraw.WORK = 'x'\n"),
];

#[test]
fn a_rust_file_s_callers_are_the_links_and_mentions_of_its_module() {
    let demo = Demo::of("md-module", DOCS_DEMO);
    for written in ["src/store.rs", "store.rs", "store"] {
        assert_eq!(
            demo.ask(&["callers", written]),
            demo.rooted(&format!(
                "callers {written} in ROOT
src/lib.rs:1 module store: 1 caller, 0 possible; links and mentions only, as Rust's uses reach what they name in it
  readme.md:3-5 section Storage #storage: mention 5
"
            )),
            "{written}"
        );
    }
    assert_eq!(
        demo.refused(&["callees", "src/store.rs"]),
        "graff: src/store.rs names a module (src/lib.rs:1 module store), whose code is what it holds: graff outline FILE lists that\n"
    );
    // A heading near a qualified name is no name near it, but one near a
    // heading is.
    assert_eq!(
        demo.refused(&["def", "Storage::loads"]),
        demo.rooted("graff: no definition named Storage::loads in ROOT, nor a name near it\n")
    );
    assert_eq!(
        demo.refused(&["def", "Reloads in silenc"]),
        demo.rooted(
            "graff: no definition named Reloads in silenc in ROOT; nearest: Reloads in silence (readme.md:7-9)\n"
        )
    );
}

#[test]
fn a_section_headed_with_a_name_of_the_code_is_named_by_its_anchor_alone() {
    let demo = Demo::of("md-code-name", DOCS_DEMO);
    // The struct is what `Storage` stands for, not the readme's section.
    assert_eq!(
        demo.ask(&["def", "Storage"]),
        demo.rooted(
            "def Storage in ROOT
src/store.rs:5-7 struct Storage, 2 impl blocks
  /// Where values live.
  pub struct Storage
  src/store.rs:9-22 impl Storage
  src/store.rs:24-28 impl std::fmt::Display for Storage
"
        )
    );
    assert_eq!(
        demo.ask(&["def", "readme.md#storage"]),
        demo.rooted(
            "def readme.md#storage in ROOT
readme.md:3-5 section Storage #storage
  /// src/store.rs keeps values, which Storage::open opens.
  ## Storage
"
        )
    );
}

#[test]
fn a_python_module_s_callers_hold_the_uses_of_the_name_an_import_binds() {
    let demo = Demo::of("py-module", DOCS_DEMO);
    assert_eq!(
        demo.ask(&["callers", "tools/draw.py"]),
        demo.rooted(
            "callers tools/draw.py in ROOT
tools/draw.py:1 file: 1 caller, 0 possible
  tools/test_draw.py (top level): ref 3; use 1
"
        )
    );
}

#[test]
fn markdown_callers_and_callees_follow_links_and_mentions() {
    let demo = Demo::of("md-callers", MARKDOWN_DEMO);
    assert_eq!(
        demo.ask(&["callers", "k3_matmul"]),
        demo.rooted(
            "callers k3_matmul in ROOT
src/k3_ops.c:4-6 function k3_matmul: 2 callers, 0 possible
  readme.md:5-8 section Code #code: link 7; mention 7
  src/k3_ops.c:8-12 function main: call 10
"
        )
    );
    assert_eq!(
        demo.ask(&["callers", "docs/guide.md#30"]),
        demo.rooted(
            "callers docs/guide.md#30 in ROOT
docs/guide.md:7-9 section 30. Thirty #30: 1 caller, 0 possible
  readme.md:10-12 section Usage #usage: link 12
"
        )
    );
    assert_eq!(
        demo.ask(&["callees", "Code"]),
        demo.rooted(
            "callees Code in ROOT
readme.md:5-8 section Code #code: 3 callees, 0 possible, 1 outside the worktree
  src/k3_ops.c:4-6 function k3_matmul: link 7; mention 7
  src/k3_ops.c:8-12 function main: mention 7
  src/k3_run.c:1 file: mention 8
  outside the worktree: K3_MAX
"
        )
    );
}
