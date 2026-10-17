"""Writes src/resolve/python_methods.txt: the names of the methods Python's
own types have, which graff's resolver does not tie by name alone to a
method of the worktree (a call `x.get()` on a dict is not the worktree's
`get`). A name is in when a class of the standard library has a method of
that name: one written in Python, read off the library's source with `ast`
-- its tests left out -- or one written in C, found by `dir()` on the
classes of the builtins and of the modules built into the interpreter or
shipped as its extensions, or of the types a module written in Python
binds at its top with `X = type(..)`, as re's `Pattern` and `Match`. Names private to a class, `_x` but not `__x__`,
are left out: a call of one is the worktree's own.

It writes src/resolve/python_stars.txt too: what `from M import *` binds
for each module of the library written in C, the names its `__all__` lists
or else those `dir()` gives that do not start with `_`, which the resolver
reads as what the module may bind in place of a definition of the
worktree, as `from _heapq import *` binds heappush over heapq.py's.

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
STARS = os.path.join(GRAFF, "src", "resolve", "python_stars.txt")

# Folders of the library that hold its tests, its demos and IDLE, not its types.
LEFT_OUT = {"test", "tests", "idlelib", "turtledemo", "__phello__", "site-packages"}

# Modules that do something on import.
NOT_IMPORTED = {"antigravity", "this", "idlelib", "turtle", "tkinter", "_tkinter"}


def public(name):
    return not name.startswith("_") or (name.startswith("__") and name.endswith("__"))


def from_source(stdlib):
    """The methods of the classes the library writes in Python, and the
    types its modules bind at their top that are no class of its source,
    as (module, name)."""
    found = set()
    bound = set()
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
                    found |= defined(node.body)
            module = module_name(stdlib, path)
            for node in tree.body:
                if is_type_binding(node):
                    bound |= {(module, t.id) for t in node.targets if public(t.id)}
    return found, bound


def module_name(stdlib, path):
    parts = os.path.relpath(path, stdlib)[: -len(".py")].split(os.sep)
    if parts[-1] == "__init__":
        parts.pop()
    return ".".join(parts)


def is_type_binding(node):
    """`X = type(..)` at the top of a module."""
    return (
        isinstance(node, ast.Assign)
        and isinstance(node.value, ast.Call)
        and isinstance(node.value.func, ast.Name)
        and node.value.func.id == "type"
        and len(node.value.args) == 1
        and all(isinstance(t, ast.Name) for t in node.targets)
    )


def defined(body):
    """The functions a class's body defines, in the branches of its `if`
    and `try` statements too: pathlib's `Path.group` under `if grp:`."""
    found = set()
    for item in body:
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
            found.add(item.name)
        elif isinstance(item, (ast.If, ast.For, ast.While, ast.With)):
            found |= defined(item.body) | defined(getattr(item, "orelse", []))
        elif isinstance(item, ast.Try):
            found |= defined(item.body) | defined(item.orelse) | defined(item.finalbody)
            for handler in item.handlers:
                found |= defined(handler.body)
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


def c_modules(stdlib):
    """The library's modules written in C: those built into the interpreter
    and those shipped as its extensions, each imported, by name."""
    names = set(sys.builtin_module_names)
    dynload = os.path.join(stdlib, "lib-dynload")
    if os.path.isdir(dynload):
        names |= {n.split(".")[0] for n in os.listdir(dynload) if n.endswith(".so")}
    modules = {}
    for name in sorted(names - NOT_IMPORTED):
        try:
            modules[name] = importlib.import_module(name)
        except Exception:
            continue
    return modules


def starred(module):
    """What `from module import *` binds."""
    names = getattr(module, "__all__", None)
    if names is None:
        names = [n for n in dir(module) if not n.startswith("_")]
    return sorted(set(names))


def from_c(stdlib, bound):
    """The methods of the classes the builtins and the library's C modules
    hold, and of the types `bound` names in the modules that bind them."""
    found = set()
    for value in vars(builtins).values():
        if isinstance(value, type):
            found |= methods(value)
    for module in c_modules(stdlib).values():
        for value in list(vars(module).values()):
            if isinstance(value, type):
                found |= methods(value)
    for name, attribute in sorted(bound):
        if name in NOT_IMPORTED:
            continue
        try:
            value = getattr(importlib.import_module(name), attribute)
        except Exception:
            continue
        if isinstance(value, type):
            found |= methods(value)
    return found


def main():
    stdlib = sysconfig.get_paths()["stdlib"]
    written, bound = from_source(stdlib)
    names = sorted(n for n in written | from_c(stdlib, bound) if public(n))
    version = sys.version.split()[0]
    with open(OUT, "w") as f:
        f.write(f"# Written by evals/python/builtin_methods.py from the library of Python {version}.\n")
        for name in names:
            f.write(name + "\n")
    print(f"{len(names)} names to {os.path.relpath(OUT, GRAFF)}")
    modules = c_modules(stdlib)
    with open(STARS, "w") as f:
        f.write(f"# Written by evals/python/builtin_methods.py from the library of Python {version}:\n")
        f.write("# a module written in C, then what `from` it `import *` binds.\n")
        for name, module in sorted(modules.items()):
            f.write(" ".join([name] + starred(module)) + "\n")
    print(f"{len(modules)} modules to {os.path.relpath(STARS, GRAFF)}")


if __name__ == "__main__":
    main()
