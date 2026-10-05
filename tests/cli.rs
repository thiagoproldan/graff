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
        demo.refused(&["callers", "store"]),
        "graff: store names a module (src/lib.rs:1 module store), and graff ties no uses to modules: graff outline FILE lists what one holds\n"
    );
    assert_eq!(
        demo.refused(&["def", "src/store.rs:"]),
        "graff: src/store.rs: names no symbol: write load, Storage::load, src/store.rs:Storage::load or src/store.rs:120\n"
    );
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
];

#[test]
fn nix_def_gives_an_options_declaration_before_what_sets_it() {
    let demo = Demo::of("nix-def", NIX_DEMO);
    assert_eq!(
        demo.ask(&["def", "sys.audio.enable"]),
        demo.rooted(
            "def sys.audio.enable in ROOT
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
    // A binding defines each attrset its own path passes through.
    assert_eq!(
        demo.ask(&["def", "services.pipewire"]),
        demo.rooted(
            "def services.pipewire in ROOT
modules/audio.nix:5 attribute body.services.pipewire.enable
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
modules/audio.nix:5 attribute body.services.pipewire.enable
  body.services.pipewire.enable = true;
"
        )
    );
}

#[test]
fn nix_callers_give_where_an_option_is_set_and_who_imports_a_file() {
    let demo = Demo::of("nix-callers", NIX_DEMO);
    // One declaration serves every `sys.*.enable`: the answer keeps the one asked.
    assert_eq!(
        demo.ask(&["callers", "sys.video.enable"]),
        demo.rooted(
            "callers sys.video.enable in ROOT
lib/mkSys.nix:5 option options.sys.${name}.enable: 1 caller, 0 possible
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
