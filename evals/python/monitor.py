"""Runs a Python script as its own `python3 SCRIPT` would, and writes down
each call made at a place in a folder's files: where it was made and what
it called, as sys.monitoring (Python 3.12+) sees it.

    python3 monitor.py OUT ROOT SCRIPT [ARGUMENT..]

Each line of OUT is a JSON object: the calling file, from ROOT, and the
place of the call's instruction there (line, end line, column, end column,
lines from 1 and columns from 0 in bytes of UTF-8, as code.co_positions
gives them), and
what it called: for a function or a class of ROOT's files, its file, its
qualified name and its first line; for anything else, `outside` and its
name. A function's wrappers, `functools.wraps`'s and `functools.cache`'s,
are seen through to the function they wrap, a bound method to its
function, and an object called to its class's `__call__`. Stopped by
SIGTERM, as a test run past its time is, it writes what it has and exits
with 124.
"""

import inspect
import json
import os
import runpy
import signal
import sys
import types

TOOL = sys.monitoring.DEBUGGER_ID


class Recorder:
    def __init__(self, root):
        self.root = os.path.realpath(root) + os.sep
        self.files = {}  # code's file -> its path from the root, or None
        self.positions = {}  # code -> its instructions' positions
        self.calls = set()

    def inside(self, filename):
        if filename not in self.files:
            full = os.path.realpath(filename) if filename and not filename.startswith("<") else ""
            self.files[filename] = full[len(self.root):] if full.startswith(self.root) else None
        return self.files[filename]

    def on_call(self, code, offset, called, arg0):
        path = self.inside(code.co_filename)
        if path is None:
            return sys.monitoring.DISABLE
        if code not in self.positions:
            self.positions[code] = list(code.co_positions())
        place = self.positions[code][offset // 2]
        if place[0] is None:
            return None
        self.calls.add((path, place, self.callee(called)))
        return None

    def callee(self, called):
        """(file, qualified name, first line) of what a call reached in the
        root's files, or (None, its name, None) outside them."""
        target = called
        if isinstance(target, types.MethodType):
            target = target.__func__
        try:
            target = inspect.unwrap(target)
        except (ValueError, TypeError):
            pass
        if isinstance(target, types.MethodType):
            target = target.__func__
        if isinstance(target, type):
            module = sys.modules.get(target.__module__)
            path = self.inside(getattr(module, "__file__", None) or "")
            line = getattr(target, "__firstlineno__", None)
            if path is not None and line is not None:
                return (path, target.__qualname__, line)
            return (None, target.__qualname__, None)
        if not isinstance(target, types.FunctionType):
            call = getattr(type(target), "__call__", None)
            if isinstance(call, types.FunctionType):
                target = call
        if isinstance(target, types.FunctionType):
            code = target.__code__
            path = self.inside(code.co_filename)
            if path is not None:
                return (path, code.co_qualname, code.co_firstlineno)
            return (None, code.co_qualname, None)
        name = getattr(target, "__qualname__", None) or getattr(target, "__name__", None) or type(target).__name__
        return (None, str(name), None)

    def write(self, out):
        with open(out, "a", encoding="utf-8") as file:
            for path, (line, end_line, column, end_column), (defined, name, first) in sorted(
                    self.calls, key=lambda c: (c[0], c[1][0] or 0, c[1][2] or 0, str(c[2]))):
                called = {"path": defined, "qualified": name, "line": first} if defined else {"outside": name}
                file.write(json.dumps({"path": path, "line": line, "end_line": end_line,
                                       "column": column, "end_column": end_column,
                                       "called": called}) + "\n")


def main():
    if len(sys.argv) < 4:
        sys.exit("usage: monitor.py OUT ROOT SCRIPT [ARGUMENT..]")
    out, root, script = sys.argv[1:4]
    script = os.path.abspath(script)
    sys.argv = [script] + sys.argv[4:]
    sys.path[0] = os.path.dirname(script)
    recorder = Recorder(root)
    sys.monitoring.use_tool_id(TOOL, "graff")
    sys.monitoring.register_callback(TOOL, sys.monitoring.events.CALL, recorder.on_call)
    sys.monitoring.set_events(TOOL, sys.monitoring.events.CALL)

    def stopped(signum, frame):
        # unittest takes a SystemExit raised in a test for the test's error
        # and runs on: write, and leave at once.
        sys.monitoring.set_events(TOOL, 0)
        recorder.write(out)
        os._exit(124)

    signal.signal(signal.SIGTERM, stopped)
    status = 0
    try:
        runpy.run_path(script, run_name="__main__")
    except SystemExit as stop:
        status = stop.code
    finally:
        sys.monitoring.set_events(TOOL, 0)
        recorder.write(out)
    sys.exit(status)


if __name__ == "__main__":
    main()
