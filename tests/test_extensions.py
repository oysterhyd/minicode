"""Stage C: scoped instructions, lazy skills and restricted child sessions."""

from __future__ import annotations

import asyncio
import json

import pytest

from minicode.context.extensions import ProjectInstructions, SkillCatalog
from minicode.core.models import Budget, EventType, ExitReason, ToolResultBlock
from minicode.providers.fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.runtime.loop import AgentRuntime
from minicode.security.policy import AutoAllowPolicy
from minicode.storage.sqlite_store import SqliteStore
from minicode.tools.registry import default_registry


def _runtime(tmp_path, turns, *, skills=None, instructions=None, delegation=False):
    workspace = tmp_path / "repo"
    workspace.mkdir(exist_ok=True)
    store = SqliteStore(tmp_path / "sessions.db")
    provider = FakeProvider(FakeProviderOptions(turns=turns))
    runtime = AgentRuntime(
        provider=provider, registry=default_registry(skills=skills, delegation=delegation),
        store=store, policy=AutoAllowPolicy(), workspace=workspace,
        provider_name="fake", model="fake-model", budget=Budget(max_rounds=10),
        project_instructions=instructions, skills=skills,
    )
    return workspace, store, runtime


def test_nested_instructions_are_loaded_only_when_scope_is_touched(tmp_path):
    workspace = tmp_path / "repo"
    nested = workspace / "module"
    nested.mkdir(parents=True)
    (workspace / "AGENTS.md").write_text("root rule", encoding="utf-8")
    (nested / "AGENTS.md").write_text("nested rule", encoding="utf-8")
    (nested / "item.py").write_text("value = 1\n", encoding="utf-8")
    instructions = ProjectInstructions(workspace)
    _, store, runtime = _runtime(tmp_path, [
        FakeTurn(tool_calls=[FakeToolCall(name="read", arguments={"path": "module/item.py"})]),
        FakeTurn(tool_calls=[FakeToolCall(name="read", arguments={"path": "module/item.py"})]),
        FakeTurn(text="done"),
    ], instructions=instructions)
    assert "root rule" in runtime._system_prompt
    assert "nested rule" not in runtime._system_prompt
    result = asyncio.run(runtime.run_turn("inspect"))
    assert result.exit_reason is ExitReason.COMPLETED
    blocks = [block for msg in store.get_messages(result.session_id)
              for block in msg.content if isinstance(block, ToolResultBlock)]
    assert blocks[0].is_error and "AGENTS.md" in blocks[0].content
    assert not blocks[1].is_error and "value = 1" in blocks[1].content
    assert "nested rule" in runtime._system_prompt
    events = store.get_events(result.session_id)
    assert any(e.type is EventType.PROJECT_INSTRUCTIONS for e in events)
    (workspace / "AGENTS.md").write_text("changed root rule", encoding="utf-8")
    with pytest.raises(ValueError, match="project instructions changed"):
        AgentRuntime.resume(
            store=store, session_id=result.session_id,
            provider=FakeProvider(FakeProviderOptions()),
            registry=default_registry(), policy=AutoAllowPolicy(),
            workspace=workspace, provider_name="fake", model="fake-model",
            project_instructions=ProjectInstructions(workspace),
        )
    store.close()


def test_skill_body_is_lazy_and_activation_is_idempotent(tmp_path):
    workspace = tmp_path / "repo"
    skill_dir = workspace / ".minicode" / "skills" / "review"
    skill_dir.mkdir(parents=True)
    (skill_dir / "SKILL.md").write_text(
        "---\nname: review\ndescription: Review the diff\n---\nPRIVATE SKILL BODY\n",
        encoding="utf-8",
    )
    (skill_dir / "notes.txt").write_text("reference notes", encoding="utf-8")
    catalog = SkillCatalog(workspace, user_root=tmp_path / "no-user-skills")
    _, store, runtime = _runtime(tmp_path, [
        FakeTurn(tool_calls=[FakeToolCall(name="skill_load", arguments={"name": "review"})]),
        FakeTurn(text="done"),
    ], skills=catalog)
    assert "Review the diff" in runtime._system_prompt
    assert "PRIVATE SKILL BODY" not in runtime._system_prompt
    result = asyncio.run(runtime.run_turn("check"))
    assert result.exit_reason is ExitReason.COMPLETED
    assert runtime._system_prompt.count("PRIVATE SKILL BODY") == 1
    runtime.activate_skill("review")
    assert runtime._system_prompt.count("PRIVATE SKILL BODY") == 1
    runtime.deactivate_skill("review")
    assert "PRIVATE SKILL BODY" not in runtime._system_prompt
    assert catalog.resource("review", "notes.txt").content == "reference notes"
    try:
        catalog.resource("review", "../../outside.txt")
    except ValueError:
        pass
    else:
        raise AssertionError("skill resource escaped its registered directory")
    assert any(e.type is EventType.SKILL_ACTIVATED for e in store.get_events(result.session_id))
    assert any(e.type is EventType.SKILL_DEACTIVATED for e in store.get_events(result.session_id))
    store.close()


def test_delegate_is_read_only_and_charges_parent(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    (workspace / "source.py").write_text("answer = 42\n", encoding="utf-8")
    child_report = json.dumps({
        "summary": "source is present", "findings": ["answer is 42"],
        "evidence_refs": ["source.py", "invented.py"], "unresolved": [],
    })
    _, store, runtime = _runtime(tmp_path, [
        FakeTurn(tool_calls=[FakeToolCall(name="delegate", arguments={
            "kind": "explore", "task": "inspect source and try write",
        })]),
        FakeTurn(tool_calls=[FakeToolCall(name="write", arguments={
            "path": "blocked.py", "content": "bad",
        })]),
        FakeTurn(tool_calls=[FakeToolCall(name="read", arguments={"path": "source.py"})]),
        FakeTurn(text=child_report),
        FakeTurn(text="parent done"),
    ], delegation=True)
    result = asyncio.run(runtime.run_turn("review source"))
    assert result.exit_reason is ExitReason.COMPLETED
    assert not (workspace / "blocked.py").exists()
    assert result.total_usage.total_tokens == 5 * 120
    parent_blocks = [block for msg in store.get_messages(result.session_id)
                     for block in msg.content if isinstance(block, ToolResultBlock)]
    report = json.loads(parent_blocks[0].content)
    assert report["evidence_refs"] == ["source.py"]
    assert "invented.py" in report["unresolved"][0]
    child_id = report["child_session_id"]
    child_blocks = [block for msg in store.get_messages(child_id)
                    for block in msg.content if isinstance(block, ToolResultBlock)]
    assert child_blocks[0].is_error and "unknown tool" in child_blocks[0].content
    events = store.get_events(result.session_id)
    assert any(e.type is EventType.SUBAGENT_START for e in events)
    assert any(e.type is EventType.SUBAGENT_RESULT for e in events)
    store.close()


def test_activated_skill_is_restored_with_version_check(tmp_path):
    workspace = tmp_path / "repo"
    skill_dir = workspace / ".minicode" / "skills" / "inspect"
    skill_dir.mkdir(parents=True)
    skill_file = skill_dir / "SKILL.md"
    skill_file.write_text("---\nname: inspect\ndescription: Inspect code\n---\nDo the inspection.\n", encoding="utf-8")
    catalog = SkillCatalog(workspace, user_root=tmp_path / "empty")
    _, store, runtime = _runtime(tmp_path, [FakeTurn(text="done")], skills=catalog)
    runtime.activate_skill("inspect")
    result = asyncio.run(runtime.run_turn("inspect"))

    def resume():
        return AgentRuntime.resume(
            store=store, session_id=result.session_id,
            provider=FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="done")])),
            registry=default_registry(skills=catalog), policy=AutoAllowPolicy(),
            workspace=workspace, provider_name="fake", model="fake-model",
            skills=catalog, project_instructions=ProjectInstructions(workspace),
        )

    restored = resume()
    assert restored._system_prompt.count("Do the inspection.") == 1
    skill_file.write_text("---\nname: inspect\ndescription: Inspect code\n---\nChanged.\n", encoding="utf-8")
    with pytest.raises(ValueError, match="activated skill changed"):
        resume()
    runtime.deactivate_skill("inspect")
    inactive = resume()
    assert "Do the inspection." not in inactive._system_prompt
    assert "Changed." not in inactive._system_prompt
    store.close()


def test_parent_cancellation_cancels_child(tmp_path):
    started = asyncio.Event()

    class SlowChildProvider(FakeProvider):
        async def stream(self, *, system, messages, tools):
            if any(tool.name == "delegate" for tool in tools):
                async for event in super().stream(system=system, messages=messages, tools=tools):
                    yield event
            else:
                started.set()
                await asyncio.Event().wait()
                if False:
                    yield None

    workspace, store, runtime = _runtime(tmp_path, [], delegation=True)
    (workspace / "source.py").write_text("pass\n", encoding="utf-8")
    runtime._provider = SlowChildProvider(FakeProviderOptions(turns=[
        FakeTurn(tool_calls=[FakeToolCall(name="delegate", arguments={
            "kind": "review", "task": "inspect source",
        })]),
    ]))

    async def exercise():
        task = asyncio.create_task(runtime.run_turn("review"))
        await asyncio.wait_for(started.wait(), 2)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task

    asyncio.run(exercise())
    assert runtime.session_id is not None
    parent_events = store.get_events(runtime.session_id)
    child_id = next(e.data["child_session_id"] for e in parent_events
                    if e.type is EventType.SUBAGENT_START)
    assert store.get_session(child_id).exit_reason == ExitReason.CANCELLED.value
    assert store.get_session(runtime.session_id).exit_reason == ExitReason.CANCELLED.value
    store.close()
