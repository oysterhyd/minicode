"""Resumable read-only code review with a host-owned test step.

The journal is written before and after each stage. A check command left in
``running`` state after a process crash becomes ``unknown`` and is never
reissued without an explicit retry. The model stage has only read tools and
can continue its durable AgentRuntime session after interruption.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
from pathlib import Path
from typing import Any

from minicode.core.clock import utc_now
from minicode.core.models import Budget, ExitReason, TextBlock
from minicode.goals.checker import workspace_fingerprint
from minicode.runtime import AgentRuntime
from minicode.security import DefaultPolicy
from minicode.storage import ArtifactStore, SqliteStore
from minicode.tools.artifacts import ReadArtifactTool
from minicode.tools.command import capture_bounded, decode_shell_output, spawn_shell
from minicode.tools.files import LsTool, ReadTool
from minicode.tools.registry import ToolRegistry
from minicode.tools.search import GrepTool


def _atomic_json(path: Path, data: dict[str, Any]) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def _read_registry() -> ToolRegistry:
    registry = ToolRegistry()
    for tool in (ReadTool(), LsTool(), GrepTool(), ReadArtifactTool()):
        registry.register(tool)
    return registry


def _review_text(store: SqliteStore, session_id: str) -> str:
    messages = store.get_messages(session_id)
    return "\n".join(block.text for message in messages if message.role == "assistant"
                     for block in message.content if isinstance(block, TextBlock))


def _git_snapshot(workspace: Path) -> str:
    parts = []
    for args in (["git", "status", "--short"], ["git", "diff", "--no-ext-diff", "--binary"]):
        result = subprocess.run(args, cwd=workspace, capture_output=True, text=True,
                                encoding="utf-8", errors="replace", timeout=30, check=False)
        if result.returncode != 0:
            raise ValueError(f"git snapshot failed ({args[1]}): {result.stderr.strip()}")
        parts.append(result.stdout)
    return "Git status:\n" + parts[0] + "\nGit diff:\n" + parts[1]


async def _check(command: str, workspace: Path, timeout_s: float) -> tuple[int, str]:
    process = await spawn_shell(command, workspace)
    raw, timed_out, overflow = await capture_bounded(process, timeout_s, max_bytes=10_000_000)
    code = 124 if timed_out else (125 if overflow else process.returncode)
    return code, decode_shell_output(raw)


async def run_review(
    workspace: Path, output: Path, check_command: str, provider: Any, model: str,
    *, retry_unknown: bool = False, check_timeout_s: float = 120.0,
) -> dict[str, Any]:
    """Run or resume snapshot → check → read-only model review."""
    workspace = Path(workspace).resolve()
    output = Path(output).resolve()
    if not workspace.is_dir() or not check_command.strip():
        raise ValueError("an existing workspace and nonempty check command are required")
    if output.is_relative_to(workspace):
        raise ValueError("review output must be outside the workspace being reviewed")
    journal_path = output / "journal.json"
    if journal_path.exists():
        journal = json.loads(journal_path.read_text(encoding="utf-8"))
        if (journal["workspace"] != str(workspace) or journal["check_command"] != check_command
                or journal["model"] != model):
            raise ValueError("review journal belongs to a different workspace, check, or model")
        if workspace_fingerprint(workspace) != journal["fingerprint"]:
            raise ValueError("workspace content changed; start a new review output directory")
    else:
        if output.exists() and any(output.iterdir()):
            raise ValueError("new review output directory must be empty")
        output.mkdir(parents=True, exist_ok=True)
        journal = {"workspace": str(workspace), "check_command": check_command,
                   "model": model, "fingerprint": workspace_fingerprint(workspace),
                   "created_at": utc_now(), "snapshot": "pending", "check": "pending",
                   "review": "pending", "check_exit_code": None, "session_id": None}
        _atomic_json(journal_path, journal)

    if journal["snapshot"] != "done":
        journal["snapshot"] = "running"
        _atomic_json(journal_path, journal)
        (output / "diff.patch").write_text(_git_snapshot(workspace), encoding="utf-8")
        journal["snapshot"] = "done"
        _atomic_json(journal_path, journal)

    if journal["check"] == "running":
        journal["check"] = "unknown"
        _atomic_json(journal_path, journal)
    if journal["check"] == "unknown":
        if not retry_unknown:
            return journal
        journal["check"] = "pending"
        _atomic_json(journal_path, journal)
    if journal["check"] == "pending":
        journal["check"] = "running"
        _atomic_json(journal_path, journal)
        code, log = await _check(check_command, workspace, check_timeout_s)
        (output / "check.log").write_text(log, encoding="utf-8")
        journal["check_exit_code"] = code
        journal["check"] = "done"
        _atomic_json(journal_path, journal)
        if workspace_fingerprint(workspace) != journal["fingerprint"]:
            raise ValueError("check command changed workspace source; review stopped")

    if journal["review"] == "done":
        return journal
    journal["review"] = "running"
    _atomic_json(journal_path, journal)
    registry = _read_registry()
    with SqliteStore(output / "review.sqlite3") as store:
        try:
            kwargs = dict(provider=provider, registry=registry, store=store,
                          policy=DefaultPolicy(), workspace=workspace,
                          provider_name=provider.name, model=model,
                          artifact_store=ArtifactStore(store),
                          budget=Budget(max_rounds=8, max_total_tokens=80_000, max_seconds=300))
            session_id = journal.get("session_id")
            if not session_id:
                previous = store.list_sessions()
                if previous:
                    session_id = previous[0].session_id
            if session_id:
                saved = store.get_session(session_id)
                if saved is not None and saved.exit_reason == ExitReason.COMPLETED.value:
                    (output / "review.md").write_text(_review_text(store, session_id) + "\n",
                                                      encoding="utf-8")
                    journal["session_id"] = session_id
                    journal["review"] = "done"
                    _atomic_json(journal_path, journal)
                    return journal
                runtime = AgentRuntime.resume(session_id=session_id, **kwargs)
                result = await runtime.continue_turn()
            else:
                runtime = AgentRuntime(**kwargs)
                prompt = (
                    "只读审查当前代码改动。列出有证据的缺陷，给出文件与行号；没有发现则说明检查范围。"
                    "不得修改文件。\n\n" + (output / "diff.patch").read_text(encoding="utf-8")[-20_000:]
                    + "\n\n检查命令退出码：" + str(journal["check_exit_code"])
                    + "\n检查输出末尾：\n" + (output / "check.log").read_text(encoding="utf-8")[-8000:]
                )
                result = await runtime.run_turn(prompt)
            journal["session_id"] = result.session_id
            if result.exit_reason is ExitReason.COMPLETED:
                (output / "review.md").write_text(_review_text(store, result.session_id) + "\n",
                                                  encoding="utf-8")
                journal["review"] = "done"
            else:
                journal["review"] = "paused"
                journal["review_exit_reason"] = result.exit_reason.value
            _atomic_json(journal_path, journal)
            return journal
        finally:
            await registry.aclose()
