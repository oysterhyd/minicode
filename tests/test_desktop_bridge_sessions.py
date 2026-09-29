"""Desktop sidebar session management and per-conversation "always allow"."""

from __future__ import annotations

import asyncio
import sqlite3
import sys
from pathlib import Path

import pytest

from minicode.core.models import EventType, Message, TextBlock
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.storage import SqliteStore
from minicode.storage.artifacts import ArtifactStore
from minicode.tasks.taskstore import TaskStore

DESKTOP = Path(__file__).resolve().parents[1] / "desktop"
if str(DESKTOP) not in sys.path:
    sys.path.insert(0, str(DESKTOP))

import bridge as bridge_mod  # noqa: E402


@pytest.fixture()
def workspace(tmp_path):
    root = tmp_path / "ws"
    root.mkdir()
    return root


@pytest.fixture()
def store(tmp_path):
    opened = SqliteStore(tmp_path / "sessions.sqlite3")
    yield opened
    opened.close()


@pytest.fixture()
def events(monkeypatch):
    captured: list[dict] = []
    monkeypatch.setattr(bridge_mod, "emit", captured.append)
    return captured


def _session(store: SqliteStore, workspace: Path, text: str = "first prompt") -> str:
    session_id = store.create_session(workspace=str(workspace), provider="fake", model="fake")
    store.append_message(session_id, Message(role="user", content=[TextBlock(text=text)]))
    return session_id


def test_rename_and_pin_sessions(store, workspace, events):
    session_id = _session(store, workspace, "fix the tests\nmore detail")

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        rows = await router.handle("renameSession", {"sessionId": session_id, "title": "  " + "x" * 200})
        row = next(r for r in rows if r["session_id"] == session_id)
        assert row["title"] == "x" * 120 and row["custom_title"] is True

        rows = await router.handle("renameSession", {"sessionId": session_id, "title": ""})
        row = next(r for r in rows if r["session_id"] == session_id)
        assert row["title"] == "fix the tests" and row["custom_title"] is False

        rows = await router.handle("pinSession", {"sessionId": session_id, "pinned": True})
        assert next(r for r in rows if r["session_id"] == session_id)["pinned"] is True
        rows = await router.handle("pinSession", {"sessionId": session_id, "pinned": False})
        assert next(r for r in rows if r["session_id"] == session_id)["pinned"] is False
        # Sidebar management never creates a conversation Bridge.
        assert router.clients == {} and router.sessions == {}

        with pytest.raises(ValueError, match="找不到会话"):
            await router.handle("renameSession", {"sessionId": "missing", "title": "x"})

    asyncio.run(scenario())


def test_updated_at_tracks_latest_event(store, workspace):
    session_id = _session(store, workspace)
    row = next(r for r in bridge_mod.session_list(store) if r["session_id"] == session_id)
    assert row["updated_at"] == row["created_at"]
    event = store.append_event(session_id, EventType.ROUND_START, {"round": 1})
    row = next(r for r in bridge_mod.session_list(store) if r["session_id"] == session_id)
    assert row["updated_at"] == event.timestamp


def test_delete_session_removes_rows_and_artifacts(store, workspace, events):
    session_id = _session(store, workspace)
    keep = _session(store, workspace, "keep me")
    store.append_event(session_id, EventType.ROUND_START, {})
    store.set_session_pinned(session_id, True)
    TaskStore(store).add(session_id, "t1", "task")
    ref = ArtifactStore(store).spill(session_id, "out", "hello")
    artifact_dir = ArtifactStore(store)._session_dir(session_id)
    assert (artifact_dir / f"{ref.artifact_id}.txt").exists()

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        await router.handle("selectSession", {"sessionId": session_id, "clientKey": session_id})
        assert session_id in router.sessions
        rows = await router.handle("deleteSession", {"sessionId": session_id})
        assert [r["session_id"] for r in rows] == [keep]
        assert session_id not in router.sessions and session_id not in router.recent
        assert all(c.runtime is None or c.runtime.session_id != session_id for c in router.clients.values())
        await router.aclose()

    asyncio.run(scenario())
    reopened = SqliteStore(store._db_path)
    try:
        for table in ("sessions", "messages", "events", "artifacts", "session_meta", "tasks"):
            count = reopened.conn.execute(
                f"SELECT COUNT(*) FROM {table} WHERE session_id = ?", (session_id,)).fetchone()[0]
            assert count == 0, table
        assert reopened.get_session(keep) is not None
    finally:
        reopened.close()
    assert not artifact_dir.exists()


def _bash_twice():
    def factory(model: str, effort: str = "off"):
        return FakeProvider(FakeProviderOptions(turns=[
            FakeTurn(text="", tool_calls=[FakeToolCall(name="bash", arguments={"command": "echo one", "cwd": "."})]),
            FakeTurn(text="", tool_calls=[FakeToolCall(name="bash", arguments={"command": "echo two", "cwd": "."})]),
            FakeTurn(text="done"),
        ]))
    return factory


async def _wait(predicate, timeout: float = 20.0):
    for _ in range(int(timeout / 0.05)):
        if predicate():
            return True
        await asyncio.sleep(0.05)
    return False


def test_deleting_a_running_session_is_refused(monkeypatch, store, workspace, events):
    monkeypatch.setattr(bridge_mod, "provider_for", _bash_twice())

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            started = await router.handle("sendPrompt", {
                "text": "go", "workspace": str(workspace), "model": "fake", "clientKey": "draft-0"})
            session_id = started["sessionId"]
            assert await _wait(lambda: any(e.get("event") == "approval" for e in events))
            with pytest.raises(ValueError, match="任务运行中"):
                await router.handle("deleteSession", {"sessionId": session_id})
            assert store.get_session(session_id) is not None
            await router.handle("cancelTurn", {"sessionId": session_id, "clientKey": "draft-0"})
            assert await _wait(lambda: not router.sessions[session_id].busy())
            await router.handle("deleteSession", {"sessionId": session_id})
            assert store.get_session(session_id) is None
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_always_allow_for_session(monkeypatch, store, workspace, events):
    monkeypatch.setattr(bridge_mod, "provider_for", _bash_twice())

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            started = await router.handle("sendPrompt", {
                "text": "go", "workspace": str(workspace), "model": "fake", "clientKey": "draft-0"})
            session_id = started["sessionId"]
            assert await _wait(lambda: any(e.get("event") == "approval" for e in events))
            approval = next(e for e in events if e.get("event") == "approval")
            assert await router.handle("resolveApproval", {
                "approvalId": approval["approvalId"], "granted": True, "remember": "session",
                "sessionId": session_id, "clientKey": "draft-0"}) is True
            bridge = router.sessions[session_id]
            assert await _wait(lambda: not bridge.busy())

            assert [e["event"] for e in events if e.get("event", "").startswith("approval")] == \
                ["approval", "approval_auto"]
            auto = next(e for e in events if e.get("event") == "approval_auto")
            assert auto["request"]["tool_name"] == "bash" and auto["sessionId"] == session_id
            assert auto["clientKey"] == "draft-0"
            done = next(e for e in events if e.get("event") == "run_done")
            assert done["result"]["exit_reason"] == "completed"

            state = await router.handle("getState", {"sessionId": session_id, "clientKey": "draft-0"})
            assert state["alwaysAllow"] == ["bash"]

            # A new conversation copied from this one does not inherit the grant.
            fresh = await router.handle("resetSession", {"clientKey": "draft-1", "sourceClientKey": "draft-0"})
            assert fresh["alwaysAllow"] == []

            cleared = await router.handle("clearAlwaysAllow", {"sessionId": session_id, "clientKey": "draft-0"})
            assert cleared["alwaysAllow"] == []
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_denied_remember_does_not_allow(store, workspace, events):
    bridge = bridge_mod.Bridge(store)

    async def scenario():
        loop = asyncio.get_running_loop()
        bridge.approvals["s:1"] = loop.create_future()
        bridge.approval_tools["s:1"] = "bash"
        assert await bridge.handle("resolveApproval", {"approvalId": "s:1", "granted": False, "remember": "session"})
        assert bridge.always_allow == set()

    asyncio.run(scenario())


def test_old_database_gains_session_meta(tmp_path, workspace):
    """A database created before session_meta opens and works unchanged."""
    path = tmp_path / "old.sqlite3"
    old = SqliteStore(path)
    session_id = _session(old, workspace)
    old.conn.execute("DROP TABLE session_meta")
    old.conn.commit()
    old.close()
    assert "session_meta" not in {r[0] for r in sqlite3.connect(path).execute(
        "SELECT name FROM sqlite_master WHERE type='table'")}

    reopened = SqliteStore(path)
    try:
        row = next(r for r in bridge_mod.session_list(reopened) if r["session_id"] == session_id)
        assert row["title"] == "first prompt" and row["pinned"] is False
        reopened.set_session_title(session_id, "Renamed")
        assert reopened.session_meta()[session_id] == {"title": "Renamed", "pinned": False}
        reopened.delete_session(session_id)  # works before TaskStore ever created `tasks`
        assert reopened.get_session(session_id) is None
    finally:
        reopened.close()
