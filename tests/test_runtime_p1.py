"""P1 integration tests for the agent loop: goal gate, compaction hook,
background delivery, output spilling and session resume.

All P1 services are duck-typed fakes here — the concrete modules live in
minicode.context / minicode.goals / minicode.tasks and are exercised by their
own test files; these tests pin the *runtime contract* only.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from types import SimpleNamespace

import pytest

from minicode.core.models import (
    Budget,
    Event,
    EventType,
    ExitReason,
    Message,
    TextBlock,
    ToolResultBlock,
    ToolUseBlock,
)
from minicode.providers import (
    FakeProvider,
    FakeProviderOptions,
    FakeToolCall,
    FakeTurn,
)
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.tools.registry import default_registry


class EventSink:
    """Async callback collecting every mirrored event."""

    def __init__(self) -> None:
        self.events: list[Event] = []

    async def __call__(self, event: Event) -> None:
        self.events.append(event)

    def of_type(self, event_type: EventType) -> list[Event]:
        return [e for e in self.events if e.type is event_type]


# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


class FakeGoalChecker:
    """Duck-typed GoalChecker returning queued reports."""

    def __init__(self, reports, max_fix_attempts: int = 2, snapshot=None) -> None:
        self.reports = list(reports)
        self.calls = 0
        self.spec = SimpleNamespace(max_fix_attempts=max_fix_attempts)
        self.protected_snapshot = snapshot

    async def run(self):
        report = self.reports[min(self.calls, len(self.reports) - 1)]
        self.calls += 1
        return report

    def format_failure_report(self, report) -> str:
        failed = [i.item_id for i in report.items if not i.passed]
        return f"验收失败：{', '.join(failed)}。必须修复后重新提交。"


def goal_report(passed: bool, fingerprint: str = "fp1") -> SimpleNamespace:
    return SimpleNamespace(
        passed=passed,
        fingerprint=fingerprint,
        items=[
            SimpleNamespace(
                item_id="tests-pass",
                kind="command",
                passed=passed,
                exit_code=0 if passed else 1,
                detail="ok" if passed else "1 failed",
            )
        ],
    )


class FakeCompactor:
    def __init__(self, trigger: bool = True) -> None:
        self.trigger = trigger
        self.calls = 0

    def needs_compaction(self, system, messages, specs) -> bool:
        return self.trigger

    def compact(self, system, messages, specs):
        self.calls += 1
        stats = SimpleNamespace(
            tokens_before=999,
            tokens_after=100,
            archived_units=2,
            shrunk_results=1,
            summarized_units=0,
        )
        return SimpleNamespace(
            messages=[Message(role="user", content=[TextBlock(text="已压缩")])],
            stats=stats,
            changed=True,
        )


class FakeArtifactStore:
    def __init__(self) -> None:
        self.spilled: list[tuple[str, str]] = []

    def spill(self, session_id: str, kind: str, content: str):
        self.spilled.append((kind, content))
        return SimpleNamespace(artifact_id=f"{kind}_deadbeef", kind=kind, size=len(content))

    def read(self, session_id: str, artifact_id: str):
        for kind, content in self.spilled:
            if artifact_id == f"{kind}_deadbeef":
                return content
        return None


class FakeBackgroundManager:
    def __init__(self, jobs=None) -> None:
        self._pending = list(jobs or [])
        self.cancelled = False

    def start(self, command, cwd, timeout_s):  # pragma: no cover - unused here
        return "bg_1"

    def poll_completed(self):
        jobs, self._pending = self._pending, []
        return jobs

    async def cancel_all(self) -> None:
        self.cancelled = True


def bg_job(status: str = "completed", exit_code: int = 0) -> SimpleNamespace:
    return SimpleNamespace(
        job_id="bg_7",
        command="python -m pytest -q",
        status=status,
        exit_code=exit_code,
        output="3 passed in 0.01s",
    )


# ---------------------------------------------------------------------------
# Harness
# ---------------------------------------------------------------------------


@pytest.fixture()
def harness_factory(tmp_path):
    created: list[tuple[AgentRuntime, SqliteStore]] = []

    def factory(
        provider,
        *,
        budget: Budget | None = None,
        compactor=None,
        goal_checker=None,
        evidence_ledger=None,
        background_manager=None,
        artifact_store=None,
    ):
        sink = EventSink()
        workspace = tmp_path / "ws"
        workspace.mkdir(exist_ok=True)
        store = SqliteStore(tmp_path / f"db{len(created)}.sqlite3")
        runtime = AgentRuntime(
            provider=provider,
            registry=default_registry(),
            store=store,
            policy=AutoAllowPolicy(),
            workspace=workspace,
            provider_name="fake",
            model="fake-model",
            budget=budget,
            on_event=sink,
            compactor=compactor,
            goal_checker=goal_checker,
            evidence_ledger=evidence_ledger,
            background_manager=background_manager,
            artifact_store=artifact_store,
        )
        created.append((runtime, store))
        return runtime, store, sink

    yield factory
    for _, store in created:
        store.close()


# ---------------------------------------------------------------------------
# Goal gate
# ---------------------------------------------------------------------------


def test_goal_gate_pass_completes_session(harness_factory):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="我做完了")]))
    checker = FakeGoalChecker([goal_report(True)])
    runtime, store, sink = harness_factory(provider, goal_checker=checker)

    result = asyncio.run(runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert checker.calls == 1
    checks = sink.of_type(EventType.GOAL_CHECK)
    assert len(checks) == 1
    assert checks[0].data["passed"] is True
    assert checks[0].data["items"][0]["item_id"] == "tests-pass"


def test_goal_gate_failure_feeds_report_back_and_retries(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[FakeTurn(text="看起来好了"), FakeTurn(text="这次真的好了")]
        )
    )
    checker = FakeGoalChecker([goal_report(False), goal_report(True)])
    runtime, store, sink = harness_factory(provider, goal_checker=checker)

    result = asyncio.run(runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert checker.calls == 2
    # The failure report was appended as a user message between the answers.
    roles = [m.role for m in store.get_messages(result.session_id)]
    assert roles == ["user", "assistant", "user", "assistant"]
    texts = [
        b.text
        for m in store.get_messages(result.session_id)
        for b in m.content
        if isinstance(b, TextBlock)
    ]
    assert any("验收失败" in t for t in texts)


def test_goal_gate_exhausts_attempts_goal_not_met(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[FakeTurn(text="a"), FakeTurn(text="b"), FakeTurn(text="c")]
        )
    )
    checker = FakeGoalChecker([goal_report(False)], max_fix_attempts=1)
    runtime, store, sink = harness_factory(provider, goal_checker=checker)

    result = asyncio.run(runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.GOAL_NOT_MET
    assert sink.of_type(EventType.GOAL_CHECK)[0].data["attempt"] == 1
    assert sink.of_type(EventType.GOAL_CHECK)[-1].data["attempt"] == 2
    summary = store.get_session(result.session_id)
    assert summary.status == "goal_not_met"


def test_goal_gate_protected_snapshot_captured_on_first_turn(harness_factory):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done")]))
    snapshot = SimpleNamespace(capture=lambda: None, violations=lambda: [])
    captured = []
    snapshot.capture = lambda: captured.append(True)  # type: ignore[method-assign]
    checker = FakeGoalChecker([goal_report(True)], snapshot=snapshot)
    runtime, _, _ = harness_factory(provider, goal_checker=checker)

    asyncio.run(runtime.run_turn("go"))

    assert captured == [True]


def test_evidence_invalidated_by_workspace_change(harness_factory):
    from minicode.goals.checker import workspace_fingerprint

    provider = FakeProvider(
        FakeProviderOptions(turns=[FakeTurn(text="v1"), FakeTurn(text="v2")])
    )
    runtime, _, _ = harness_factory(
        provider, goal_checker=FakeGoalChecker([goal_report(True)])
    )
    # Seed a file so the workspace fingerprint is stable and comparable with
    # the report's fingerprint (both use goals.checker.workspace_fingerprint).
    (runtime._workspace / "src.py").write_text("a = 1", encoding="utf-8")
    fp = workspace_fingerprint(runtime._workspace)
    checker = FakeGoalChecker([goal_report(True, fp)])
    runtime._goal_checker = checker

    asyncio.run(runtime.run_turn("go"))
    assert checker.calls == 1  # evidence recorded

    (runtime._workspace / "src.py").write_text("a = 2", encoding="utf-8")
    asyncio.run(runtime.run_turn("go"))
    # The code changed after the passing evidence: checks must re-run.
    assert checker.calls == 2


def test_valid_evidence_skips_rerun(harness_factory):
    from minicode.goals.checker import workspace_fingerprint

    provider = FakeProvider(
        FakeProviderOptions(turns=[FakeTurn(text="v1"), FakeTurn(text="v2")])
    )
    runtime, _, _ = harness_factory(
        provider, goal_checker=FakeGoalChecker([goal_report(True)])
    )
    (runtime._workspace / "src.py").write_text("a = 1", encoding="utf-8")
    checker = FakeGoalChecker(
        [goal_report(True, workspace_fingerprint(runtime._workspace))]
    )
    runtime._goal_checker = checker

    asyncio.run(runtime.run_turn("go"))
    asyncio.run(runtime.run_turn("go"))

    # Workspace unchanged: the recorded evidence covers the second answer.
    assert checker.calls == 1


# ---------------------------------------------------------------------------
# Compaction hook
# ---------------------------------------------------------------------------


def test_compaction_hook_rewrites_persisted_messages(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(turns=[FakeTurn(text="first"), FakeTurn(text="second")])
    )
    compactor = FakeCompactor(trigger=True)
    runtime, store, sink = harness_factory(provider, compactor=compactor)

    asyncio.run(runtime.run_turn("one"))

    assert compactor.calls >= 1
    assert sink.of_type(EventType.CONTEXT_COMPACTED)
    compacted = sink.of_type(EventType.CONTEXT_COMPACTED)[0].data
    assert compacted["tokens_before"] == 999
    assert compacted["tokens_after"] == 100
    # The compacted view replaced the persisted conversation.
    messages = store.get_messages(runtime.session_id)
    assert messages[0].content[0].text == "已压缩"


def test_compaction_not_triggered_when_needs_false(harness_factory):
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="hi")]))
    compactor = FakeCompactor(trigger=False)
    runtime, store, sink = harness_factory(provider, compactor=compactor)

    asyncio.run(runtime.run_turn("go"))

    assert compactor.calls == 0
    assert sink.of_type(EventType.CONTEXT_COMPACTED) == []


# ---------------------------------------------------------------------------
# Background delivery
# ---------------------------------------------------------------------------


def test_background_job_delivered_once_as_user_message(harness_factory):
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[FakeTurn(tool_calls=[FakeToolCall(name="read_file", arguments={"path": "a.txt"})]), FakeTurn(text="结果看到了")]
        )
    )
    manager = FakeBackgroundManager([bg_job()])
    runtime, store, sink = harness_factory(provider, background_manager=manager)
    (runtime._workspace / "a.txt").write_text("x", encoding="utf-8")

    result = asyncio.run(runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    events = sink.of_type(EventType.BACKGROUND_JOB_COMPLETED)
    assert len(events) == 1
    assert events[0].data["job_id"] == "bg_7"
    # Delivered exactly once even across more rounds (poll drains).
    texts = [
        b.text
        for m in store.get_messages(result.session_id)
        for b in m.content
        if isinstance(b, TextBlock)
    ]
    assert sum("[后台任务 bg_7" in t for t in texts) == 1
    assert manager.cancelled is True  # cancelled at finalize


# ---------------------------------------------------------------------------
# Output spilling
# ---------------------------------------------------------------------------


def test_large_output_spilled_with_readback_reference(harness_factory):
    # search_text over a big file produces an output above the spill threshold
    # (long lines: the search results cap alone would keep the output small).
    big = "\n".join(f"needle line {i} " + "x" * 200 for i in range(200))
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(
                    tool_calls=[
                        FakeToolCall(name="search_text", arguments={"pattern": "needle"})
                    ]
                ),
                FakeTurn(text="done"),
            ]
        )
    )
    store_artifacts = FakeArtifactStore()
    runtime, db, sink = harness_factory(provider, artifact_store=store_artifacts)
    (runtime._workspace / "big.txt").write_text(big, encoding="utf-8")

    result = asyncio.run(runtime.run_turn("go"))

    assert result.exit_reason is ExitReason.COMPLETED
    assert store_artifacts.spilled, "oversized output should be spilled"
    kind, content = store_artifacts.spilled[0]
    assert kind == "tool_output"
    assert len(content) > 4000 and "needle line 0" in content
    # Model-facing result keeps a preview plus the artifact reference.
    result_blocks = [
        b
        for m in db.get_messages(result.session_id)
        for b in m.content
        if isinstance(b, ToolResultBlock)
    ]
    assert len(result_blocks) == 1
    assert "tool_output_deadbeef" in result_blocks[0].content
    assert "read_artifact" in result_blocks[0].content
    assert len(result_blocks[0].content) < len(big) // 2


# ---------------------------------------------------------------------------
# Resume
# ---------------------------------------------------------------------------


def _prepare_dangling_session(tmp_path: Path, tool_name: str, arguments: dict):
    """Persist a session that died right after a tool call started."""
    store = SqliteStore(tmp_path / "resume.sqlite3")
    workspace = tmp_path / "ws"
    workspace.mkdir(exist_ok=True)
    session_id = store.create_session(
        workspace=str(workspace), provider="fake", model="fake-model"
    )
    store.append_message(session_id, Message(role="user", content=[TextBlock(text="go")]))
    call = ToolUseBlock(id="call_lost", name=tool_name, input=arguments)
    store.append_message(session_id, Message(role="assistant", content=[call]))
    store.append_event(
        session_id,
        EventType.TOOL_CALL_START,
        {"call_id": call.id, "name": tool_name, "arguments": arguments},
    )
    store.update_session(session_id, status="running")
    return store, session_id, workspace, call


def test_resume_reexecutes_read_only_call(harness_factory, tmp_path):
    store, session_id, workspace, call = _prepare_dangling_session(
        tmp_path, "read_file", {"path": "notes.txt"}
    )
    (workspace / "notes.txt").write_text("kept", encoding="utf-8")
    sink = EventSink()
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="续上了")]))

    runtime = AgentRuntime.resume(
        store=store,
        session_id=session_id,
        provider=provider,
        registry=default_registry(),
        policy=AutoAllowPolicy(),
        workspace=workspace,
        on_event=sink,
    )
    result = asyncio.run(runtime.run_turn("继续"))

    assert result.exit_reason is ExitReason.COMPLETED
    # The read-only call was re-executed under a fresh event record.
    starts = [e for e in sink.of_type(EventType.TOOL_CALL_START) if e.data.get("recovered")]
    assert len(starts) == 1 and starts[0].data["name"] == "read_file"
    # Result stored with the ORIGINAL call id (the model conversation stays valid).
    recovered = [
        b
        for m in store.get_messages(session_id)
        for b in m.content
        if isinstance(b, ToolResultBlock) and b.tool_use_id == call.id
    ]
    assert len(recovered) == 1
    assert "kept" in recovered[0].content


def test_resume_marks_write_side_effect_unknown(harness_factory, tmp_path):
    store, session_id, workspace, call = _prepare_dangling_session(
        tmp_path, "apply_patch", {"path": "x.txt", "new_text": "??"}
    )
    sink = EventSink()
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="明白，先核实")]))

    runtime = AgentRuntime.resume(
        store=store,
        session_id=session_id,
        provider=provider,
        registry=default_registry(),
        policy=AutoAllowPolicy(),
        workspace=workspace,
        on_event=sink,
    )
    result = asyncio.run(runtime.run_turn("继续"))

    assert result.exit_reason is ExitReason.COMPLETED
    unknown = sink.of_type(EventType.SIDE_EFFECT_UNKNOWN)
    assert len(unknown) == 1
    assert unknown[0].data["name"] == "apply_patch"
    assert unknown[0].data["call_id"] == call.id
    # The file must NOT have been written by recovery.
    assert not (workspace / "x.txt").exists()
    recovered = [
        b
        for m in store.get_messages(session_id)
        for b in m.content
        if isinstance(b, ToolResultBlock) and b.tool_use_id == call.id
    ]
    assert len(recovered) == 1
    assert "副作用状态未知" in recovered[0].content


def test_resume_unknown_session_raises(tmp_path):
    store = SqliteStore(tmp_path / "db.sqlite3")
    try:
        with pytest.raises(ValueError):
            AgentRuntime.resume(
                store=store,
                session_id="nope",
                provider=FakeProvider(),
                registry=default_registry(),
                policy=AutoAllowPolicy(),
                workspace=tmp_path,
            )
    finally:
        store.close()


def test_resume_restores_usage_rounds_and_history(harness_factory, tmp_path):
    provider = FakeProvider(
        FakeProviderOptions(turns=[FakeTurn(text="第一轮", input_tokens=50)])
    )
    runtime, store, _ = harness_factory(provider)
    first = asyncio.run(runtime.run_turn("一"))
    old_session = first.session_id

    resumed = AgentRuntime.resume(
        store=store,
        session_id=old_session,
        provider=FakeProvider(
            FakeProviderOptions(turns=[FakeTurn(text="第二轮", input_tokens=70)])
        ),
        registry=default_registry(),
        policy=AutoAllowPolicy(),
    )
    second = asyncio.run(resumed.run_turn("二"))

    assert second.rounds == 2  # cumulative across the resume boundary
    assert second.total_usage.input_tokens == 120
    roles = [m.role for m in store.get_messages(old_session)]
    assert roles == ["user", "assistant", "user", "assistant"]
    assert len(resumed._pending_dangling) == 0
