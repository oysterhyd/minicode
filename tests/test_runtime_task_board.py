"""Task graph and approval contracts through the real runtime loop."""

import asyncio

from minicode.core.models import EventType, ExitReason
from minicode.providers import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy, DefaultPolicy
from minicode.storage import SqliteStore
from minicode.tasks import TaskStore
from minicode.tools.registry import default_registry


def test_task_board_is_available_to_agent_and_survives_resume(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    store = SqliteStore(tmp_path / "sessions.db")
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="task_create", arguments={
            "task_id": "first", "title": "First"})]),
        FakeTurn(tool_calls=[FakeToolCall(name="task_create", arguments={
            "task_id": "second", "title": "Second", "depends_on": ["first"]})]),
        FakeTurn(tool_calls=[FakeToolCall(name="task_claim", arguments={"task_id": "second"})]),
        FakeTurn(tool_calls=[FakeToolCall(name="task_claim", arguments={"task_id": "first"})]),
        FakeTurn(tool_calls=[FakeToolCall(name="task_complete", arguments={
            "task_id": "first", "ok": True})]),
        FakeTurn(tool_calls=[FakeToolCall(name="task_claim", arguments={"task_id": "second"})]),
        FakeTurn(text="done"),
    ]))
    runtime = AgentRuntime(provider=provider, registry=default_registry(tasks=True),
                           store=store, policy=AutoAllowPolicy(), workspace=workspace,
                           provider_name="fake", model="fake")
    result = asyncio.run(runtime.run_turn("work"))
    assert result.exit_reason is ExitReason.COMPLETED
    rows = {row.task_id: row for row in TaskStore(store).list_tasks(result.session_id)}
    assert rows["first"].status == "done"
    assert rows["second"].status == "running"
    assert rows["second"].owner == "agent"
    results = [event for event in store.get_events(result.session_id)
               if event.type is EventType.TOOL_CALL_RESULT]
    assert "not claimable" in str(results[2].data["error"])
    store.close()
    reopened = SqliteStore(tmp_path / "sessions.db")
    assert {row.task_id for row in TaskStore(reopened).list_tasks(result.session_id)} == {"first", "second"}
    reopened.close()


def test_invalid_write_arguments_never_request_approval(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    store = SqliteStore(tmp_path / "sessions.db")
    approvals = []

    async def approve(request):
        approvals.append(request)
        raise AssertionError("invalid arguments reached approval")

    runtime = AgentRuntime(
        provider=FakeProvider(FakeProviderOptions(turns=[
            FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={"path": "a.txt"})]),
            FakeTurn(text="done"),
        ])), registry=default_registry(), store=store, policy=DefaultPolicy(),
        workspace=workspace, provider_name="fake", model="fake", approval_handler=approve,
    )
    result = asyncio.run(runtime.run_turn("work"))
    assert result.exit_reason is ExitReason.COMPLETED
    assert not approvals
    assert not (workspace / "a.txt").exists()
    assert not any(event.type is EventType.APPROVAL_REQUEST
                   for event in store.get_events(result.session_id))
    store.close()


def test_unknown_write_argument_never_request_approval(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    store = SqliteStore(tmp_path / "sessions.db")
    approvals = []

    async def approve(request):
        approvals.append(request)
        raise AssertionError("unknown arguments reached approval")

    runtime = AgentRuntime(
        provider=FakeProvider(FakeProviderOptions(turns=[
            FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={
                "path": "a.txt", "content": "x", "unexpected": True})]),
            FakeTurn(text="done"),
        ])), registry=default_registry(), store=store, policy=DefaultPolicy(),
        workspace=workspace, provider_name="fake", model="fake", approval_handler=approve,
    )
    result = asyncio.run(runtime.run_turn("work"))
    assert result.exit_reason is ExitReason.COMPLETED
    assert not approvals
    assert not (workspace / "a.txt").exists()
    store.close()
