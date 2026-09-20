"""Tests for minicode.storage (SqliteStore) — plain sync pytest, tmp_path-backed."""

from __future__ import annotations

import json
import sqlite3

import pytest

from minicode.core.models import (
    EventType,
    Message,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
)
from minicode.storage import DEFAULT_DB_PATH, SCHEMA_VERSION, SessionSummary, SqliteStore


@pytest.fixture()
def store(tmp_path):
    """A store over a throwaway db file, closed after the test."""
    s = SqliteStore(tmp_path / "sessions.db")
    yield s
    s.close()


# ---------------------------------------------------------------------------
# Sessions
# ---------------------------------------------------------------------------


def test_create_and_get_session_roundtrip(store):
    session_id = store.create_session(workspace="/tmp/ws", provider="anthropic", model="claude-x")
    summary = store.get_session(session_id)

    assert isinstance(summary, SessionSummary)
    assert summary.session_id == session_id
    assert summary.workspace == "/tmp/ws"
    assert summary.provider == "anthropic"
    assert summary.model == "claude-x"
    assert summary.status == "running"
    assert summary.exit_reason is None
    assert summary.rounds == 0
    assert summary.input_tokens == 0
    assert summary.output_tokens == 0
    # created_at is a UTC ISO-8601 string with an explicit offset
    assert summary.created_at.endswith("+00:00")

    assert store.get_session("does-not-exist") is None


def test_list_sessions_newest_first(store):
    # Ordering relies on `created_at DESC, rowid DESC` inside list_sessions, so
    # it stays deterministic even if both creates land on the same clock tick
    # (Windows timer granularity).
    first = store.create_session(workspace="w1", provider="p", model="m")
    second = store.create_session(workspace="w2", provider="p", model="m")

    listed = store.list_sessions()
    assert [s.session_id for s in listed] == [second, first]


def test_default_db_path_constant():
    assert DEFAULT_DB_PATH.name == "sessions.db"
    assert DEFAULT_DB_PATH.parent.name == ".minicode"


# ---------------------------------------------------------------------------
# Messages
# ---------------------------------------------------------------------------


def test_message_roundtrip_all_block_kinds_and_ordering(store):
    session_id = store.create_session(workspace="w", provider="p", model="m")
    messages = [
        Message(role="user", content=[TextBlock(text="hello 你好 world")]),
        Message(
            role="assistant",
            content=[
                TextBlock(text="thinking..."),
                ToolUseBlock(
                    id="call_1",
                    name="bash",
                    input={"cmd": "ls -la", "nested": {"deep": [1, 2, 3], "k": "值"}},
                ),
            ],
        ),
        Message(
            role="user",
            content=[ToolResultBlock(tool_use_id="call_1", content="file list", is_error=True)],
        ),
    ]

    seqs = [store.append_message(session_id, m) for m in messages]
    assert seqs == [0, 1, 2]

    loaded = store.get_messages(session_id)
    assert loaded == messages  # pydantic models compare by field values
    assert [m.role for m in loaded] == ["user", "assistant", "user"]
    assert loaded[1].content[1].input["nested"]["deep"] == [1, 2, 3]
    assert loaded[2].content[0].is_error is True


def test_messages_content_column_is_block_json(store, tmp_path):
    """The storage format contract: content column is the JSON of the block list."""
    session_id = store.create_session(workspace="w", provider="p", model="m")
    store.append_message(session_id, Message(role="user", content=[TextBlock(text="raw")]))

    con = sqlite3.connect(tmp_path / "sessions.db")
    try:
        role, content = con.execute("SELECT role, content FROM messages").fetchone()
    finally:
        con.close()
    assert role == "user"
    assert json.loads(content) == [{"type": "text", "text": "raw"}]


# ---------------------------------------------------------------------------
# Events
# ---------------------------------------------------------------------------


def test_events_seq_monotonic_and_data_intact(store):
    session_id = store.create_session(workspace="w", provider="p", model="m")
    ev0 = store.append_event(session_id, EventType.SESSION_START)
    ev1 = store.append_event(
        session_id, EventType.TOOL_CALL_START, {"tool": "bash", "input": {"cmd": "echo 你好"}}
    )
    ev2 = store.append_event(session_id, EventType.SESSION_END, {"exit": "completed"})

    assert [e.seq for e in (ev0, ev1, ev2)] == [0, 1, 2]

    events = store.get_events(session_id)
    assert [e.seq for e in events] == [0, 1, 2]
    assert [e.type for e in events] == [
        EventType.SESSION_START,
        EventType.TOOL_CALL_START,
        EventType.SESSION_END,
    ]
    assert events[0].data == {}
    assert events[1].data == {"tool": "bash", "input": {"cmd": "echo 你好"}}
    assert events[2].data == {"exit": "completed"}
    for e in events:
        assert e.timestamp  # non-empty ISO timestamp string


def test_messages_and_events_have_independent_sequences(store):
    session_id = store.create_session(workspace="w", provider="p", model="m")
    assert store.append_message(session_id, Message(role="user", content=[TextBlock(text="a")])) == 0
    assert store.append_event(session_id, EventType.ROUND_START).seq == 0
    assert (
        store.append_message(session_id, Message(role="assistant", content=[TextBlock(text="b")])) == 1
    )
    assert store.append_event(session_id, EventType.ROUND_END).seq == 1

    assert [m.content[0].text for m in store.get_messages(session_id)] == ["a", "b"]
    assert [e.seq for e in store.get_events(session_id)] == [0, 1]


def test_append_to_unknown_session_raises(store):
    msg = Message(role="user", content=[TextBlock(text="x")])
    with pytest.raises(ValueError, match="unknown session"):
        store.append_message("ghost", msg)
    with pytest.raises(ValueError, match="unknown session"):
        store.append_event("ghost", EventType.ROUND_START)


# ---------------------------------------------------------------------------
# update_session
# ---------------------------------------------------------------------------


def test_update_session_partial_and_clear(store):
    session_id = store.create_session(workspace="w", provider="p", model="m")
    store.update_session(
        session_id,
        status="completed",
        exit_reason="completed",
        rounds=3,
        input_tokens=120,
        output_tokens=45,
    )
    s = store.get_session(session_id)
    assert (s.status, s.exit_reason, s.rounds, s.input_tokens, s.output_tokens) == (
        "completed",
        "completed",
        3,
        120,
        45,
    )
    assert (s.workspace, s.provider, s.model) == ("w", "p", "m")

    # partial update keeps the other fields
    store.update_session(session_id, rounds=5)
    s = store.get_session(session_id)
    assert s.rounds == 5
    assert s.exit_reason == "completed"
    assert s.input_tokens == 120

    # exit_reason=None explicitly clears it; plain-None params mean unchanged
    store.update_session(session_id, exit_reason=None)
    s = store.get_session(session_id)
    assert s.exit_reason is None
    assert s.status == "completed"
    assert s.rounds == 5

    # no-op update on a known session is allowed and changes nothing
    store.update_session(session_id)
    assert store.get_session(session_id).rounds == 5


def test_update_unknown_session_raises(store):
    with pytest.raises(ValueError, match="unknown session"):
        store.update_session("nope", rounds=1)


# ---------------------------------------------------------------------------
# Persistence across instances (reopen after "process exit") and versioning
# ---------------------------------------------------------------------------


def test_persistence_across_instances(tmp_path):
    db_path = tmp_path / "sessions.db"

    store1 = SqliteStore(db_path)
    session_id = store1.create_session(workspace="w", provider="anthropic", model="m")
    store1.append_message(session_id, Message(role="user", content=[TextBlock(text="hi")]))
    store1.append_event(session_id, EventType.ROUND_START, {"round": 0})
    store1.update_session(
        session_id, status="cancelled", exit_reason="cancelled", rounds=2,
        input_tokens=10, output_tokens=20,
    )
    store1.close()

    store2 = SqliteStore(db_path)
    try:
        summary = store2.get_session(session_id)
        assert summary.status == "cancelled"
        assert summary.exit_reason == "cancelled"
        assert (summary.rounds, summary.input_tokens, summary.output_tokens) == (2, 10, 20)

        assert store2.get_messages(session_id) == [
            Message(role="user", content=[TextBlock(text="hi")])
        ]

        events = store2.get_events(session_id)
        assert len(events) == 1
        assert events[0].type == EventType.ROUND_START
        assert events[0].data == {"round": 0}

        assert [s.session_id for s in store2.list_sessions()] == [session_id]
    finally:
        store2.close()


def test_fresh_db_has_current_schema_version(tmp_path):
    db_path = tmp_path / "sessions.db"
    store = SqliteStore(db_path)
    try:
        con = sqlite3.connect(db_path)
        try:
            assert con.execute("PRAGMA user_version").fetchone()[0] == SCHEMA_VERSION
        finally:
            con.close()
    finally:
        store.close()


def test_rejects_database_from_newer_version(tmp_path):
    db_path = tmp_path / "sessions.db"
    store = SqliteStore(db_path)
    store.close()

    con = sqlite3.connect(db_path)
    try:
        con.execute(f"PRAGMA user_version = {SCHEMA_VERSION + 1}")
        con.commit()
    finally:
        con.close()

    with pytest.raises(RuntimeError, match="newer version"):
        SqliteStore(db_path)
