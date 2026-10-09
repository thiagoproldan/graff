"""What the Markdown checks share: a corpus, taken out of a git repository
at a commit or read as a folder is, or a registry's crates, each its own
worktree; the tools nixpkgs builds; and graff's examples run on a worktree."""

import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "nix"))

from common import build, graff_version, ratio, run  # noqa: E402,F401

# The extensions src/lang.rs reads as Markdown, as it compares them: exactly.
MARKDOWN = (".md", ".markdown")


def tool(attribute, binary):
    """A program as nixpkgs builds it, by its path in the store."""
    built = run(["nix", "build", "--no-link", "--print-out-paths", f"nixpkgs#{attribute}"]).stdout.split()[0]
    return os.path.join(built, "bin", binary)


def listed(folder):
    """Every file under a folder, from its top, as `git ls-files` lists a
    fresh worktree of it: not .git's, nor a symbolic link."""
    found = []
    for directory, folders, names in os.walk(folder):
        folders[:] = sorted(f for f in folders if f != ".git")
        for name in names:
            path = os.path.join(directory, name)
            if not os.path.islink(path):
                found.append(os.path.relpath(path, folder))
    return sorted(found)


class Worktree:
    """A worktree's files and graff's reading of them."""

    def __init__(self, folder, name):
        self.folder, self.name = folder, name
        self.paths = listed(folder)
        self.markdown = [p for p in self.paths if p.endswith(MARKDOWN)]
        self.texts = {}

    def read(self, path):
        """A file's text, as graff reads it: bad UTF-8 replaced, its line
        endings kept."""
        if path not in self.texts:
            with open(os.path.join(self.folder, path), encoding="utf-8", errors="replace", newline="") as file:
                self.texts[path] = file.read()
        return self.texts[path]

    def extractions(self, binaries):
        """examples/extract.rs's extraction of each Markdown file, by path."""
        if not self.markdown:
            return {}
        done = run([binaries["extract"]], cwd=self.folder, stdin="\n".join(self.markdown) + "\n", check=False)
        return {item["path"]: item["extraction"] for item in map(json.loads, done.stdout.splitlines())}

    def edges(self, binaries, broken=False):
        """examples/resolve.rs's edges of the Markdown files, resolved against
        every file graff reads."""
        command = [binaries["resolve"], self.folder, "--markdown"] + (["--broken"] if broken else [])
        done = run(command, stdin="\n".join(self.paths) + "\n")
        return [json.loads(line) for line in done.stdout.splitlines()]


class Corpus:
    """The worktrees a check reads, and what they are."""

    def __init__(self, name, source, at=None, crates=False):
        self.name, self.made = name, None
        source = os.path.realpath(os.path.expanduser(source))
        repository = os.path.isdir(os.path.join(source, ".git")) or (
            os.path.isfile(os.path.join(source, "HEAD")) and os.path.isdir(os.path.join(source, "objects")))
        if crates:
            folders = sorted(os.path.join(source, f) for f in os.listdir(source)
                             if os.path.isfile(os.path.join(source, f, "README.md")))
            self.worktrees = [Worktree(f, os.path.basename(f)) for f in folders]
            self.described = f"{source}: {len(folders)} crates with a README.md, each a worktree of its own"
        elif repository:
            commit = run(["git", "-C", source, "rev-parse", "--short=7", at or "HEAD"]).stdout.strip()
            self.made = tempfile.mkdtemp(prefix="graff-markdown-")
            archive = subprocess.Popen(["git", "-C", source, "archive", commit], stdout=subprocess.PIPE)
            subprocess.run(["tar", "-x", "-C", self.made], stdin=archive.stdout, check=True)
            if archive.wait() != 0:
                raise RuntimeError("git archive failed")
            self.worktrees = [Worktree(self.made, name)]
            self.described = f"{source} at {commit} (git archive)"
        else:
            if at:
                raise SystemExit("--at needs a git repository")
            self.worktrees = [Worktree(source, name)]
            self.described = f"{source}, as it is"

    def close(self):
        if self.made:
            shutil.rmtree(self.made, ignore_errors=True)


def arguments(usage):
    """NAME SOURCE [--at COMMIT] [--crates], and the flags a check adds."""
    args = sys.argv[1:]
    flags = {}
    for flag, takes in (("--at", True), ("--crates", False), ("--marksman", False), ("--seed", True),
                        ("--size", True), ("--batch", True), ("--judged", True)):
        if flag in args:
            index = args.index(flag)
            if takes:
                if index + 1 == len(args):
                    sys.exit(usage)
                flags[flag] = args[index + 1]
                del args[index:index + 2]
            else:
                flags[flag] = True
                del args[index]
    return args, flags
