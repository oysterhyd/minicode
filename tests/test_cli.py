"""End-to-end tests for the minicode CLI (typer.testing.CliRunner).

Everything runs against the deterministic FakeProvider with tmp workspaces
and tmp SQLite databases, so the suite is fast and needs no network or API
keys. The pagination demo test copies ``examples/pagination`` into tmp_path
and rewrites the scripted test command to use the current interpreter
(``sys.executable``), which is guaranteed to have pytest installed.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
from pathlib import Path

from typer.testing import CliRunner

from minicode.cli import app
from minicode.core.models import EventType
from minicode.storage import SqliteStore

runner = CliRunner()

_PROJECT_ROOT = Path(__file__).resolve().parents[1]


def _write_script(path: Path, turns: list[dict], **options) -> Path:
    payload = {"turns": turns, **options}
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    return path


def _create_file_script(tmp_path: Path) -> Path:
    """Script: apply_patch creates new.txt, then a Chinese final answer."""
    return _write_script(
        tmp_path / "create_file_script.json",
        turns=[
            {
                "tool_calls": [
                    {
                        "name": "apply_patch",
                        "arguments": {"path": "new.txt", "new_text": "hello"},
                    }
                ]
            },
            {"text": "文件已创建，任务完成。"},
        ],
    )


def _run_create_file(tmp_path: Path):
    """Run the create-file demo; returns (result, workspace, db, session_id)."""
    ws = tmp_path / "ws"
    ws.mkdir()
    db = tmp_path / "db.sqlite3"
    result = runner.invoke(
        app,
        [
            "run",
            "创建 new.txt",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(_create_file_script(tmp_path)),
            "--yes",
            "--db",
            str(db),
        ],
    )
    assert result.exit_code == 0, result.output
    store = SqliteStore(db)
    try:
        sessions = store.list_sessions()
    finally:
        store.close()
    assert len(sessions) == 1
    return result, ws, db, sessions[0].session_id


# ---------------------------------------------------------------------------
# 1. run: apply_patch + final text, --yes
# ---------------------------------------------------------------------------


def test_run_creates_file_and_reports_completed(tmp_path):
    result, ws, db, _ = _run_create_file(tmp_path)

    assert (ws / "new.txt").read_text(encoding="utf-8") == "hello"
    assert "文件已创建，任务完成。" in result.output
    assert "退出原因" in result.output
    assert "已完成" in result.output
    assert "修改摘要" in result.output
    assert "✓" in result.output  # successful tool result line


# ---------------------------------------------------------------------------
# 2. run against the pagination fixture with the scripted fix
# ---------------------------------------------------------------------------


def test_run_fixes_pagination_fixture(tmp_path):
    dst = tmp_path / "pagination"
    shutil.copytree(
        _PROJECT_ROOT / "examples" / "pagination",
        dst,
        ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache"),
    )
    # The shipped script uses generic `python`; rewrite it to the interpreter
    # running the tests (guaranteed to have pytest on it).
    script_path = dst / "scripts" / "fix_pagination.json"
    data = json.loads(script_path.read_text(encoding="utf-8"))
    rewritten = False
    for turn in data["turns"]:
        for call in turn.get("tool_calls", []):
            if call["name"] == "run_command":
                # PowerShell needs the & call operator for quoted paths;
                # bash accepts the quoted path directly. No -q: pytest 9
                # hides the "N passed" summary line under double quiet.
                if os.name == "nt":
                    call["arguments"]["command"] = (
                        f'& "{sys.executable}" -m pytest test_paginate.py'
                    )
                else:
                    call["arguments"]["command"] = (
                        f'"{sys.executable}" -m pytest test_paginate.py'
                    )
                rewritten = True
    assert rewritten, "script must contain a run_command call"
    script_path.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")

    result = runner.invoke(
        app,
        [
            "run",
            "修复分页 bug",
            "--workspace",
            str(dst),
            "--provider",
            "fake",
            "--script",
            str(script_path),
            "--yes",
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
    )

    assert result.exit_code == 0, result.output
    fixed = (dst / "paginate.py").read_text(encoding="utf-8")
    assert "page_size + 1" not in fixed  # the bug line is gone
    assert "end = start + page_size" in fixed
    assert "4 passed" in result.output  # pytest run reported via tool result
    assert "✓" in result.output
    assert "修改摘要" in result.output


# ---------------------------------------------------------------------------
# 3. run error paths + sessions/report around the created session
# ---------------------------------------------------------------------------


def test_run_bad_script_path_fails_cleanly(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    result = runner.invoke(
        app,
        [
            "run",
            "x",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(tmp_path / "missing.json"),
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
    )
    assert result.exit_code != 0
    assert "脚本" in result.output


def test_run_nonexistent_workspace_fails_cleanly(tmp_path):
    result = runner.invoke(
        app,
        [
            "run",
            "x",
            "--workspace",
            str(tmp_path / "missing_ws"),
            "--provider",
            "fake",
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
    )
    assert result.exit_code != 0
    assert "工作区" in result.output


def test_sessions_list_shows_completed_session(tmp_path):
    _, _, db, session_id = _run_create_file(tmp_path)

    # Cross-check the store contents directly.
    store = SqliteStore(db)
    try:
        summary = store.get_session(session_id)
        assert summary is not None
        assert summary.status == "completed"
    finally:
        store.close()

    result = runner.invoke(app, ["sessions", "list", "--db", str(db)])
    assert result.exit_code == 0, result.output
    assert session_id[:8] in result.output
    assert "已完成" in result.output


def test_report_prints_diff_for_session(tmp_path):
    _, _, db, session_id = _run_create_file(tmp_path)

    result = runner.invoke(app, ["report", session_id, "--db", str(db)])
    assert result.exit_code == 0, result.output
    assert "apply_patch" in result.output
    assert "+hello" in result.output  # the full diff for the created file
    assert "已完成" in result.output
    assert "退出原因" in result.output


def test_report_unknown_session_fails_cleanly(tmp_path):
    _, _, db, _ = _run_create_file(tmp_path)

    result = runner.invoke(app, ["report", "ffffffff" * 4, "--db", str(db)])
    assert result.exit_code != 0
    assert "未找到" in result.output


# ---------------------------------------------------------------------------
# 4. chat REPL over CliRunner-fed stdin
# ---------------------------------------------------------------------------


def test_chat_replies_then_exits(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(
        tmp_path / "chat_script.json",
        turns=[{"text": "你好，我是 minicode 助手。"}],
    )

    result = runner.invoke(
        app,
        [
            "chat",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(script),
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
        input="你好\nexit\n",
    )

    assert result.exit_code == 0, result.output
    assert "你好，我是 minicode 助手。" in result.output  # streamed reply
    assert "你 >" in result.output  # the REPL prompt rendered
    assert "退出原因" in result.output  # per-turn summary


def test_chat_exits_on_eof(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "chat_script.json", turns=[{"text": "在的。"}])

    result = runner.invoke(
        app,
        [
            "chat",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(script),
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
        input="你好\n",  # no "exit": the REPL ends on EOF (Ctrl+D)
    )

    assert result.exit_code == 0, result.output
    assert "在的。" in result.output


# ---------------------------------------------------------------------------
# 5. sessions list / report in isolation over a pre-seeded db
# ---------------------------------------------------------------------------


def test_sessions_list_and_report_with_seeded_db(tmp_path):
    db = tmp_path / "seeded.sqlite3"
    store = SqliteStore(db)
    session_id = store.create_session(
        workspace=str(tmp_path / "ws"), provider="fake", model="fake"
    )
    store.append_event(
        session_id,
        EventType.SESSION_START,
        {"workspace": "ws", "provider": "fake", "model": "fake"},
    )
    store.append_event(session_id, EventType.ROUND_START, {"round": 1})
    store.append_event(
        session_id,
        EventType.TOOL_CALL_START,
        {"call_id": "t1", "name": "apply_patch", "arguments": {"path": "a.py"}},
    )
    store.append_event(
        session_id,
        EventType.TOOL_CALL_RESULT,
        {
            "call_id": "t1",
            "name": "apply_patch",
            "success": True,
            "exit_code": None,
            "error": None,
            "output_preview": (
                "Applied patch to a.py (+1 -0)\n"
                "--- a/a.py\n+++ b/a.py\n@@ -0,0 +1 @@\n+hello"
            ),
        },
    )
    store.append_event(
        session_id,
        EventType.SESSION_END,
        {
            "exit_reason": "completed",
            "rounds": 2,
            "total_usage": {
                "input_tokens": 120,
                "output_tokens": 40,
                "total_tokens": 160,
            },
        },
    )
    store.update_session(
        session_id,
        status="completed",
        exit_reason="completed",
        rounds=2,
        input_tokens=120,
        output_tokens=40,
    )
    store.close()

    list_result = runner.invoke(app, ["sessions", "list", "--db", str(db)])
    assert list_result.exit_code == 0, list_result.output
    assert session_id[:8] in list_result.output
    assert "已完成" in list_result.output
    assert "fake/fake" in list_result.output

    report_result = runner.invoke(app, ["report", session_id, "--db", str(db)])
    assert report_result.exit_code == 0, report_result.output
    assert "apply_patch" in report_result.output
    assert "+hello" in report_result.output  # diff from the event preview
    assert "事件统计" in report_result.output
    assert "总计" in report_result.output
    assert "已完成 (completed)" in report_result.output
