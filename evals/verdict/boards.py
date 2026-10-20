"""A board's items as ekko keeps them, for the harness and the checks (task
693): the reading half of ekko's evals/boards.py.

From ekko 0.46.0 a board lives in storage/board.sqlite, and storage.json
beside it is only a stamp every watcher follows: {"0": "An ekko from before
boards were kept in SQLite cannot read this board: ..."}. A board no such
ekko has written to yet is storage.json alone, and its first write moves it
into the database. So nothing here opens a board's files but this module,
which reads either form. The database is read through SQLite, read-only, so
what its write-ahead log holds is read too, and its layout (user_version) is
checked: a later layout is refused rather than misread.
"""

import json
import os
import sqlite3

DATABASE = "board.sqlite"
# The layout these functions know: the database's user_version.
LAYOUT = 1
# What reading a board can raise: no board, a stamp with no database, a
# layout from a later ekko, a database SQLite cannot read.
ERRORS = (OSError, ValueError, sqlite3.Error)


def database_of(board):
    return os.path.join(board, "storage", DATABASE)


def is_stamp(items):
    """Whether storage.json held ekko's stamp rather than a board."""
    return isinstance(items, dict) and list(items) == ["0"] and isinstance(items["0"], str)


def _connect(path):
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    layout = conn.execute("PRAGMA user_version").fetchone()[0]
    if layout > LAYOUT:
        conn.close()
        raise ValueError(f"{path} has layout {layout}, from an ekko later than these evals, which know layout {LAYOUT}")
    return conn


def _items_in(database):
    conn = _connect(database)
    try:
        return {str(id): json.loads(body) for id, body in conn.execute("SELECT id, body FROM items ORDER BY id")}
    finally:
        conn.close()


def read(board):
    """The items of the board whose .ekko folder is `board`, as storage.json
    held them: by display id, as a string."""
    database = database_of(board)
    if os.path.exists(database):
        return _items_in(database)
    with open(os.path.join(board, "storage", "storage.json")) as handle:
        items = json.load(handle)
    if is_stamp(items):
        # The board moved into its database between the two looks.
        if os.path.exists(database):
            return _items_in(database)
        raise ValueError(f"{board}/storage/storage.json says the board is kept in {DATABASE}, which is missing")
    return items


def counters(board):
    """The board's counters, as counters.json held them."""
    database = database_of(board)
    if os.path.exists(database):
        conn = _connect(database)
        try:
            row = conn.execute("SELECT value FROM meta WHERE key = 'counters'").fetchone()
        finally:
            conn.close()
        return json.loads(row[0]) if row and row[0] else {}
    try:
        with open(os.path.join(board, "storage", "counters.json")) as handle:
            return json.load(handle)
    except FileNotFoundError:
        # The board moved into its database between the two looks.
        if os.path.exists(database):
            return counters(board)
        raise
