"""Crash at each side-effect persistence boundary and resume without replay."""

import asyncio

import pytest
from pydantic import BaseModel

from minicode.core.models import EventType, ExitReason, ToolOutcome
from minicode.providers import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.tools.base import BaseTool, ToolContext
from minicode.tools.registry import ToolRegistry


class SimulatedCrash(BaseException):
    pass


class CounterArgs(BaseModel):
    path: str


class CounterWrite(BaseTool):
    name = "write"
    description = "Append one marker for persistence tests."
    args_model = CounterArgs

    def __init__(self):
        self.crash_before = False

    async def execute(self, args: CounterArgs, ctx: ToolContext) -> ToolOutcome:
        if self.crash_before:
            self.crash_before = False
            raise SimulatedCrash()
        with (ctx.workspace / args.path).open("a", encoding="utf-8") as handle:
            handle.write("x")
        return ToolOutcome(output="written")


def _harness(tmp_path, tool):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    store = SqliteStore(tmp_path / "db.sqlite3")
    registry = ToolRegistry()
    registry.register(tool)
    options = FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={"path": "count.txt"})]),
        FakeTurn(text="done"),
    ])

    def runtime(provider=None):
        return AgentRuntime(provider=provider or FakeProvider(options), registry=registry,
                            store=store, policy=AutoAllowPolicy(), workspace=workspace,
                            provider_name="fake", model="fake")

    return workspace, store, registry, options, runtime


@pytest.mark.parametrize("stage,expected_count", [
    ("before_execution", 0),
    ("after_execution_before_result", 1),
    ("after_result_persisted", 1),
])
def test_crash_recovery_never_replays_write(tmp_path, stage, expected_count):
    tool = CounterWrite()
    workspace, store, registry, options, create = _harness(tmp_path, tool)
    runtime = create()
    if stage == "before_execution":
        tool.crash_before = True
    elif stage == "after_execution_before_result":
        def crash_after_execution(session_id, outcome):
            raise SimulatedCrash()
        runtime._maybe_spill = crash_after_execution
    else:
        def crash_after_result(calls, results):
            raise SimulatedCrash()
        runtime._tool_cycle_stalled = crash_after_result
    with pytest.raises(SimulatedCrash):
        asyncio.run(runtime.run_turn("go"))
    session_id = runtime.session_id
    assert session_id is not None
    counter = workspace / "count.txt"
    assert (counter.read_text(encoding="utf-8").count("x") if counter.exists() else 0) == expected_count

    resumed = AgentRuntime.resume(store=store, session_id=session_id,
                                  provider=FakeProvider(options), registry=registry,
                                  policy=AutoAllowPolicy(), workspace=workspace,
                                  provider_name="fake", model="fake")
    result = asyncio.run(resumed.continue_turn())
    assert result.exit_reason is ExitReason.COMPLETED
    assert (counter.read_text(encoding="utf-8").count("x") if counter.exists() else 0) == expected_count
    unknown = [e for e in store.get_events(session_id) if e.type is EventType.SIDE_EFFECT_UNKNOWN]
    assert len(unknown) == (0 if stage == "after_result_persisted" else 1)
    store.close()
