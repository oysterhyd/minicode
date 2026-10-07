"""Deterministic regression tests for audit findings in the shared runtime."""
from __future__ import annotations

import asyncio
import time

import pytest

from minicode.core.models import Budget, EventType, ExitReason, TextBlock
from minicode.providers import FakeProvider
from minicode.runtime.events import EventRecorder
from minicode.runtime.loop import AgentRuntime
from minicode.security.policy import ModePolicy, PermissionMode
from minicode.storage import SqliteStore
from minicode.tasks.background import BackgroundJob, BackgroundManager
from minicode.tasks.taskstore import TaskStore
from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.files import EditTool
from minicode.tools.registry import default_registry


def runtime(store, workspace, **kwargs):
    return AgentRuntime(provider=FakeProvider(), provider_name="fake", model="fake", registry=default_registry(), store=store,
                        policy=ModePolicy(PermissionMode.BYPASS), workspace=workspace, **kwargs)


@pytest.mark.parametrize("initial,updated", [(0, 99), (0, 1), (1000, 99), (1000, -1)])
def test_latest_budget_survives_recovery(tmp_path, initial, updated):
    with SqliteStore(tmp_path / "sessions.db") as store:
        agent = runtime(store, tmp_path, budget=Budget(max_total_tokens=initial))
        result = asyncio.run(agent.run_turn("hello"))
        agent.set_budget(Budget(max_total_tokens=updated))
        resumed = AgentRuntime.resume(store=store, session_id=result.session_id,
                                      provider=FakeProvider(), registry=default_registry(),
                                      policy=ModePolicy(), budget=Budget())
        assert resumed.budget.max_total_tokens == updated
        if updated > 0 and resumed.usage.total_tokens > updated:
            continued = asyncio.run(resumed.run_turn("more"))
            assert continued.exit_reason is ExitReason.TOKEN_BUDGET


def test_background_delivery_cancelled_at_observer_retains_both_results(tmp_path):
    async def scenario():
        with SqliteStore(tmp_path / "sessions.db") as store:
            manager = BackgroundManager()
            agent = runtime(store, tmp_path, background_manager=manager)
            sid = (await agent.run_turn("hello")).session_id
            for index in (1, 2):
                job = BackgroundJob(job_id=f"job-{index}", command="echo result", cwd=str(tmp_path), status="completed",
                                    output=f"unique-result-{index}", exit_code=0)
                manager._jobs[job.job_id] = job
                manager._pending.add(job.job_id)
                agent._announced_jobs.add(job.job_id)
                store.append_event(sid, EventType.BACKGROUND_JOB_STARTED, {"job_id": job.job_id, "command": job.command})
            cancelled = False

            async def observer(event):
                nonlocal cancelled
                if event.type is EventType.BACKGROUND_JOB_COMPLETED and not cancelled:
                    cancelled = True
                    raise asyncio.CancelledError

            with pytest.raises(asyncio.CancelledError):
                await agent._deliver_finished_jobs(EventRecorder(store, sid, observer), sid)
            await agent._deliver_finished_jobs(EventRecorder(store, sid), sid)
            await agent._deliver_finished_jobs(EventRecorder(store, sid), sid)
            for messages in (agent.messages, store.get_messages(sid)):
                text = "\n".join(b.text for m in messages for b in m.content if isinstance(b, TextBlock))
                assert text.count("unique-result-1") == 1
                assert text.count("unique-result-2") == 1
            assert len([e for e in store.get_events(sid) if e.type is EventType.BACKGROUND_JOB_COMPLETED]) == 2
            resumed = AgentRuntime.resume(store=store, session_id=sid, provider=FakeProvider(),
                                          registry=default_registry(), policy=ModePolicy())
            assert resumed._lost_jobs == []
    asyncio.run(scenario())


@pytest.mark.parametrize("large", [False, True])
@pytest.mark.parametrize("old,new", [("two", "TWO"), ("one\r\ntwo", "one\r\nTWO")])
def test_exact_edit_preserves_crlf_bytes(tmp_path, large, old, new):
    target = tmp_path / "file.txt"
    original = b"one\r\ntwo\nthree\rfour\r\n"
    target.write_bytes(original)
    ctx = ToolContext(workspace=tmp_path, limits=ToolLimits(max_read_bytes=1 if large else 1000))
    result = asyncio.run(EditTool().run({"path": "file.txt", "old_text": old, "new_text": new}, ctx))
    assert result.success
    assert target.read_bytes() == original.replace(old.encode(), new.encode())


def test_large_edit_timeout_does_not_commit_or_leave_temp_files(tmp_path, monkeypatch):
    from minicode.tools import files
    original_decoder = files.codecs.getincrementaldecoder

    def slow_decoder(encoding):
        factory = original_decoder(encoding)
        class SlowDecoder:
            def __init__(self, **kwargs):
                self.decoder = factory(**kwargs)
            def decode(self, chunk, final=False):
                time.sleep(.01)
                return self.decoder.decode(chunk, final=final)
        return SlowDecoder

    monkeypatch.setattr(files.codecs, "getincrementaldecoder", slow_decoder)
    target = tmp_path / "large.txt"
    original = b"x" * 400_000 + b"needle"
    target.write_bytes(original)
    ctx = ToolContext(workspace=tmp_path, limits=ToolLimits(max_read_bytes=1))

    async def scenario():
        with pytest.raises(TimeoutError):
            async with asyncio.timeout(.02):
                await EditTool().run({"path": "large.txt", "old_text": "needle", "new_text": "MUTATED"}, ctx)
    asyncio.run(scenario())
    assert target.read_bytes() == original
    assert list(tmp_path.glob(".large.txt.*")) == []


def test_large_edit_cancelled_after_temp_creation_never_commits(tmp_path, monkeypatch):
    from minicode.tools import files
    original_mkstemp = files.tempfile.mkstemp
    def cancel_after_temp(*args, **kwargs):
        result = original_mkstemp(*args, **kwargs)
        asyncio.get_running_loop().call_soon(asyncio.current_task().cancel)
        return result
    monkeypatch.setattr(files.tempfile, "mkstemp", cancel_after_temp)
    target = tmp_path / "large.txt"
    original = b"needle" + b"x" * 400_000
    target.write_bytes(original)
    ctx = ToolContext(workspace=tmp_path, limits=ToolLimits(max_read_bytes=1))
    async def scenario():
        with pytest.raises(asyncio.CancelledError):
            await EditTool().run({"path": "large.txt", "old_text": "needle", "new_text": "MUTATED"}, ctx)
    asyncio.run(scenario())
    assert target.read_bytes() == original
    assert list(tmp_path.glob(".large.txt.*")) == []


def test_task_created_after_dependencies_finish_is_ready(tmp_path):
    with SqliteStore(tmp_path / "tasks.db") as store:
        tasks = TaskStore(store)
        tasks.add("s", "a", "A")
        tasks.claim("s", "a", "worker")
        tasks.complete("s", "a", True, owner="worker")
        tasks.add("s", "b", "B", depends_on=["a"])
        tasks.claim("s", "b", "worker")
        assert next(t for t in tasks.list_tasks("s") if t.task_id == "b").status == "running"
        tasks.add("s", "c", "C", depends_on=["a", "b"])
        assert next(t for t in tasks.list_tasks("s") if t.task_id == "c").status == "pending"
        tasks.complete("s", "b", False, owner="worker")
        tasks.add("s", "d", "D", depends_on=["b"])
        assert next(t for t in tasks.list_tasks("s") if t.task_id == "d").status == "pending"
