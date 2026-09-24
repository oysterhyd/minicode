"""Regression tests for per-conversation run isolation in the desktop bridge.

Two reported desktop bugs are covered here:

* a conversation whose task is running could not be switched away from, and
  switching back raised ``当前任务仍在运行`` — the bridge held ONE global run
  slot, so any second conversation had to wait for the first to finish;
* events from a running conversation had no way to say which conversation they
  belonged to, so a switch would have written another conversation's output into
  the visible one.

``BridgeRouter`` gives every conversation its own :class:`Bridge` (its own
runtime, provider and run task) and tags each event with the ``clientKey`` that
started it. These tests drive the real router with a scripted provider, so a run
is genuinely in flight while the switch happens.
"""

from __future__ import annotations

import asyncio
import os
import sys
import time
from pathlib import Path

import pytest

from minicode.core.models import ApprovalDecision
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.storage import SqliteStore

DESKTOP = Path(__file__).resolve().parents[1] / "desktop"
if str(DESKTOP) not in sys.path:
    sys.path.insert(0, str(DESKTOP))

import bridge as bridge_mod  # noqa: E402  (needs the desktop path above)


def _sleep_command(seconds: float) -> str:
    """A command that occupies the shell for *seconds*, on either platform."""
    exe = sys.executable.replace("\\", "/")
    snippet = f"import time; time.sleep({seconds})"
    if os.name == "nt":
        return f'& "{exe}" -c "{snippet}"' if " " in exe else f'{exe} -c "{snippet}"'
    return f'"{exe}" -c "{snippet}"'


@pytest.fixture()
def workspace(tmp_path):
    root = tmp_path / "ws"
    root.mkdir()
    return root


@pytest.fixture()
def store(tmp_path):
    opened = SqliteStore(tmp_path / "bridge-test.sqlite3")
    yield opened
    opened.close()


@pytest.fixture()
def events(monkeypatch):
    """Capture what the bridge would print to Electron."""
    captured: list[dict] = []
    monkeypatch.setattr(bridge_mod, "emit", captured.append)
    return captured


def scripted(sleep_s: float = 2.0):
    """Build a provider whose first turn calls bash and whose second ends."""

    def factory(model: str, effort: str = "off"):
        return FakeProvider(FakeProviderOptions(turns=[
            FakeTurn(text="", tool_calls=[FakeToolCall(
                name="bash", arguments={"command": _sleep_command(sleep_s), "cwd": "."})]),
            FakeTurn(text="finished"),
        ]))

    return factory


async def approve_pending(router, events, *, rounds: int = 400) -> None:
    """Approve every approval request the way the UI would, own channel included."""
    seen: set[str] = set()
    for _ in range(rounds):
        await asyncio.sleep(0.05)
        for message in list(events):
            if message.get("event") != "approval":
                continue
            approval_id = message["approvalId"]
            if approval_id in seen:
                continue
            seen.add(approval_id)
            owner = message.get("sessionId") or approval_id.split(":", 1)[0]
            await router.handle("resolveApproval", {
                "approvalId": approval_id, "granted": True,
                "sessionId": owner, "clientKey": message.get("clientKey") or "draft-0",
            })


async def wait_done(task: asyncio.Task, timeout: float = 45.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and not task.done():
        await asyncio.sleep(0.05)
    return task.done()


def test_switching_conversations_while_a_task_runs(monkeypatch, store, workspace, events):
    """A running conversation can be left and re-entered; its run still finishes."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(2.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            # One approver for the whole scenario: every scripted turn calls bash.
            approver = asyncio.create_task(approve_pending(router, events))

            # A second, idle conversation to switch to.
            other = (await router.handle("sendPrompt", {
                "text": "hello", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "other"}))["sessionId"]
            other_bridge = router.sessions[other]
            assert await wait_done(other_bridge.run_task)

            started = await router.handle("sendPrompt", {
                "text": "slow task", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"})
            running_id = started["sessionId"]
            running_bridge = router.sessions[running_id]
            assert not running_bridge.run_task.done()
            await asyncio.sleep(0.8)  # let the tool call actually start
            assert not running_bridge.run_task.done(), "run should still be in flight"

            # 1. Leaving a running conversation must not wait for it.
            at = time.monotonic()
            state = await router.handle("selectSession", {
                "sessionId": other, "workspace": str(workspace), "clientKey": other})
            assert time.monotonic() - at < 2.0, "switch blocked while a task was running"
            assert state["sessionId"] == other
            assert not running_bridge.run_task.done(), "switching cancelled the run"
            assert router.clients[other] is other_bridge, "switch reused the running bridge"

            # 2. Re-entering the running conversation returns ITS state instead of
            #    raising 当前任务仍在运行.
            back = await router.handle("selectSession", {
                "sessionId": running_id, "workspace": str(workspace), "clientKey": "draft-0"})
            assert back["sessionId"] == running_id
            assert router.clients["draft-0"] is running_bridge

            # 3. Each conversation's requests reach its own runtime.
            mine = await router.handle("getState", {"sessionId": running_id, "clientKey": "draft-0"})
            theirs = await router.handle("getState", {"sessionId": other, "clientKey": other})
            assert mine["sessionId"] == running_id
            assert theirs["sessionId"] == other

            # 4. The run survives the switching and completes.
            assert await wait_done(running_bridge.run_task), "run never finished after switching"

            # 5. Its events stay attributed to the conversation that started them.
            await asyncio.sleep(0.3)
            mine_events = [e for e in events if e.get("sessionId") == running_id]
            assert {e.get("clientKey") for e in mine_events} == {"draft-0"}
            assert any(e.get("event") == "run_done" for e in mine_events)
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_two_conversations_run_at_the_same_time(monkeypatch, store, workspace, events):
    """Two conversations are genuinely independent: both run, neither blocks."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(1.5))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            first = (await router.handle("sendPrompt", {
                "text": "first", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"}))["sessionId"]
            second = (await router.handle("sendPrompt", {
                "text": "second", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-1"}))["sessionId"]
            assert first != second
            assert router.sessions[first] is not router.sessions[second]

            approver = asyncio.create_task(approve_pending(router, events))
            first_bridge = router.sessions[first]
            second_bridge = router.sessions[second]
            assert await wait_done(first_bridge.run_task)
            assert await wait_done(second_bridge.run_task)

            await asyncio.sleep(0.3)
            for session_id, key in ((first, "draft-0"), (second, "draft-1")):
                owned = [e for e in events if e.get("sessionId") == session_id]
                assert owned and {e.get("clientKey") for e in owned} == {key}
                assert any(e.get("event") == "run_done" for e in owned)
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_a_second_prompt_on_the_same_conversation_is_rejected(monkeypatch, store, workspace, events):
    """Guarding one conversation's run must not guard the whole bridge."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(2.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            started = await router.handle("sendPrompt", {
                "text": "slow", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"})
            session_id = started["sessionId"]
            approver = asyncio.create_task(approve_pending(router, events))
            await asyncio.sleep(0.8)

            # The same conversation cannot start a second overlapping run...
            with pytest.raises(ValueError, match="仍在运行"):
                await router.handle("sendPrompt", {
                    "text": "again", "workspace": str(workspace), "model": "fake",
                    "sessionId": session_id, "clientKey": "draft-0"})

            # ...but a different conversation still can.
            other = await router.handle("sendPrompt", {
                "text": "elsewhere", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-9"})
            assert other["sessionId"] != session_id

            assert await wait_done(router.sessions[session_id].run_task)
            assert await wait_done(router.sessions[other["sessionId"]].run_task)
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_cancel_only_affects_its_own_conversation(monkeypatch, store, workspace, events):
    """Stopping one conversation leaves the other one running."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(3.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            first = (await router.handle("sendPrompt", {
                "text": "first", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"}))["sessionId"]
            second = (await router.handle("sendPrompt", {
                "text": "second", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-1"}))["sessionId"]
            approver = asyncio.create_task(approve_pending(router, events))
            await asyncio.sleep(1.2)

            await router.handle("cancelTurn", {"sessionId": first, "clientKey": "draft-0"})
            assert await wait_done(router.sessions[first].run_task)
            assert not router.sessions[second].run_task.done(), "cancel hit the wrong conversation"
            assert await wait_done(router.sessions[second].run_task)
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_aclose_stops_every_conversation(monkeypatch, store, workspace, events):
    """Closing the app cancels all runs and survives conversations without one."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(5.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        await router.handle("initialize", {"clientKey": "draft-0"})
        started = (await router.handle("sendPrompt", {
            "text": "slow", "workspace": str(workspace), "model": "fake",
            "sessionId": None, "clientKey": "draft-0"}))["sessionId"]
        approver = asyncio.create_task(approve_pending(router, events))
        await asyncio.sleep(0.8)
        bridge = router.sessions[started]
        assert not bridge.run_task.done()

        await asyncio.wait_for(router.aclose(), timeout=20)
        assert bridge.run_task.done()
        approver.cancel()

    asyncio.run(scenario())


def test_approval_decision_reaches_the_conversation_that_asked(monkeypatch, store, workspace, events):
    """An approval id is namespaced per conversation, so answers cannot cross over."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(1.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            started = (await router.handle("sendPrompt", {
                "text": "needs approval", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"}))["sessionId"]
            bridge = router.sessions[started]

            # Wait for the approval request and answer it on the right channel.
            approval = None
            for _ in range(200):
                await asyncio.sleep(0.05)
                approval = next((m for m in events if m.get("event") == "approval"), None)
                if approval:
                    break
            assert approval is not None, "no approval request was emitted"
            approval_id = approval["approvalId"]
            assert approval_id.startswith(f"{started}:"), "approval id is not per-conversation"

            # An unknown/guessed id is a no-op rather than a cross-conversation grant.
            assert await router.handle("resolveApproval", {"approvalId": "wrong:1", "granted": True}) is False
            assert await router.handle("resolveApproval", {
                "approvalId": approval_id, "granted": True,
                "sessionId": started, "clientKey": "draft-0"}) is True
            assert await wait_done(bridge.run_task)
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_cancelled_run_always_reports_completion(monkeypatch, store, workspace, events):
    """Stop must always produce a terminal event, even with a slow registry close."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(6.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            await router.handle("initialize", {"clientKey": "draft-0"})
            started = (await router.handle("sendPrompt", {
                "text": "slow", "workspace": str(workspace), "model": "fake",
                "sessionId": None, "clientKey": "draft-0"}))["sessionId"]
            bridge = router.sessions[started]

            # Make the teardown slow so the cancel lands inside it: that is the
            # window in which a second Stop used to swallow the terminal event.
            async def slow_close():
                await asyncio.sleep(0.5)

            bridge.registry = type("SlowRegistry", (), {"aclose": staticmethod(slow_close)})()

            approver = asyncio.create_task(approve_pending(router, events))
            await asyncio.sleep(0.8)
            await router.handle("cancelTurn", {"sessionId": started, "clientKey": "draft-0"})
            # A second Stop while the first is still cleaning up.
            await router.handle("cancelTurn", {"sessionId": started, "clientKey": "draft-0"})
            assert await wait_done(bridge.run_task), "run never finished"

            await asyncio.sleep(0.2)
            terminal = [e for e in events
                        if e.get("sessionId") == started
                        and e.get("event") in {"run_done", "run_error"}]
            assert terminal, "cancelling never produced a terminal event"
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())


def test_idle_conversations_are_released(monkeypatch, store, workspace, events):
    """Idle conversations must not hold runtimes open for the whole app session."""
    monkeypatch.setattr(bridge_mod, "provider_for", scripted(0.0))

    async def scenario():
        router = bridge_mod.BridgeRouter(store)
        try:
            router.IDLE_LIMIT = 2
            await router.handle("initialize", {"clientKey": "draft-0"})
            approver = asyncio.create_task(approve_pending(router, events))
            created = []
            for index in range(5):
                key = f"draft-{index}"
                session_id = (await router.handle("sendPrompt", {
                    "text": f"task {index}", "workspace": str(workspace), "model": "fake",
                    "sessionId": None, "clientKey": key}))["sessionId"]
                created.append(session_id)
                assert await wait_done(router.sessions[session_id].run_task)

            tracked = [key for key in router.recent if key in router.sessions]
            assert len(tracked) <= router.IDLE_LIMIT, f"no eviction happened: {len(tracked)}"
            # The most recent conversation is still usable without a reload.
            newest = created[-1]
            assert router.sessions[newest].runtime is not None
            approver.cancel()
        finally:
            await router.aclose()

    asyncio.run(scenario())
