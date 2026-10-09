"""Writes src/resolve/python_methods.txt: the names of the methods Python's
own types have, which graff's resolver does not tie by name alone to a
method of the worktree (a call `x.get()` on a dict is not the worktree's
`get`). A name is in when a class of the standard library has a method of
that name: one written in Python, read off the library's source with `ast`
-- its tests left out -- or one written in C, found by `dir()` on the
classes of the builtins and of the modules built into the interpreter or
shipped as its extensions. Names private to a class, `_x` but not `__x__`,
are left out: a call of one is the worktree's own.

    nix develop -c python3 evals/python/builtin_methods.py

It reads the library of the Python it runs on.
"""

import ast
import builtins
import importlib
import os
import sys
import sysconfig

HERE = os.path.dirname(os.path.abspath(__file__))
GRAFF = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(GRAFF, "src", "resolve", "python_methods.txt")

# Folders of the library that hold its tests, its demos and IDLE, not its types.
LEFT_OUT = {"test", "tests", "idlelib", "turtledemo", "__phello__", "site-packages"}

# Modules that do something on import.
NOT_IMPORTED = {"antigravity", "this", "idlelib", "turtle", "tkinter", "_tkinter"}


def public(name):
    return not name.startswith("_") or (name.startswith("__") and name.endswith("__"))


def from_source(stdlib):
    """The methods of the classes the library writes in Python."""
    found = set()
    for folder, subfolders, files in os.walk(stdlib):
        subfolders[:] = sorted(s for s in subfolders if s not in LEFT_OUT)
        for name in sorted(files):
            if not name.endswith(".py"):
                continue
            path = os.path.join(folder, name)
            try:
                with open(path, "rb") as f:
                    tree = ast.parse(f.read(), path)
            except (SyntaxError, ValueError):
                continue
            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef):
                    for item in node.body:
                        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                            found.add(item.name)
    return found


def methods(cls):
    found = set()
    for name in dir(cls):
        try:
            value = getattr(cls, name)
        except Exception:
            continue
        if callable(value):
            found.add(name)
    return found


def from_c(stdlib):
    """The methods of the classes the builtins and the library's C modules hold."""
    found = set()
    for value in vars(builtins).values():
        if isinstance(value, type):
            found |= methods(value)
    names = set(sys.builtin_module_names)
    dynload = os.path.join(stdlib, "lib-dynload")
    if os.path.isdir(dynload):
        names |= {n.split(".")[0] for n in os.listdir(dynload) if n.endswith(".so")}
    for name in sorted(names - NOT_IMPORTED):
        try:
            module = importlib.import_module(name)
        except Exception:
            continue
        for value in list(vars(module).values()):
            if isinstance(value, type):
                found |= methods(value)
    return found


def main():
    stdlib = sysconfig.get_paths()["stdlib"]
    names = sorted(n for n in from_source(stdlib) | from_c(stdlib) if public(n))
    version = sys.version.split()[0]
    with open(OUT, "w") as f:
        f.write(f"# Written by evals/python/builtin_methods.py from the library of Python {version}.\n")
        for name in names:
            f.write(name + "\n")
    print(f"{len(names)} names to {os.path.relpath(OUT, GRAFF)}")


if __name__ == "__main__":
    main()
