"""Regression coverage for runtime state, deadlines, recovery and subprocess ownership."""

import asyncio
import os
import sys
import time
from pathlib import Path

import pytest
from rich.console import Console

from minicode.cli import _Setup, _build_services, _make_resumed_runtime, _prepare_resume
from minicode.core.models import Budget, EventType, ExitReason, StopReason, ToolOutcome
from minicode.goals import AcceptanceSpec, GoalChecker, ProtectedSnapshot, workspace_fingerprint
from minicode.providers import FakeProvider, FakeProviderOptions, FakeTurn
from minicode.runtime import AgentRuntime
from minicode.security import AutoAllowPolicy
from minicode.storage import SqliteStore
from minicode.tasks.background import BackgroundManager
from minicode.tools.registry import default_registry


def scripted(*turns):
    return FakeProvider(FakeProviderOptions(turns=[FakeTurn(**turn) for turn in turns]))


@pytest.fixture
def env(tmp_path):
    ws = tmp_path / "workspace"
    ws.mkdir()
    store = SqliteStore(tmp_path / "sessions.db")
    yield ws, store
    store.close()


def runtime_for(env, provider=None, **kwargs):
    ws, store = env
    provider = provider or scripted({"text": "done"})
    return AgentRuntime(
        provider=provider, provider_name=provider.name, model=getattr(provider, "model", "fake"),
        workspace=ws, store=store, registry=default_registry(), policy=AutoAllowPolicy(), **kwargs,
    )


def resume(env, sid, **kwargs):
    return AgentRuntime.resume(
        store=env[1], session_id=sid, provider=kwargs.pop("provider", scripted({"text": "done"})),
        registry=default_registry(), policy=AutoAllowPolicy(), **kwargs,
    )


def write_call(path="late.txt"):
    return {"name": "write", "arguments": {"path": path, "content": "side effect"}}


def protected_checker(ws):
    spec = AcceptanceSpec(items=[{"id": "protected", "type": "protected", "path": "guard.txt"}], max_fix_attempts=1)
    return GoalChecker(spec, ws, ProtectedSnapshot(ws, ["guard.txt"]))


def test_builtin_services_only_expose_supported_tools_and_keep_artifacts(env):
    ws, store = env
    setup = _Setup(ws, scripted(), "fake", "fake", Budget())
    services = _build_services(setup, store, Console(), True)
    assert set(services.registry.names()) == {"read", "write", "edit", "bash", "ls", "grep"}
    runtime = runtime_for(env)
    assert "read_artifact" not in runtime._system_prompt
    assert "delegate" not in runtime._system_prompt
    sid = store.create_session(workspace=str(ws), provider="fake", model="fake")
    ref = services.artifact_store.spill(sid, "tool_output", "full output")
    assert services.artifact_store.read(sid, ref.artifact_id) == "full output"


def test_resume_uses_saved_adapter_and_persists_explicit_override(env, monkeypatch):
    ws, store = env
    sid = store.create_session(workspace=str(ws), provider="anthropic", model="saved-model")
    builds = []

    def build(choice, model, script):
        builds.append((choice.value, model))
        adapter = scripted({"text": "ok"})
        adapter.name, adapter.model = choice.value, model
        return adapter, choice.value, model

    monkeypatch.setattr("minicode.cli._build_provider", build)
    setup = _prepare_resume(store.get_session(sid), budget=Budget())
    assert builds == [("anthropic", "saved-model")]
    services = _build_services(setup, store, Console(), True)
    runtime = _make_resumed_runtime(setup, store, store.get_session(sid), services)
    assert runtime.provider.name == runtime.provider_name == store.get_session(sid).provider
    assert runtime.provider.model == runtime.model == store.get_session(sid).model
    other = scripted({"text": "changed"})
    other.name, other.model = "commandcode", "new-model"
    runtime = resume(env, sid, provider=other)
    saved = store.get_session(sid)
    assert runtime.provider_name == saved.provider == "commandcode"
    assert runtime.model == saved.model == "new-model"
    with pytest.raises(ValueError, match="provider_name"):
        resume(env, sid, provider=other, provider_name="anthropic")
    with pytest.raises(ValueError, match="model"):
        runtime.set_model(provider=other, provider_name="commandcode", model="wrong")
    assert store.get_session(sid) == saved


def test_protected_baseline_survives_reopening_store_and_resume(env):
    ws, store = env
    (ws / "guard.txt").write_text("original")
    runtime = runtime_for(env, goal_checker=protected_checker(ws))
    sid = asyncio.run(runtime.run_turn("start")).session_id
    (ws / "guard.txt").write_text("tampered")
    reopened = SqliteStore(store._db_path)
    try:
        # No --acceptance argument is needed to restore the original gate.
        restored = resume((ws, reopened), sid)
        assert restored._goal_checker.protected_snapshot.violations()
        assert asyncio.run(restored.run_turn("continue")).exit_reason == ExitReason.GOAL_NOT_MET
        again = resume((ws, reopened), sid)
        assert again._goal_checker.protected_snapshot.violations()
    finally:
        reopened.close()


def test_resume_rebinds_checker_and_snapshot_and_does_not_reuse_evidence(env, tmp_path):
    ws, store = env
    (ws / "guard.txt").write_text("original")
    sid = asyncio.run(runtime_for(env, goal_checker=protected_checker(ws)).run_turn("start")).session_id
    new_ws = tmp_path / "other"
    new_ws.mkdir()
    (new_ws / "guard.txt").write_text("modified")
    restored = resume(env, sid, workspace=new_ws, goal_checker=protected_checker(ws))
    assert restored.workspace == restored._goal_checker.workspace == new_ws
    assert restored._goal_checker.protected_snapshot.workspace == new_ws
    assert str(new_ws) in restored._system_prompt
    assert store.get_session(sid).workspace == str(new_ws)
    assert restored._goal_checker.protected_snapshot.violations()


def test_legacy_protected_session_does_not_capture_a_new_baseline(env):
    ws, store = env
    sid = store.create_session(workspace=str(ws), provider="fake", model="fake")
    (ws / "guard.txt").write_text("possibly already modified")
    restored = resume(env, sid, goal_checker=protected_checker(ws))
    assert "缺少" in restored._goal_checker.protected_snapshot.violations()[0][1]


def test_deadline_cancels_stream_and_closes_provider(env):
    closed = []

    class SlowProvider:
        name = "slow"

        async def stream(self, **kwargs):
            try:
                await asyncio.sleep(30)
                yield
            finally:
                closed.append(True)

    runtime = runtime_for(env, SlowProvider(), budget=Budget(max_seconds=.05))
    start = time.monotonic()
    result = asyncio.run(runtime.run_turn("go"))
    assert result.exit_reason == ExitReason.TIME_BUDGET
    assert time.monotonic() - start < 2
    assert closed == [True]


@pytest.mark.parametrize("blocking", [False, True])
def test_deadline_stops_running_tool_and_never_starts_next_write(env, monkeypatch, blocking):
    ws, store = env
    provider = scripted({"tool_calls": [{"name": "read", "arguments": {"path": "x"}}, write_call()]})
    runtime = runtime_for(env, provider, budget=Budget(max_seconds=.05))
    cancelled = []

    async def slow_tool(*args):
        try:
            if blocking:
                time.sleep(.1)
                return ToolOutcome(output="finished synchronous work")
            await asyncio.sleep(30)
        finally:
            cancelled.append(True)

    monkeypatch.setattr(runtime._registry.get("read"), "run", slow_tool)
    result = asyncio.run(runtime.run_turn("go"))
    assert result.exit_reason == ExitReason.TIME_BUDGET
    assert cancelled == [True]
    assert not (ws / "late.txt").exists()
    starts = [e.data["name"] for e in store.get_events(result.session_id) if e.type == EventType.TOOL_CALL_START]
    assert starts == ["read"]


def test_deadline_rechecked_after_synchronous_approval(env):
    from minicode.core.models import ApprovalDecision
    from minicode.security import DefaultPolicy

    async def approve(request):
        time.sleep(.1)
        return ApprovalDecision(granted=True)

    runtime = runtime_for(env, scripted({"tool_calls": [write_call()]}),
                          budget=Budget(max_seconds=.05), approval_handler=approve)
    runtime._policy = DefaultPolicy()
    result = asyncio.run(runtime.run_turn("go"))
    assert result.exit_reason == ExitReason.TIME_BUDGET
    assert not (env[0] / "late.txt").exists()


@pytest.mark.parametrize("calls", [[], [write_call()]])
def test_max_tokens_never_completes_or_executes_truncated_calls(env, calls):
    runtime = runtime_for(env, scripted({"text": "partial", "tool_calls": calls,
                                        "stop_reason": StopReason.MAX_TOKENS}))
    result = asyncio.run(runtime.run_turn("go"))
    assert result.exit_reason == ExitReason.MAX_TOKENS
    assert env[1].get_session(result.session_id).status == "max_tokens"
    assert not (env[0] / "late.txt").exists()
    assert not runtime._find_dangling_calls(env[1].get_messages(result.session_id))


@pytest.mark.parametrize("error", [FileNotFoundError(2, "deleted"), PermissionError(13, "denied")])
def test_workspace_fingerprint_handles_failed_stat_without_stale_size(tmp_path, monkeypatch, error):
    (tmp_path / "first.txt").write_text("first")
    target = tmp_path / "missing.txt"
    target.write_text("missing")
    original = Path.stat

    def stat(path, *args, **kwargs):
        if path == target:
            raise error
        return original(path, *args, **kwargs)

    monkeypatch.setattr(Path, "stat", stat)
    assert len(workspace_fingerprint(tmp_path)) == 64
    (tmp_path / "first.txt").unlink()
    assert len(workspace_fingerprint(tmp_path)) == 64


def test_workspace_fingerprint_handles_broken_symlink(tmp_path):
    try:
        (tmp_path / "broken").symlink_to(tmp_path / "absent")
    except OSError:
        pytest.skip("symlinks unavailable")
    assert len(workspace_fingerprint(tmp_path)) == 64


def python_command(snippet):
    exe = sys.executable.replace("\\", "/")
    return f'{"& " if os.name == "nt" else ""}"{exe}" -c "{snippet}"'


def test_goal_fingerprint_and_protection_are_checked_after_all_commands(env):
    ws, _ = env
    (ws / "guard.txt").write_text("original")
    spec = AcceptanceSpec(items=[
        {"id": "protected", "type": "protected", "path": "guard.txt"},
        {"id": "command", "type": "command", "command": python_command(
            "from pathlib import Path; Path('guard.txt').write_text('modified')")},
    ])
    snapshot = ProtectedSnapshot(ws, ["guard.txt"])
    snapshot.capture()
    before = workspace_fingerprint(ws)
    report = asyncio.run(GoalChecker(spec, ws, snapshot).run())
    assert report.items[1].passed
    assert not report.passed and not report.items[0].passed
    assert report.fingerprint == workspace_fingerprint(ws) != before


@pytest.mark.parametrize("cancel", [False, True])
def test_background_lifecycle_records_start_and_lost_before_session_end(env, cancel):
    ws, store = env

    async def scenario():
        manager = BackgroundManager()
        job_id = await manager.start(python_command("import time; time.sleep(30)"), ws, 60)
        proc = manager._procs[job_id]
        runtime = runtime_for(env, background_manager=manager)
        if cancel:
            started = asyncio.Event()

            class Hanging:
                name = "fake"

                async def stream(self, **kwargs):
                    started.set()
                    await asyncio.sleep(30)
                    yield

            runtime._provider = Hanging()
            task = asyncio.create_task(runtime.run_turn("go"))
            await started.wait()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
        else:
            await runtime.run_turn("go")
        assert proc.returncode is not None
        assert manager.get(job_id).status == "lost"
        events = store.get_events(runtime.session_id)
        types = [e.type for e in events]
        assert types.count(EventType.BACKGROUND_JOB_STARTED) == 1
        assert types.count(EventType.BACKGROUND_JOB_LOST) == 1
        assert types.index(EventType.BACKGROUND_JOB_LOST) < types.index(EventType.SESSION_END)
        assert any(job_id in getattr(block, "text", "")
                   for message in store.get_messages(runtime.session_id) for block in message.content)

    asyncio.run(scenario())


def test_resume_records_unfinished_background_job_as_lost_once(env):
    ws, store = env
    sid = store.create_session(workspace=str(ws), provider="fake", model="fake")
    store.append_event(sid, EventType.BACKGROUND_JOB_STARTED, {"job_id": "orphan", "command": "test"})
    asyncio.run(resume(env, sid).run_turn("continue"))
    asyncio.run(resume(env, sid).run_turn("again"))
    assert len([e for e in store.get_events(sid) if e.type == EventType.BACKGROUND_JOB_LOST]) == 1


@pytest.mark.parametrize("kind", ["foreground", "acceptance"])
def test_deadline_kills_running_shell_in_tool_and_goal_check(env, monkeypatch, kind):
    from minicode.tools.command import spawn_shell

    processes = []

    async def spawn(*args):
        proc = await spawn_shell(*args)
        processes.append(proc)
        return proc

    command = python_command("import time; time.sleep(30)")
    if kind == "foreground":
        monkeypatch.setattr("minicode.tools.command.spawn_shell", spawn)
        runtime = runtime_for(env, scripted({"tool_calls": [
            {"name": "bash", "arguments": {"command": command}}, write_call(),
        ]}), budget=Budget(max_seconds=1))
    else:
        monkeypatch.setattr("minicode.goals.checker.spawn_shell", spawn)
        spec = AcceptanceSpec(items=[{"id": "slow", "type": "command", "command": command}])
        runtime = runtime_for(env, goal_checker=GoalChecker(spec, env[0]), budget=Budget(max_seconds=1))
    start = time.monotonic()
    result = asyncio.run(runtime.run_turn("go"))
    assert result.exit_reason == ExitReason.TIME_BUDGET
    assert time.monotonic() - start < 6  # includes process-tree teardown
    assert processes and all(proc.returncode is not None for proc in processes)
    assert not (env[0] / "late.txt").exists()


def test_cancellation_terminates_background_grandchild(env):
    ws, store = env
    # A shell -> parent Python -> child Python tree; the grandchild announces
    # readiness before cancellation and would mutate the workspace if leaked.
    (ws / "child.py").write_text(
        "from pathlib import Path\nimport time\n"
        "Path('ready').write_text('ready')\ntime.sleep(1.5)\n"
        "Path('leaked').write_text('bad')\ntime.sleep(30)\n"
    )
    (ws / "parent.py").write_text(
        "import subprocess, sys, time\nsubprocess.Popen([sys.executable, 'child.py'])\ntime.sleep(30)\n"
    )

    async def scenario():
        manager = BackgroundManager()
        command = python_command("exec(open('parent.py').read())")
        provider = scripted({"tool_calls": [{"name": "bash", "arguments": {"command": command, "background": True}}]})
        runtime = runtime_for(env, provider, background_manager=manager)
        stream = provider.stream

        # Avoid depending on the fake provider's internal turn cursor.
        calls = 0

        async def stream_then_wait(**kwargs):
            nonlocal calls
            calls += 1
            if calls > 1:
                await asyncio.sleep(30)
            async for event in stream(**kwargs):
                yield event

        provider.stream = stream_then_wait
        task = asyncio.create_task(runtime.run_turn("go"))
        try:
            async with asyncio.timeout(15):
                while not (ws / "ready").exists():
                    await asyncio.sleep(.02)
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            await asyncio.sleep(1.7)
            assert not (ws / "leaked").exists()
            events = store.get_events(runtime.session_id)
            assert len([e for e in events if e.type == EventType.BACKGROUND_JOB_STARTED]) == 1
            assert len([e for e in events if e.type == EventType.BACKGROUND_JOB_LOST]) == 1
        finally:
            if not task.done():
                task.cancel()
                await asyncio.gather(task, return_exceptions=True)

    asyncio.run(scenario())


def test_repeated_cancel_does_not_interrupt_background_cleanup(env):
    class Manager(BackgroundManager):
        def __init__(self):
            super().__init__()
            self.cleaning = asyncio.Event()
            self.cleaned = False

        async def cancel_all(self):
            self.cleaning.set()
            await asyncio.sleep(.05)
            self.cleaned = True

    async def scenario():
        manager = Manager()
        entered = asyncio.Event()

        class Waiting:
            name = "waiting"

            async def stream(self, **kwargs):
                entered.set()
                await asyncio.sleep(30)
                yield

        runtime = runtime_for(env, Waiting(), background_manager=manager)
        task = asyncio.create_task(runtime.run_turn("go"))
        await entered.wait()
        task.cancel()
        await manager.cleaning.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert manager.cleaned
        assert env[1].get_session(runtime.session_id).status == "cancelled"

    asyncio.run(scenario())
