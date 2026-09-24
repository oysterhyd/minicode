"""Review journal resumes safely around an interrupted check command."""

import asyncio
import json
import subprocess

import pytest

from minicode.providers import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn
from minicode.workflow import run_review
from minicode.workflow import review as module


class Crash(BaseException):
    pass


def _repo(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    (workspace / "main.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q", str(workspace)], check=True)
    return workspace


def _provider():
    return FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="No findings in this small diff.")]))


def test_review_workflow_writes_journal_and_read_only_report(tmp_path, monkeypatch):
    workspace = _repo(tmp_path)
    output = tmp_path / "review"
    async def check(*args):
        return 0, "1 passed"
    monkeypatch.setattr(module, "_check", check)
    journal = asyncio.run(run_review(workspace, output, "python -m pytest -q", _provider(), "fake"))
    assert journal["snapshot"] == journal["check"] == journal["review"] == "done"
    assert journal["check_exit_code"] == 0
    assert "No findings" in (output / "review.md").read_text(encoding="utf-8")
    assert (output / "review.sqlite3").exists()
    assert {tool.name for tool in module._read_registry()._tools.values()} == {
        "read", "ls", "grep", "read_artifact"}


def test_interrupted_check_requires_explicit_retry(tmp_path, monkeypatch):
    workspace = _repo(tmp_path)
    output = tmp_path / "review"
    calls = 0
    async def crash(*args):
        nonlocal calls
        calls += 1
        raise Crash()
    monkeypatch.setattr(module, "_check", crash)
    with pytest.raises(Crash):
        asyncio.run(run_review(workspace, output, "check", _provider(), "fake"))
    assert json.loads((output / "journal.json").read_text(encoding="utf-8"))["check"] == "running"
    journal = asyncio.run(run_review(workspace, output, "check", _provider(), "fake"))
    assert journal["check"] == "unknown"
    assert calls == 1

    async def successful(*args):
        nonlocal calls
        calls += 1
        return 0, "ok"
    monkeypatch.setattr(module, "_check", successful)
    journal = asyncio.run(run_review(workspace, output, "check", _provider(), "fake",
                                     retry_unknown=True))
    assert calls == 2
    assert journal["review"] == "done"


def test_completed_model_session_is_reused_after_journal_crash(tmp_path, monkeypatch):
    workspace = _repo(tmp_path)
    output = tmp_path / "review"
    async def check(*args):
        return 0, "ok"
    monkeypatch.setattr(module, "_check", check)
    asyncio.run(run_review(workspace, output, "check", _provider(), "fake"))
    journal_path = output / "journal.json"
    data = json.loads(journal_path.read_text(encoding="utf-8"))
    data["review"] = "running"
    journal_path.write_text(json.dumps(data), encoding="utf-8")
    (output / "review.md").unlink()
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="This must not run")]))
    recovered = asyncio.run(run_review(workspace, output, "check", provider, "fake"))
    assert recovered["review"] == "done"
    assert provider.turns_consumed == 0
    assert "No findings" in (output / "review.md").read_text(encoding="utf-8")


def test_paused_model_review_continues_same_session(tmp_path, monkeypatch):
    workspace = _repo(tmp_path)
    output = tmp_path / "review"
    async def check(*args):
        return 0, "ok"
    monkeypatch.setattr(module, "_check", check)
    turns = [FakeTurn(tool_calls=[FakeToolCall(name="read" if index % 2 else "ls",
                                               arguments={"path": "main.py" if index % 2 else "."})])
             for index in range(8)]
    options = FakeProviderOptions(turns=[*turns, FakeTurn(text="Finished after resume")])
    first = asyncio.run(run_review(workspace, output, "check", FakeProvider(options), "fake"))
    assert first["review"] == "paused"
    resumed = asyncio.run(run_review(workspace, output, "check", FakeProvider(options), "fake"))
    assert resumed["review"] == "done"
    assert resumed["session_id"] == first["session_id"]
    assert "Finished after resume" in (output / "review.md").read_text(encoding="utf-8")
