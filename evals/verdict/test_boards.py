"""boards.py must read a board in either form ekko keeps it, and the harness
and the checks must read it through boards.py: each case on a board kept as
JSON and on the same board kept in a database, whose rows are still in its
write-ahead log, as ekko leaves them while a session runs; and the reads
that must say no, a stamp with no database and a later layout. The rule
over the class: no other file here names a board's files but to seed a
scratch board, which ekko reads as JSON until its first write.

    python3 evals/verdict/test_boards.py
"""

import json
import os
import shutil
import sqlite3
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import boards  # noqa: E402
import checks  # noqa: E402
import harness  # noqa: E402

# ekko's src/database.rs, layout 1.
SCHEMA = """
CREATE TABLE items (id INTEGER PRIMARY KEY, body TEXT NOT NULL);
CREATE TABLE meta (key TEXT PRIMARY KEY, value);
CREATE TABLE journal (seq INTEGER PRIMARY KEY, rev INTEGER NOT NULL, entry TEXT NOT NULL);
CREATE TABLE changes (seq INTEGER PRIMARY KEY, rev INTEGER NOT NULL, at INTEGER NOT NULL, id INTEGER NOT NULL, before TEXT, after TEXT);
"""
STAMP = {"0": "An ekko from before boards were kept in SQLite cannot read this board: upgrade ekko, then restart every Claude Code session."}
T0 = 1_792_000_000_000
ITEMS = {
    "1": {"_id": 1, "_timestamp": T0, "description": "Seed the board", "_isTask": True, "isComplete": False, "boards": ["My Board"]},
    "2": {"_id": 2, "_timestamp": T0 + 1000, "description": "Write the parser", "_isTask": True, "isComplete": True,
          "doneBy": {"since": T0 + 9000}, "boards": ["My Board"]},
    "3": {"_id": 3, "_timestamp": T0 + 5000, "description": "Where this stopped", "_isTask": False, "handoff": True, "boards": ["My Board"]},
}
COUNTERS = {"revision": 7, "highestId": 3}


def as_json(root):
    board = os.path.join(root, ".ekko")
    os.makedirs(os.path.join(board, "storage"))
    json.dump(ITEMS, open(os.path.join(board, "storage", "storage.json"), "w"), indent=4)
    json.dump(COUNTERS, open(os.path.join(board, "storage", "counters.json"), "w"))
    json.dump({"id": "p", "name": "proj"}, open(os.path.join(board, "project.json"), "w"))
    return board


def as_database(root, layout=1):
    """The board in a database, the writer left open so that its rows stay
    in the write-ahead log, and storage.json the stamp. Returns the writer."""
    board = os.path.join(root, ".ekko")
    os.makedirs(os.path.join(board, "storage"))
    writer = sqlite3.connect(boards.database_of(board))
    writer.execute("PRAGMA journal_mode=WAL")
    writer.executescript(SCHEMA)
    writer.execute(f"PRAGMA user_version = {layout}")
    writer.executemany("INSERT INTO items (id, body) VALUES (?, ?)", [(int(k), json.dumps(v)) for k, v in ITEMS.items()])
    writer.execute("INSERT INTO meta (key, value) VALUES ('counters', ?)", (json.dumps(COUNTERS),))
    writer.commit()
    json.dump(STAMP, open(os.path.join(board, "storage", "storage.json"), "w"))
    json.dump({"id": "p", "name": "proj"}, open(os.path.join(board, "project.json"), "w"))
    return board, writer


def main():
    failed = []
    root = tempfile.mkdtemp(prefix="graff-boards-")
    try:
        json_board = as_json(os.path.join(root, "json"))
        db_board, writer = as_database(os.path.join(root, "db"))
        wal = boards.database_of(db_board) + "-wal"
        if not os.path.getsize(wal):
            failed.append("the fixture's rows are not in the write-ahead log")
        for form, board in (("json", json_board), ("database", db_board)):
            try:
                if boards.read(board) != ITEMS:
                    failed.append(f"read, {form}: not the board's items")
                if boards.counters(board) != COUNTERS:
                    failed.append(f"counters, {form}: not the board's counters")
            except Exception as error:  # noqa: BLE001
                failed.append(f"read, {form}: {type(error).__name__}: {error}")

            repo = os.path.dirname(board)
            try:
                if harness.handoffs_since(repo, T0 // 1000) != [3]:
                    failed.append(f"handoffs_since, {form}: want [3]")
            except Exception as error:  # noqa: BLE001
                failed.append(f"handoffs_since, {form}: {type(error).__name__}: {error}")

            dest = os.path.join(root, f"snapshot-{form}")
            try:
                kept, newer = harness.snapshot(board, T0 + 4000, {1}, dest)
                seeded = json.load(open(os.path.join(dest, "storage", "storage.json")))
                if kept != 2 or sorted(seeded) != ["1", "2"] or seeded["2"]["isComplete"] or "doneBy" in seeded["2"]:
                    failed.append(f"snapshot, {form}: want items 1 and 2, 2 reopened; got {sorted(seeded)}")
                if json.load(open(os.path.join(dest, "storage", "counters.json"))) != {"revision": 7, "highestId": 2}:
                    failed.append(f"snapshot, {form}: counters not the board's, highestId not 2")
            except Exception as error:  # noqa: BLE001
                failed.append(f"snapshot, {form}: {type(error).__name__}: {error}")

            was = checks.BOARDS
            checks.BOARDS = {"proj": board}
            try:
                spec = checks.spec_of({"board": "proj", "prompt": "do task 2"})
                if "Task 2: Write the parser" not in spec:
                    failed.append(f"spec_of, {form}: task 2's text missing")
            except Exception as error:  # noqa: BLE001
                failed.append(f"spec_of, {form}: {type(error).__name__}: {error}")
            finally:
                checks.BOARDS = was
        writer.close()

        # Must say no: a stamp with no database beside it, and a layout from a
        # later ekko.
        os.remove(boards.database_of(db_board))
        try:
            boards.read(db_board)
            failed.append("a stamp with no database read as a board")
        except ValueError:
            pass
        later, writer = as_database(os.path.join(root, "later"), layout=2)
        writer.close()
        try:
            boards.read(later)
            failed.append("layout 2 read as layout 1")
        except ValueError:
            pass
    finally:
        shutil.rmtree(root, ignore_errors=True)

    # The class: every line of this folder's code that names storage.json,
    # counters.json or board.sqlite, outside boards.py and the tests, must be
    # the snapshot writing its scratch board.
    here = os.path.dirname(os.path.abspath(__file__))
    for name in sorted(os.listdir(here)):
        if not name.endswith(".py") or name == "boards.py" or name.startswith("test_"):
            continue
        for number, line in enumerate(open(os.path.join(here, name)), 1):
            named = any(f'"{file}"' in line for file in ("storage.json", "counters.json", "board.sqlite"))
            if named and not ("os.path.join(dest, " in line and '"w")' in line):
                failed.append(f"{name}:{number} reads a board's file, not through boards.py: {line.strip()[:90]}")

    for line in failed:
        print("FAIL " + line)
    print(f"boards in both forms, through boards.py, the harness and the checks: {'ok' if not failed else f'{len(failed)} failed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
