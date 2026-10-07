"""Regression coverage for persistence and artifact edge cases found in audit."""

from __future__ import annotations

import asyncio
import io
import sqlite3

import pytest

from minicode.storage import ArtifactStore, SqliteStore


@pytest.fixture()
def store(tmp_path):
    with SqliteStore(tmp_path / "sessions.db") as value:
        yield value


def session(store):
    return store.create_session(workspace="workspace", provider="fake", model="fake")


def test_failed_commit_rolls_back_and_connection_remains_usable(store):
    store.conn.execute("PRAGMA foreign_keys = ON")
    store.conn.executescript(
        "CREATE TABLE parent (id INTEGER PRIMARY KEY);"
        "CREATE TABLE child (parent_id INTEGER REFERENCES parent(id) "
        "DEFERRABLE INITIALLY DEFERRED);"
    )
    with pytest.raises(sqlite3.IntegrityError):
        with store.transaction() as conn:
            conn.execute("INSERT INTO child VALUES (1)")
    assert not store.conn.in_transaction
    assert store.conn.execute("SELECT COUNT(*) FROM child").fetchone()[0] == 0
    with store.transaction() as conn:
        conn.execute("INSERT INTO parent VALUES (1)")
        conn.execute("INSERT INTO child VALUES (1)")
    assert not store.conn.in_transaction
    assert store.conn.execute("SELECT COUNT(*) FROM child").fetchone()[0] == 1


def test_nested_rollback_preserves_outer_transaction(store):
    with store.transaction() as conn:
        conn.execute("CREATE TABLE audit_values (value TEXT)")
        conn.execute("INSERT INTO audit_values VALUES ('outer')")
        with pytest.raises(ValueError):
            with store.transaction() as nested:
                nested.execute("INSERT INTO audit_values VALUES ('inner')")
                raise ValueError("abort nested write")
        conn.execute("INSERT INTO audit_values VALUES ('after')")
    assert [row[0] for row in store.conn.execute("SELECT value FROM audit_values")] == ["outer", "after"]


def test_binary_artifact_utf8_sample_may_end_mid_character(store, monkeypatch):
    monkeypatch.setattr("minicode.storage.artifacts.locale.getpreferredencoding", lambda _: "latin-1")
    original = "x" * 8191 + "汉🙂后续" * 50
    artifacts = ArtifactStore(store)
    sid = session(store)
    ref = asyncio.run(artifacts.spill_binary_stream(sid, "command_output", io.BytesIO(original.encode("utf-8"))))
    assert artifacts.read(sid, ref.artifact_id) == original
    assert ref.size == len(original)


@pytest.mark.parametrize("binary", [False, True])
def test_artifact_preserves_mixed_newlines_and_character_offsets(store, binary):
    original = "first\r\n第二行\nthird\rfourth"
    artifacts = ArtifactStore(store)
    sid = session(store)
    if binary:
        ref = asyncio.run(artifacts.spill_binary_stream(sid, "command_output", io.BytesIO(original.encode("utf-8"))))
    else:
        ref = artifacts.spill(sid, "tool_output", original)
    assert artifacts.read(sid, ref.artifact_id) == original
    assert ref.size == len(original)
    pages = [artifacts.read_page(sid, ref.artifact_id, offset, 3)[0] for offset in range(0, len(original), 3)]
    assert "".join(pages) == original
    assert artifacts.read_page(sid, ref.artifact_id, len(original), 3) == ("", len(original), False)
