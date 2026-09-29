"""Tests for minicode.runtime.loop (AgentRuntime) — plain sync pytest driving
async with ``asyncio.run``; no pytest-asyncio."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

import pytest

from minicode.core.models import (
    ApprovalDecision,
    ApprovalRequest,
    Budget,
    Event,
    EventType,
    ExitReason,
    ModelResponse,
    ToolResultBlock,
)
from minicode.providers import (
    FakeProvider,
    FakeProviderOptions,
    FakeToolCall,
    FakeTurn,
    ProviderError,
    ResponseDone,
)
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy, DefaultPolicy, PolicyBehavior
from minicode.storage import SqliteStore
from minicode.tools.registry import default_registry

# ---------------------------------------------------------------------------
# Harness: FakeProvider + tmp workspace + SqliteStore + event sink
# ---------------------------------------------------------------------------


class EventSink:
    """Async callback collecting every mirrored event, usable as ``on_event``."""

    def __init__(self) -> None:
        self.events: list[Event] = []

    async def __call__(self, event: Event) -> None:
        self.events.append(event)

    def of_type(self, event_type: EventType) -> list[Event]:
        return [e for e in self.events if e.type is event_type]


@dataclass
class Harness:
    runtime: AgentRuntime
    store: SqliteStore
    events: EventSink
    workspace: Path


@pytest.fixture()
def harness_factory(tmp_path):
    """Build harnesses over one tmp dir; closes every store at teardown."""
    harnesses: list[Harness] = []

    def factory(
        provider: Any,
        *,
        policy: Any | None = None,
        budget: Budget | None = None,
        approval_handler: Any | None = None,
        on_text_delta: Any | None = None,
    ) -> Harness:
        sink = EventSink()
        workspace = tmp_path / "ws"
        store = SqliteStore(tmp_path / "db.sqlite3")
        runtime = AgentRuntime(
            provider=provider,
            registry=default_registry(),
            store=store,
            policy=policy if policy is not None else AutoAllowPolicy(),
            workspace=workspace,
            provider_name="fake",
            model="fake-model",
            budget=budget,
            approval_handler=approval_handler,
            on_text_delta=on_text_delta,
            on_event=sink,
        )
        harness = Harness(runtime=runtime, store=store, events=sink, workspace=workspace)
        harnesses.append(harness)
        return harness

    yield factory

    for harness in harnesses:
        harness.store.close()


def tool_result_blocks(store: SqliteStore, session_id: str) -> list[ToolResultBlock]:
    """Every ToolResultBlock persisted for the session, in message order."""
    return [
        block
        for message in store.get_messages(session_id)
        for block in message.content
        if isinstance(block, ToolResultBlock)
    ]


def _write_turn(path: str = "new.txt") -> FakeTurn:
    return FakeTurn(
        tool_calls=[FakeToolCall(name="write", arguments={"path": path, "content": "hello"})]
    )


# ---------------------------------------------------------------------------
# 1. Completed turn: messages, event order, streamed deltas
# ---------------------------------------------------------------------------


def test_completed_turn_persists_messages_events_and_deltas(harness_factory):
    text = "你好，任务完成了"
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text=text)]))
    deltas: list[str] = []

    async def collect_delta(piece: str) -> None:
        deltas.append(piece)

    harness = harness_factory(provider, on_text_delta=collect_delta)

    result = asyncio.run(harness.runtime.run_turn("帮我看看"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert result.rounds == 1

    messages = harness.store.get_messages(result.session_id)
    assert [m.role for m in messages] == ["user", "assistant"]
    assert messages[0].content[0].text == "帮我看看"
    assert messages[1].content[0].text == text

    # Streamed deltas reached the callback and reassemble the full text.
    assert "".join(deltas) == text

    # Exact event order for a text-only turn.
    assert [e.type for e in harness.events.events] == [
        EventType.SESSION_START,
        EventType.ROUND_START,
        EventType.ASSISTANT_MESSAGE,
        EventType.SESSION_END,
    ]
    start = harness.events.of_type(EventType.SESSION_START)[0]
    assert start.data["provider"] == "fake"
    assert start.data["model"] == "fake-model"
    assert start.data["workspace"] == str(harness.workspace.resolve())


# ---------------------------------------------------------------------------
# 2. Tool round-trip: file created, ToolResultBlock persisted, rounds == 2
# ---------------------------------------------------------------------------


def test_tool_roundtrip_creates_file_and_backfills_result(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[_write_turn(), FakeTurn(text="done")])
    )
    harness = harness_factory(provider)  # AutoAllowPolicy

    result = asyncio.run(harness.runtime.run_turn("create the file"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert result.rounds == 2
    assert (harness.workspace / "new.txt").read_text(encoding="utf-8") == "hello"

    # The tool result was fed back to the model as a user message.
    tool_blocks = tool_result_blocks(harness.store, result.session_id)
    assert len(tool_blocks) == 1
    assert tool_blocks[0].is_error is False
    assert tool_blocks[0].tool_use_id == "fake_tool_1"

    starts = harness.events.of_type(EventType.TOOL_CALL_START)
    results = harness.events.of_type(EventType.TOOL_CALL_RESULT)
    assert len(starts) == 1 and len(results) == 1
    assert starts[0].data["name"] == "write"
    assert results[0].data["success"] is True


# ---------------------------------------------------------------------------
# 3. Unknown tool: error backfilled, loop continues to COMPLETED
# ---------------------------------------------------------------------------


def test_unknown_tool_backfills_error_and_continues(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(tool_calls=[FakeToolCall(name="does_not_exist")]),
                FakeTurn(text="recovered"),
            ]
        )
    )
    harness = harness_factory(provider)

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    blocks = tool_result_blocks(harness.store, result.session_id)
    assert len(blocks) == 1
    assert blocks[0].is_error is True
    assert "unknown tool" in blocks[0].content
    assert blocks[0].tool_use_id == "fake_tool_1"


# ---------------------------------------------------------------------------
# 4. Invalid arguments: pydantic failure backfilled, loop continues
# ---------------------------------------------------------------------------


def test_invalid_arguments_backfilled_and_loop_continues(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                # edit without the required new_text
                FakeTurn(tool_calls=[FakeToolCall(name="edit", arguments={"path": "x.txt", "old_text": "a"})]),
                FakeTurn(text="will fix it"),
            ]
        )
    )
    harness = harness_factory(provider)

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    blocks = tool_result_blocks(harness.store, result.session_id)
    assert len(blocks) == 1
    assert blocks[0].is_error is True
    assert "invalid arguments" in blocks[0].content
    assert not (harness.workspace / "x.txt").exists()


# ---------------------------------------------------------------------------
# 5. Policy deny: tool never runs, model told, turn still COMPLETED
# ---------------------------------------------------------------------------


def test_policy_deny_blocks_execution_and_backfills(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[_write_turn(), FakeTurn(text="understood")])
    )
    harness = harness_factory(
        provider, policy=DefaultPolicy(rules={"write": PolicyBehavior.DENY})
    )

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert not (harness.workspace / "new.txt").exists()
    blocks = tool_result_blocks(harness.store, result.session_id)
    assert blocks[0].is_error is True
    assert "denied" in blocks[0].content


# ---------------------------------------------------------------------------
# 6. Approval granted: tool runs after the handler approves
# ---------------------------------------------------------------------------


def test_approval_granted_runs_tool(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[_write_turn(), FakeTurn(text="done")])
    )

    async def approve(request: ApprovalRequest) -> ApprovalDecision:
        return ApprovalDecision(granted=True)

    harness = harness_factory(provider, policy=DefaultPolicy(), approval_handler=approve)

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert (harness.workspace / "new.txt").read_text(encoding="utf-8") == "hello"

    requests = harness.events.of_type(EventType.APPROVAL_REQUEST)
    assert len(requests) == 1
    assert requests[0].data["tool_name"] == "write"
    assert requests[0].data["summary"]  # compact human-readable summary present

    decisions = harness.events.of_type(EventType.APPROVAL_DECISION)
    assert len(decisions) == 1
    assert decisions[0].data["granted"] is True


# ---------------------------------------------------------------------------
# 7. Approval denied: error result, no execution
# ---------------------------------------------------------------------------


def test_approval_denied_blocks_execution(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[_write_turn(), FakeTurn(text="ok, skipping")])
    )

    async def deny(request: ApprovalRequest) -> ApprovalDecision:
        return ApprovalDecision(granted=False, reason="no")

    harness = harness_factory(provider, policy=DefaultPolicy(), approval_handler=deny)

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert not (harness.workspace / "new.txt").exists()

    blocks = tool_result_blocks(harness.store, result.session_id)
    assert blocks[0].is_error is True
    assert "approval denied" in blocks[0].content

    decisions = harness.events.of_type(EventType.APPROVAL_DECISION)
    assert len(decisions) == 1
    assert decisions[0].data["granted"] is False
    assert decisions[0].data["reason"] == "no"


# ---------------------------------------------------------------------------
# 8. ASK without an approval handler configured
# ---------------------------------------------------------------------------


def test_ask_without_handler_backfills_error(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[_write_turn(), FakeTurn(text="noted")])
    )
    harness = harness_factory(provider, policy=DefaultPolicy(), approval_handler=None)

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert not (harness.workspace / "new.txt").exists()
    blocks = tool_result_blocks(harness.store, result.session_id)
    assert blocks[0].is_error is True
    assert "approval" in blocks[0].content
    # No approval events without a handler.
    assert harness.events.of_type(EventType.APPROVAL_REQUEST) == []


# ---------------------------------------------------------------------------
# 9. Max rounds: budget stops after N rounds, session status persisted
# ---------------------------------------------------------------------------


def test_max_rounds_exit(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                _write_turn("a.txt"),
                _write_turn("b.txt"),
                _write_turn("c.txt"),  # never streamed: budget spent after 2 rounds
            ]
        )
    )
    harness = harness_factory(provider, budget=Budget(max_rounds=2))

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.MAX_ROUNDS
    assert result.rounds == 2
    assert (harness.workspace / "a.txt").exists()
    assert (harness.workspace / "b.txt").exists()
    assert not (harness.workspace / "c.txt").exists()

    summary = harness.store.get_session(result.session_id)
    assert summary is not None
    assert summary.status == "paused"
    assert summary.exit_reason == "max_rounds"
    assert summary.rounds == 2


# ---------------------------------------------------------------------------
# 10. Token budget: fires after the assistant response, before its tool runs
# ---------------------------------------------------------------------------


def test_token_budget_stops_before_tool_execution(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(
                    tool_calls=[
                        FakeToolCall(
                            name="write",
                            arguments={"path": "new.txt", "new_text": "hello"},
                        )
                    ],
                    input_tokens=1000,
                    output_tokens=500,
                ),
                FakeTurn(text="done"),  # never reached
            ]
        )
    )
    harness = harness_factory(provider, budget=Budget(max_total_tokens=1200))

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.TOKEN_BUDGET
    assert result.rounds == 1
    assert result.total_usage.total_tokens == 1500

    # The assistant response of round 1 was fully processed...
    assistant_events = harness.events.of_type(EventType.ASSISTANT_MESSAGE)
    assert len(assistant_events) == 1
    assert assistant_events[0].data["tool_calls"] == ["write"]

    # ...but its tool call must NOT have executed (no side effects on a blown budget).
    assert not (harness.workspace / "new.txt").exists()
    assert harness.events.of_type(EventType.TOOL_CALL_START) == []
    assert harness.events.of_type(EventType.TOOL_CALL_RESULT) == []


# ---------------------------------------------------------------------------
# 11. Time budget: max_seconds=0 means unlimited
# ---------------------------------------------------------------------------


def test_time_budget_zero_seconds(harness_factory):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="hi")]))
    harness = harness_factory(provider, budget=Budget(max_seconds=0))

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert result.rounds == 1
    assert [m.role for m in harness.store.get_messages(result.session_id)] == ["user", "assistant"]
    assert len(harness.events.of_type(EventType.ROUND_START)) == 1


# ---------------------------------------------------------------------------
# 12. Provider error: PROVIDER_ERROR exit, error surfaced in SESSION_END
# ---------------------------------------------------------------------------


class ExplodingProvider:
    """Provider whose stream() fails immediately with a ProviderError."""

    name = "explode"

    async def stream(self, *, system, messages, tools):  # type: ignore[no-untyped-def]
        raise ProviderError("boom")
        yield  # pragma: no cover - unreachable; makes this an async generator


def test_provider_error_finalizes_session(harness_factory):
    harness = harness_factory(ExplodingProvider())

    result = asyncio.run(harness.runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.PROVIDER_ERROR
    # The round counter increments before streaming, so the failed round counts.
    assert result.rounds == 1
    assert harness.events.of_type(EventType.ROUND_START)

    ends = harness.events.of_type(EventType.SESSION_END)
    assert len(ends) == 1
    assert ends[0].data["exit_reason"] == "provider_error"
    assert ends[0].data["error"] == "boom"

    summary = harness.store.get_session(result.session_id)
    assert summary is not None
    assert summary.status == "paused"
    assert summary.exit_reason == "provider_error"


# ---------------------------------------------------------------------------
# 13. Cancellation: CancelledError propagates, session finalized as cancelled
# ---------------------------------------------------------------------------


class HangingProvider:
    """Provider whose stream() never completes."""

    name = "hang"

    async def stream(self, *, system, messages, tools):  # type: ignore[no-untyped-def]
        await asyncio.sleep(3600)
        yield ResponseDone(response=ModelResponse())  # pragma: no cover - unreachable


def test_cancellation_finalizes_and_propagates(harness_factory):
    harness = harness_factory(HangingProvider())

    async def scenario() -> None:
        task = asyncio.create_task(harness.runtime.run_turn("go"))
        await asyncio.sleep(0.05)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(scenario())

    assert harness.runtime.session_id is not None
    summary = harness.store.get_session(harness.runtime.session_id)
    assert summary is not None
    assert summary.status == "paused"
    assert summary.exit_reason == "cancelled"
    assert harness.events.of_type(EventType.SESSION_END)


# ---------------------------------------------------------------------------
# 14. Chat continuity: one session, accumulating history/usage/rounds
# ---------------------------------------------------------------------------


def test_chat_continuity_accumulates_across_turns(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[FakeTurn(text="first"), FakeTurn(text="second")])
    )
    harness = harness_factory(provider)

    first = asyncio.run(harness.runtime.run_turn("one"))
    second = asyncio.run(harness.runtime.run_turn("two"))

    assert first.session_id == second.session_id == harness.runtime.session_id

    messages = harness.store.get_messages(second.session_id)
    assert [m.role for m in messages] == ["user", "assistant", "user", "assistant"]
    assert len(messages) >= 4

    # Usage and rounds accumulate across turns in the final RunResult
    # (FakeProvider defaults: 100 input + 20 output per turn).
    assert second.rounds == 2
    assert second.total_usage.input_tokens == 200
    assert second.total_usage.output_tokens == 40
    assert second.total_usage.total_tokens == 240

    # The second turn reused the session: SESSION_START emitted exactly once.
    assert len(harness.events.of_type(EventType.SESSION_START)) == 1


# ---------------------------------------------------------------------------
# Live model switching and context-window accounting
# ---------------------------------------------------------------------------


def test_set_model_swaps_provider_and_updates_session(harness_factory, tmp_path):
    from minicode.core.catalog import MODEL_CATALOG

    harness = harness_factory(FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="嗨")])))
    runtime = harness.runtime
    result = asyncio.run(runtime.run_turn("开始"))
    assert result.exit_reason.value == "completed"

    new_provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="换了")]))
    new_provider.name = "commandcode"
    glm = MODEL_CATALOG["z.ai/glm-5.3-flash"]
    runtime.set_model(provider=new_provider, provider_name="commandcode", model=glm.name)

    assert runtime.model == glm.name
    assert runtime.provider is new_provider
    assert runtime.context_window == glm.context_window == 1_048_576
    assert runtime.prompt_budget_tokens() == 1_048_576 - 16_384
    # The persisted session row reflects the switch.
    summary = harness.store.get_session(result.session_id)
    assert summary is not None and summary.model == glm.name
    assert summary.provider == runtime.provider_name == new_provider.name


def test_context_tokens_used_grows_with_messages(harness_factory):
    harness = harness_factory(FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="回复")])))
    runtime = harness.runtime
    before = runtime.context_token_breakdown()
    assert before["system"] > 0
    assert before["tools"] > 0
    assert before["messages"] == 0
    assert runtime.context_tokens_used() == sum(before.values())
    asyncio.run(runtime.run_turn("一句用户输入"))
    # The user message plus the assistant reply now sit in the context.
    after = runtime.context_token_breakdown()
    assert after["messages"] > before["messages"]
    assert runtime.context_tokens_used() == sum(after.values())


def test_runtime_usage_includes_cache_read_tokens(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[FakeTurn(text="回复", input_tokens=100, cache_read_tokens=60)]
        )
    )
    harness = harness_factory(provider)
    asyncio.run(harness.runtime.run_turn("x"))
    usage = harness.runtime.usage
    assert usage.input_tokens == 100
    assert usage.cache_read_tokens == 60


def test_uncapped_budget_runs_past_any_token_total(harness_factory):
    """With the default (uncapped) token budget, heavy spend never ends the
    session — only the round cap does. Regression guard for session 39e5f16a,
    where a 200k cumulative cap killed an ordinary 20-round run."""
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(
                    tool_calls=[FakeToolCall(name="ls", arguments={"path": "."})],
                    input_tokens=500_000,
                    output_tokens=100_000,
                ),
                FakeTurn(text="第一轮之后的总结", input_tokens=500_000, output_tokens=100_000),
            ]
        )
    )
    harness = harness_factory(provider, budget=Budget(max_rounds=3, max_total_tokens=0))

    result = asyncio.run(harness.runtime.run_turn("go"))

    # Completed on its own answer after 1.2M tokens of accumulated spend.
    assert result.exit_reason is ExitReason.COMPLETED
    assert result.total_usage.total_tokens == 1_200_000
    assert result.rounds == 2
