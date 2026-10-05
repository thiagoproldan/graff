"""What the Nix checks share: a commit of a repository taken out with git
archive, graff built and run on it, and the line that says what ran."""

import json
import os
import shutil
import subprocess
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))
PRIVATE = os.path.join(HERE, "private")


def run(command, cwd=None, stdin=None, env=None, check=True):
    return subprocess.run(command, cwd=cwd, input=stdin, env=env, capture_output=True, text=True, check=check)


def build():
    """graff and the examples the checks read, as the worktree has them now."""
    run(["cargo", "build", "--release", "--quiet", "--bin", "graff", "--example", "resolve", "--example", "extract"],
        cwd=GRAFF)
    release = os.path.join(GRAFF, "target", "release")
    return {"graff": os.path.join(release, "graff"), "resolve": os.path.join(release, "examples", "resolve"),
            "extract": os.path.join(release, "examples", "extract")}


def graff_version():
    head = run(["git", "-C", GRAFF, "rev-parse", "--short=7", "HEAD"]).stdout.strip()
    dirty = run(["git", "-C", GRAFF, "status", "--porcelain", "--untracked-files=no"]).stdout.strip()
    return f"graff {head}{' with uncommitted changes' if dirty else ''}"


class Snapshot:
    """A commit's files in a folder of their own, a git repository with
    nothing committed, which graff lists as it lists a worktree."""

    def __init__(self, repository, commit):
        self.folder = tempfile.mkdtemp(prefix="graff-nix-")
        archive = subprocess.Popen(["git", "-C", repository, "archive", commit], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", self.folder], stdin=archive.stdout, check=True)
        if archive.wait() != 0:
            raise RuntimeError("git archive failed")
        run(["git", "init", "-q"], cwd=self.folder)
        self.cache = tempfile.mkdtemp(prefix="graff-nix-cache-")
        self.paths = sorted(os.path.relpath(os.path.join(folder, name), self.folder)
                            for folder, _, names in os.walk(self.folder) if ".git" not in folder.split(os.sep)
                            for name in names if name.endswith(".nix"))

    def read(self, path):
        with open(os.path.join(self.folder, path), encoding="utf-8", errors="replace", newline="") as file:
            return file.read()

    def graff(self, binary, *args):
        """graff's answer as JSON, with no budget to cut it; None when graff
        names nothing so."""
        env = {**os.environ, "XDG_CACHE_HOME": self.cache}
        done = run([binary, *args, "--json", "--budget", "100000000"], cwd=self.folder, env=env, check=False)
        return json.loads(done.stdout) if done.returncode == 0 else None

    def edges(self, binary, broken=False):
        """examples/resolve.rs's edges over every Nix file."""
        command = [binary, self.folder] + (["--broken"] if broken else [])
        return [json.loads(line) for line in run(command, stdin="\n".join(self.paths) + "\n").stdout.splitlines()]

    def extractions(self, binary):
        """examples/extract.rs's extraction of every Nix file, by path."""
        done = run([binary], cwd=self.folder, stdin="\n".join(self.paths) + "\n")
        return {item["path"]: item["extraction"] for item in map(json.loads, done.stdout.splitlines())}

    def close(self):
        shutil.rmtree(self.folder, ignore_errors=True)
        shutil.rmtree(self.cache, ignore_errors=True)


def ratio(numerator, denominator):
    return numerator / denominator if denominator else 0.0


def private(name, lines):
    """Writes what names the corpus's own files and options where git does
    not keep it."""
    os.makedirs(PRIVATE, exist_ok=True)
    path = os.path.join(PRIVATE, name)
    with open(path, "w") as out:
        out.write("\n".join(lines) + "\n")
    return path
