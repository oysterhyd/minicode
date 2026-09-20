"""Tests for minicode.tasks.subagent and minicode.tools.delegate — plain sync
pytest driving async with ``asyncio.run`` (no pytest-asyncio)."""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass

import pytest

from minicode.core.models import Budget, ToolResultBlock
from minicode.providers import (
    FakeProvider,
    FakeProviderOptions,
    FakeToolCall,
    FakeTurn,
)
from minicode.storage import SqliteStore
from minicode.tasks.subagent import SubagentResult, SubagentRunner, validate_refs
from minicode.tools.base import ToolContext
from minicode.tools.delegate import DelegateSubagentTool

# ---------------------------------------------------------------------------
# Harness: tmp workspace with one known file, shared store, scripted provider
# ---------------------------------------------------------------------------


@pytest.fixture()
def env(tmp_path):
    """(workspace, store) pair; the store is closed at teardown."""
    workspace = tmp_path / "ws"
    workspace.mkdir()
    (workspace / "notes.txt").write_text("hello world\n", encoding="utf-8")
    store = SqliteStore(tmp_path / "db.sqlite3")
    yield workspace, store
    store.close()


_SUMMARY_PAYLOAD = {
    "summary": "notes.txt says hello",
    "findings": ["file contains a greeting"],
    "evidence_refs": ["notes.txt:1"],
    "unresolved": [],
}


def _read_then_json_turns():
    """turn1 reads notes.txt, turn2 answers with the JSON object."""
    return [
        FakeTurn(
            tool_calls=[
                FakeToolCall(name="read_file", arguments={"path": "notes.txt"})
            ]
        ),
        FakeTurn(
            text="```json\n" + json.dumps(_SUMMARY_PAYLOAD, ensure_ascii=False) + "\n```"
        ),
    ]


def tool_result_blocks(store: SqliteStore, session_id: str) -> list[ToolResultBlock]:
    return [
        block
        for message in store.get_messages(session_id)
        for block in message.content
        if isinstance(block, ToolResultBlock)
    ]


# ---------------------------------------------------------------------------
# SubagentRunner
# ---------------------------------------------------------------------------


def test_subagent_runs_read_only_tool_and_parses_json(env):
    workspace, store = env
    provider = FakeProvider(FakeProviderOptions(turns=_read_then_json_turns()))
    runner = SubagentRunner(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )

    result = asyncio.run(runner.run("研究 notes.txt 的内容"))

    assert result.summary == "notes.txt says hello"
    assert result.findings == ["file contains a greeting"]
    assert result.evidence_refs == ["notes.txt:1"]
    assert result.unresolved == []
    # rounds: one for the tool call, one for the final JSON answer.
    assert result.rounds == 2
    # FakeProvider defaults: 100 in + 20 out per turn, two turns.
    assert result.input_tokens == 200
    assert result.output_tokens == 40

    # The child session was persisted in the shared parent store.
    sessions = store.list_sessions()
    assert len(sessions) == 1
    messages = store.get_messages(sessions[0].session_id)
    assert [m.role for m in messages] == ["user", "assistant", "user", "assistant"]


def test_subagent_json_without_markdown_fence(env):
    workspace, store = env
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(text=json.dumps(_SUMMARY_PAYLOAD, ensure_ascii=False)),
            ]
        )
    )
    runner = SubagentRunner(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )
    result = asyncio.run(runner.run("go"))
    assert result.summary == "notes.txt says hello"
    assert result.findings == ["file contains a greeting"]


def test_subagent_non_json_text_becomes_summary(env):
    workspace, store = env
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="plain prose")]))
    runner = SubagentRunner(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )
    result = asyncio.run(runner.run("go"))
    assert result.summary == "plain prose"
    assert result.findings == []
    assert result.evidence_refs == []


def test_subagent_is_read_only_unknown_write_tool_backfilled(env):
    workspace, store = env
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(
                    tool_calls=[
                        FakeToolCall(
                            name="apply_patch",
                            arguments={"path": "evil.txt", "new_text": "owned"},
                        )
                    ]
                ),
                FakeTurn(text="cannot write; stopping"),  # loop continued past the error
            ]
        )
    )
    runner = SubagentRunner(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )

    result = asyncio.run(runner.run("try to modify a file"))

    # The write tool is not registered at all, so the loop backfilled an
    # unknown-tool error and continued to completion — the workspace is
    # untouched (read-only proof).
    assert not (workspace / "evil.txt").exists()
    assert result.rounds == 2

    session_id = store.list_sessions()[0].session_id
    blocks = tool_result_blocks(store, session_id)
    assert len(blocks) == 1
    assert blocks[0].is_error is True
    assert "unknown tool: apply_patch" in blocks[0].content


def test_subagent_max_rounds_inherits_parent_caps(env):
    workspace, store = env
    # Three tool-call turns but only two rounds allowed: the budget fires.
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(
                    tool_calls=[FakeToolCall(name="list_files", arguments={})]
                ),
                FakeTurn(
                    tool_calls=[FakeToolCall(name="list_files", arguments={})]
                ),
                FakeTurn(
                    tool_calls=[FakeToolCall(name="list_files", arguments={})]
                ),
            ]
        )
    )
    runner = SubagentRunner(
        provider=provider,
        workspace=workspace,
        parent_budget=Budget(max_rounds=2, max_total_tokens=999_999, max_seconds=600),
        store=store,
    )
    result = asyncio.run(runner.run("list everything", max_rounds=2))
    assert result.rounds == 2


# ---------------------------------------------------------------------------
# validate_refs
# ---------------------------------------------------------------------------


def test_validate_refs_keeps_existing_paths_moves_missing_to_unresolved(tmp_path):
    (tmp_path / "notes.txt").write_text("x\n", encoding="utf-8")
    result = SubagentResult(
        summary="s",
        evidence_refs=["notes.txt:1", "missing.py:3", "artifact:abc123"],
    )
    out = validate_refs(result, tmp_path)
    # path:line refs are checked; non path:line shapes pass through untouched.
    assert out.evidence_refs == ["notes.txt:1", "artifact:abc123"]
    assert out.unresolved == ["missing.py:3"]


def test_validate_refs_rejects_paths_outside_workspace(tmp_path):
    outside = tmp_path.parent / "outside.py"
    outside.write_text("secret\n", encoding="utf-8")
    try:
        result = SubagentResult(summary="s", evidence_refs=["../outside.py:1"])
        out = validate_refs(result, tmp_path)
        assert out.evidence_refs == []
        assert out.unresolved == ["../outside.py:1"]
    finally:
        outside.unlink()


# ---------------------------------------------------------------------------
# DelegateSubagentTool end to end
# ---------------------------------------------------------------------------


@dataclass
class BrokenStore:
    """Store whose create_session raises — simulates a storage crash."""

    def create_session(self, **kwargs):  # noqa: ANN003 - test double
        raise RuntimeError("db boom")


def test_delegate_tool_end_to_end(env):
    workspace, store = env
    provider = FakeProvider(FakeProviderOptions(turns=_read_then_json_turns()))
    tool = DelegateSubagentTool(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )

    outcome = asyncio.run(
        tool.run({"task": "研究 notes.txt"}, ToolContext(workspace=workspace))
    )

    assert outcome.success is True
    assert outcome.error is None
    # One line each: summary, findings, unresolved, evidence.
    assert "摘要: notes.txt says hello" in outcome.output
    assert "发现: file contains a greeting" in outcome.output
    assert "未解决: （无）" in outcome.output
    assert "证据: notes.txt:1" in outcome.output
    # Subagent usage is attached to the outcome.
    assert outcome.usage is not None
    assert outcome.usage.input_tokens == 200
    assert outcome.usage.output_tokens == 40


def test_delegate_uses_constructed_workspace_not_ctx(env, tmp_path):
    workspace, store = env
    other = tmp_path / "other"
    other.mkdir()  # notes.txt only exists under the constructed workspace
    provider = FakeProvider(FakeProviderOptions(turns=_read_then_json_turns()))
    tool = DelegateSubagentTool(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )

    outcome = asyncio.run(
        tool.run({"task": "read it"}, ToolContext(workspace=other))
    )

    assert outcome.success is True
    # The read succeeded against the constructed workspace, so the child's
    # tool result is not an error and carries the file content.
    session_id = store.list_sessions()[0].session_id
    blocks = tool_result_blocks(store, session_id)
    assert blocks[0].is_error is False
    assert "hello world" in blocks[0].content


def test_delegate_tool_invalid_args_rejected_without_running(env):
    workspace, store = env
    provider = FakeProvider(FakeProviderOptions(turns=[]))
    tool = DelegateSubagentTool(
        provider=provider, workspace=workspace, parent_budget=Budget(), store=store
    )
    outcome = asyncio.run(tool.run({"task": "x", "max_rounds": 99}, ToolContext(workspace=workspace)))
    assert outcome.success is False
    assert "invalid arguments" in (outcome.error or "")
    assert provider.turns_consumed == 0  # never reached the provider


def test_delegate_tool_crash_becomes_failure_outcome(env):
    workspace, store = env
    tool = DelegateSubagentTool(
        provider=FakeProvider(FakeProviderOptions(turns=[])),
        workspace=workspace,
        parent_budget=Budget(),
        store=BrokenStore(),  # type: ignore[arg-type]
    )
    outcome = asyncio.run(tool.run({"task": "x"}, ToolContext(workspace=workspace)))
    assert outcome.success is False
    assert "subagent failed" in (outcome.error or "")
    assert "db boom" in (outcome.error or "")
