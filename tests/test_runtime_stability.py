"""Durable continuation and bounded recovery for long-running tasks."""

from __future__ import annotations

import asyncio

from minicode.core.models import (
    Budget, EventType, ExitReason, Message, ModelResponse, TextBlock,
    ToolResultBlock, ToolUseBlock, Usage,
)
from minicode.cli import _run_one_turn
from minicode.providers import (
    FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn,
    ProviderError, ProviderRequestError, ResponseDone,
)
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy, DefaultPolicy, PolicyBehavior
from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.registry import default_registry


def make_runtime(tmp_path, provider, *, budget=None, store=None):
    workspace = tmp_path / "workspace"
    workspace.mkdir(exist_ok=True)
    store = store or SqliteStore(tmp_path / "sessions.sqlite3")
    runtime = AgentRuntime(
        provider=provider, registry=default_registry(), store=store,
        policy=AutoAllowPolicy(), workspace=workspace,
        provider_name="fake", model="fake",
        budget=budget or Budget(), artifact_store=ArtifactStore(store),
    )
    return runtime, store, workspace


def read_turn(path: str = "a.txt") -> FakeTurn:
    return FakeTurn(tool_calls=[FakeToolCall(name="read", arguments={"path": path})])


def test_round_slices_continue_without_repeating_user_input(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[
        *(read_turn(f"{i}.txt") for i in range(4)), FakeTurn(text="done")
    ]))
    runtime, store, workspace = make_runtime(tmp_path, provider, budget=Budget(max_rounds=2))
    for i in range(4):
        (workspace / f"{i}.txt").write_text(f"evidence {i}", encoding="utf-8")
    try:
        first = asyncio.run(runtime.run_turn("investigate"))
        second = asyncio.run(runtime.continue_turn())
        third = asyncio.run(runtime.continue_turn())
        assert [first.exit_reason, second.exit_reason, third.exit_reason] == [
            ExitReason.MAX_ROUNDS, ExitReason.MAX_ROUNDS, ExitReason.COMPLETED
        ]
        assert third.rounds == 5
        assert store.get_session(third.session_id).status == "completed"
        user_texts = [block.text for msg in store.get_messages(third.session_id)
                      if msg.role == "user" for block in msg.content
                      if isinstance(block, TextBlock)]
        assert user_texts == ["investigate"]
    finally:
        store.close()


def test_paused_task_continues_after_process_style_resume(tmp_path):
    script = FakeProviderOptions(turns=[read_turn(), FakeTurn(text="done")])
    provider = FakeProvider(script)
    runtime, store, workspace = make_runtime(tmp_path, provider, budget=Budget(max_rounds=1))
    (workspace / "a.txt").write_text("evidence", encoding="utf-8")
    try:
        first = asyncio.run(runtime.run_turn("investigate"))
        assert first.exit_reason is ExitReason.MAX_ROUNDS
        restarted_provider = FakeProvider(script)
        restored = AgentRuntime.resume(
            store=store, session_id=first.session_id, provider=restarted_provider,
            registry=default_registry(), policy=AutoAllowPolicy(),
            workspace=workspace, provider_name="fake", model="fake",
            budget=Budget(max_rounds=1), artifact_store=ArtifactStore(store),
        )
        assert restored.task_pending
        finished = asyncio.run(restored.continue_turn())
        assert finished.exit_reason is ExitReason.COMPLETED
        assert finished.rounds == 2
        assert restarted_provider.turns_consumed == 2
    finally:
        store.close()


def test_time_slice_auto_continues_when_task_makes_progress(tmp_path):
    class SlowThenFast:
        name = "fake"

        def __init__(self):
            self.calls = 0

        async def stream(self, **kwargs):
            self.calls += 1
            if self.calls == 1:
                await asyncio.sleep(1)
            yield ResponseDone(response=ModelResponse(blocks=[TextBlock(text="done")]))

    provider = SlowThenFast()
    runtime, store, _ = make_runtime(tmp_path, provider, budget=Budget(max_seconds=.15))
    try:
        result = _run_one_turn(runtime, "go")
        assert result.exit_reason is ExitReason.COMPLETED
        assert provider.calls == 2
    finally:
        store.close()


def test_time_slice_pauses_after_no_progress(tmp_path):
    class NeverResponds:
        name = "fake"

        def __init__(self):
            self.calls = 0

        async def stream(self, **kwargs):
            self.calls += 1
            await asyncio.sleep(1)
            yield ResponseDone(response=ModelResponse(blocks=[TextBlock(text="unreachable")]))

    provider = NeverResponds()
    runtime, store, _ = make_runtime(tmp_path, provider, budget=Budget(max_seconds=.15))
    try:
        result = _run_one_turn(runtime, "go")
        assert result.exit_reason is ExitReason.TIME_BUDGET
        assert provider.calls == 2
        assert store.get_session(result.session_id).status == "paused"
    finally:
        store.close()


def test_oversized_user_input_is_archived_and_task_completes(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="ack")]))
    runtime, store, _ = make_runtime(tmp_path, provider)
    original = "任务说明 " + "x" * 800_000
    try:
        result = asyncio.run(runtime.run_turn(original))
        assert result.exit_reason is ExitReason.COMPLETED
        artifacts = store.list_artifacts(result.session_id)
        assert any(item["kind"] == "context_input" for item in artifacts)
        assert ArtifactStore(store).read(result.session_id, artifacts[0]["artifact_id"]) == original
        visible = store.get_messages(result.session_id)[0].content[0].text
        assert "[artifact:" in visible
        assert len(visible) < 2000
    finally:
        store.close()


def test_transient_provider_errors_retry_before_pausing(tmp_path):
    class Flaky:
        name = "fake"

        def __init__(self):
            self.calls = 0

        async def stream(self, **kwargs):
            self.calls += 1
            if self.calls < 3:
                raise ProviderError("temporary gateway failure")
            yield ResponseDone(response=ModelResponse(
                blocks=[TextBlock(text="done")], usage=Usage(available=True)
            ))

    provider = Flaky()
    runtime, store, _ = make_runtime(tmp_path, provider)
    try:
        result = asyncio.run(runtime.run_turn("go"))
        assert result.exit_reason is ExitReason.COMPLETED
        assert provider.calls == 3
    finally:
        store.close()


def test_provider_recovers_on_later_continuation_without_new_user_message(tmp_path):
    class DownThenUp:
        name = "fake"

        def __init__(self):
            self.calls = 0

        async def stream(self, **kwargs):
            self.calls += 1
            if self.calls <= 3:
                raise ProviderError("gateway down")
            yield ResponseDone(response=ModelResponse(blocks=[TextBlock(text="done")]))

    provider = DownThenUp()
    runtime, store, _ = make_runtime(tmp_path, provider)
    try:
        paused = asyncio.run(runtime.run_turn("go"))
        assert paused.exit_reason is ExitReason.PROVIDER_ERROR
        assert store.get_session(paused.session_id).status == "paused"
        finished = asyncio.run(runtime.continue_turn())
        assert finished.exit_reason is ExitReason.COMPLETED
        assert provider.calls == 4
        user_texts = [block.text for msg in store.get_messages(finished.session_id)
                      if msg.role == "user" for block in msg.content
                      if isinstance(block, TextBlock)]
        assert user_texts == ["go"]
    finally:
        store.close()


def test_context_rejection_shrinks_request_and_retries(tmp_path):
    class RejectOnce:
        name = "fake"

        def __init__(self):
            self.calls = 0

        async def stream(self, **kwargs):
            self.calls += 1
            if self.calls == 1:
                raise ProviderRequestError("context_length exceeded")
            yield ResponseDone(response=ModelResponse(blocks=[TextBlock(text="done")]))

    provider = RejectOnce()
    runtime, store, _ = make_runtime(tmp_path, provider)
    try:
        result = asyncio.run(runtime.run_turn("x" * 5000))
        assert result.exit_reason is ExitReason.COMPLETED
        assert provider.calls == 2
        assert store.list_artifacts(result.session_id)
    finally:
        store.close()


def test_explicit_cost_guard_pauses_without_marking_unexecuted_write_unknown(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={
            "path": "late.txt", "content": "late",
        })], input_tokens=1000),
        FakeTurn(text="done"),
    ]))
    runtime, store, workspace = make_runtime(tmp_path, provider,
                                             budget=Budget(max_total_tokens=100))
    try:
        result = asyncio.run(runtime.run_turn("go"))
        assert result.exit_reason is ExitReason.TOKEN_BUDGET
        assert not (workspace / "late.txt").exists()
        assert runtime._find_dangling_calls(store.get_messages(result.session_id)) == []
        assert any(isinstance(block, ToolResultBlock) and block.is_error
                   for msg in store.get_messages(result.session_id) for block in msg.content)
        guarded = AgentRuntime.resume(
            store=store, session_id=result.session_id, provider=provider,
            registry=default_registry(), policy=AutoAllowPolicy(),
            workspace=workspace, provider_name="fake", model="fake",
            budget=Budget(), artifact_store=ArtifactStore(store),
        )
        assert guarded._budget.max_total_tokens == 100
        still_paused = asyncio.run(guarded.continue_turn())
        assert still_paused.exit_reason is ExitReason.TOKEN_BUDGET
        assert provider.turns_consumed == 1
        restored = AgentRuntime.resume(
            store=store, session_id=result.session_id, provider=provider,
            registry=default_registry(), policy=AutoAllowPolicy(),
            workspace=workspace, provider_name="fake", model="fake",
            budget=Budget(max_total_tokens=-1), artifact_store=ArtifactStore(store),
        )
        finished = asyncio.run(restored.continue_turn())
        assert finished.exit_reason is ExitReason.COMPLETED
        assert not (workspace / "late.txt").exists()
    finally:
        store.close()


def test_identical_tool_cycles_pause_for_review_instead_of_running_forever(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[read_turn() for _ in range(8)]))
    runtime, store, workspace = make_runtime(tmp_path, provider,
                                             budget=Budget(max_rounds=0))
    (workspace / "a.txt").write_text("same", encoding="utf-8")
    try:
        result = asyncio.run(runtime.run_turn("go"))
        assert result.exit_reason is ExitReason.STALLED
        assert result.rounds == 4
        assert store.get_session(result.session_id).status == "paused"
    finally:
        store.close()


def test_recovered_read_rechecks_current_permission_policy(tmp_path):
    runtime, store, workspace = make_runtime(
        tmp_path, FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done")]))
    )
    session_id = store.create_session(
        workspace=str(workspace), provider="fake", model="fake"
    )
    store.append_message(session_id, Message(role="user", content=[TextBlock(text="go")]))
    store.append_message(session_id, Message(role="assistant", content=[
        ToolUseBlock(id="pending", name="read", input={"path": "a.txt"})
    ]))
    try:
        restored = AgentRuntime.resume(
            store=store, session_id=session_id, provider=runtime.provider,
            registry=default_registry(),
            policy=DefaultPolicy(rules={"read": PolicyBehavior.DENY}),
            workspace=workspace, provider_name="fake", model="fake",
            artifact_store=ArtifactStore(store),
        )
        result = asyncio.run(restored.continue_turn())
        assert result.exit_reason is ExitReason.COMPLETED
        blocks = [block for msg in store.get_messages(session_id) for block in msg.content
                  if isinstance(block, ToolResultBlock)]
        assert len(blocks) == 1 and blocks[0].is_error
        assert "permission denied" in blocks[0].content
        starts = [event for event in store.get_events(session_id)
                  if event.type is EventType.TOOL_CALL_START]
        assert starts[0].data["recovered"] is True
    finally:
        store.close()


def test_unexpected_goal_error_pauses_with_state_for_continuation(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done")]))
    runtime, store, _ = make_runtime(tmp_path, provider)

    class BrokenChecker:
        protected_snapshot = None

        async def run(self):
            raise RuntimeError("checker unavailable")

    runtime._goal_checker = BrokenChecker()
    try:
        result = asyncio.run(runtime.run_turn("go"))
        assert result.exit_reason is ExitReason.INTERNAL_ERROR
        assert result.error == "checker unavailable"
        assert runtime.task_pending
        assert store.get_session(result.session_id).status == "paused"
        runtime._goal_checker = None
        resumed = asyncio.run(runtime.continue_turn())
        assert resumed.exit_reason is ExitReason.COMPLETED
    finally:
        store.close()


def test_event_observer_failure_does_not_interrupt_persisted_task(tmp_path):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done")]))
    runtime, store, _ = make_runtime(tmp_path, provider)

    async def failing_observer(_event):
        raise RuntimeError("UI detached")

    runtime._on_event = failing_observer
    try:
        result = asyncio.run(runtime.run_turn("go"))
        assert result.exit_reason is ExitReason.COMPLETED
        assert store.get_events(result.session_id)
    finally:
        store.close()
