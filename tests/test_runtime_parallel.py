"""Read concurrency keeps model result order and write barriers intact."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from pydantic import BaseModel

from minicode.core.models import ApprovalDecision, ExitReason, ToolOutcome, ToolResultBlock
from minicode.providers import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy, DefaultPolicy, PolicyBehavior
from minicode.storage import SqliteStore
from minicode.tools.base import BaseTool, ToolContext, READ_EXECUTION
from minicode.tools.registry import ToolRegistry


class Args(BaseModel):
    tag: str


def _runtime(tmp_path: Path, registry: ToolRegistry, calls: list[FakeToolCall],
             *, policy=None, approval_handler=None):
    store = SqliteStore(tmp_path / "parallel.sqlite3")
    provider = FakeProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=calls), FakeTurn(text="done")
    ]))
    runtime = AgentRuntime(
        provider=provider, registry=registry, store=store,
        policy=policy or AutoAllowPolicy(), workspace=tmp_path,
        approval_handler=approval_handler,
        provider_name="fake", model="fake-model",
    )
    return runtime, store


def test_adjacent_reads_overlap_but_write_is_a_barrier(tmp_path):
    activity: list[str] = []
    active = 0
    peak = 0

    class Read(BaseTool):
        execution = READ_EXECUTION
        name = "read"
        description = "Test read"
        args_model = Args

        async def execute(self, args: Args, ctx: ToolContext) -> ToolOutcome:
            nonlocal active, peak
            active += 1
            peak = max(peak, active)
            activity.append(f"start:{args.tag}")
            await asyncio.sleep(0.03)
            activity.append(f"end:{args.tag}")
            active -= 1
            return ToolOutcome(output=args.tag)

    class Write(BaseTool):
        name = "write"
        description = "Test write"
        args_model = Args

        async def execute(self, args: Args, ctx: ToolContext) -> ToolOutcome:
            activity.append(f"write:{args.tag}")
            return ToolOutcome(output=args.tag)

    registry = ToolRegistry()
    registry.register(Read())
    registry.register(Write())
    calls = [FakeToolCall(name="read", arguments={"tag": str(i)}, id=f"r{i}") for i in range(6)]
    calls.append(FakeToolCall(name="write", arguments={"tag": "barrier"}, id="w"))
    calls.extend(FakeToolCall(name="read", arguments={"tag": str(i)}, id=f"r{i}")
                 for i in range(6, 8))
    runtime, store = _runtime(tmp_path, registry, calls)
    try:
        result = asyncio.run(runtime.run_turn("run"))
        assert result.exit_reason is ExitReason.COMPLETED
        assert peak == 4
        assert activity.index("end:5") < activity.index("write:barrier")
        assert activity.index("write:barrier") < activity.index("start:6")
        blocks = [block for message in store.get_messages(result.session_id)
                  for block in message.content if isinstance(block, ToolResultBlock)]
        assert [block.tool_use_id for block in blocks] == [call.id for call in calls]
    finally:
        store.close()


def test_cancelling_parallel_reads_stops_all_children(tmp_path):
    started = 0
    both_started = asyncio.Event()
    cancelled: list[str] = []

    class Read(BaseTool):
        execution = READ_EXECUTION
        name = "read"
        description = "Blocking test read"
        args_model = Args

        async def execute(self, args: Args, ctx: ToolContext) -> ToolOutcome:
            nonlocal started
            started += 1
            if started == 2:
                both_started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                cancelled.append(args.tag)
                raise
            return ToolOutcome(output=args.tag)

    registry = ToolRegistry()
    registry.register(Read())
    calls = [FakeToolCall(name="read", arguments={"tag": str(i)}) for i in range(2)]
    runtime, store = _runtime(tmp_path, registry, calls)

    async def scenario():
        turn = asyncio.create_task(runtime.run_turn("run"))
        await asyncio.wait_for(both_started.wait(), 2)
        turn.cancel()
        with pytest.raises(asyncio.CancelledError):
            await turn

    try:
        asyncio.run(scenario())
        assert sorted(cancelled) == ["0", "1"]
        assert runtime.session_id is not None
        assert not [block for message in store.get_messages(runtime.session_id)
                    for block in message.content if isinstance(block, ToolResultBlock)]
    finally:
        store.close()


def test_read_approvals_are_serialized_even_when_calls_overlap(tmp_path):
    approvals_active = 0
    approval_peak = 0

    class Read(BaseTool):
        execution = READ_EXECUTION
        name = "read"
        description = "Approved test read"
        args_model = Args

        async def execute(self, args: Args, ctx: ToolContext) -> ToolOutcome:
            await asyncio.sleep(0.02)
            return ToolOutcome(output=args.tag)

    async def approve(_request):
        nonlocal approvals_active, approval_peak
        approvals_active += 1
        approval_peak = max(approval_peak, approvals_active)
        await asyncio.sleep(0.02)
        approvals_active -= 1
        return ApprovalDecision(granted=True)

    registry = ToolRegistry()
    registry.register(Read())
    calls = [FakeToolCall(name="read", arguments={"tag": str(i)}) for i in range(3)]
    runtime, store = _runtime(
        tmp_path, registry, calls,
        policy=DefaultPolicy(rules={"read": PolicyBehavior.ASK}),
        approval_handler=approve,
    )
    try:
        result = asyncio.run(runtime.run_turn("run"))
        assert result.exit_reason is ExitReason.COMPLETED
        assert approval_peak == 1
    finally:
        store.close()
