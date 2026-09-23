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

import pytest
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
    """Script: write creates new.txt, then a Chinese final answer."""
    return _write_script(
        tmp_path / "create_file_script.json",
        turns=[
            {
                "tool_calls": [
                    {
                        "name": "write",
                        "arguments": {"path": "new.txt", "content": "hello"},
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
# 1. run: write + final text, --yes
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
            if call["name"] == "bash":
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
    assert rewritten, "script must contain a bash call"
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
    assert "write" in result.output
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


@pytest.mark.parametrize("override", [False, True])
def test_resume_restores_or_explicitly_overrides_provider_and_model(tmp_path, override, monkeypatch):
    ws = tmp_path / "resumed"
    ws.mkdir()
    db = tmp_path / "resume.db"
    store = SqliteStore(db)
    saved_provider = "anthropic" if override else "fake"
    sid = store.create_session(workspace=str(ws), provider=saved_provider, model="saved-model")
    store.close()
    script = _write_script(tmp_path / "resume-script.json", turns=[{"text": "resumed reply"}])
    # Ambient gateway credentials must not override saved provider selection.
    monkeypatch.setenv("COMMANDCODE_API_KEY", "ambient-key")
    args = ["resume", sid, "--db", str(db), "--script", str(script), "--yes"]
    if override:
        args += ["--provider", "fake", "--model", "explicit-model"]
    result = runner.invoke(app, args, input="continue\nexit\n")
    assert result.exit_code == 0, result.output
    assert "resumed reply" in result.output
    store = SqliteStore(db)
    try:
        summary = store.get_session(sid)
        assert summary.provider == "fake"
        assert summary.model == ("explicit-model" if override else "saved-model")
        assert summary.workspace == str(ws)
    finally:
        store.close()


def _chat(ws: Path, script: Path, db: Path, stdin: str, env: dict | None = None):
    """Invoke ``minicode chat`` with *stdin* and hermetic gateway env."""
    return runner.invoke(
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
            str(db),
        ],
        input=stdin,
        env=env,
    )


def test_chat_clear_only_resets_screen_keeps_context(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "clear_script.json", turns=[{"text": "第一条"}])
    result = _chat(ws, script, tmp_path / "db.sqlite3", "/clear\n你好\nexit\n")

    assert result.exit_code == 0, result.output
    # /clear states its semantics: rendering only, context kept.
    assert "已清屏" in result.output
    assert "上下文" in result.output
    assert "第一条" in result.output
    store = SqliteStore(tmp_path / "db.sqlite3")
    try:
        assert len(store.list_sessions()) == 1
    finally:
        store.close()


def test_chat_new_starts_fresh_session(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(
        tmp_path / "new_script.json",
        turns=[{"text": "旧会话"}, {"text": "新会话"}],
    )
    db = tmp_path / "db.sqlite3"
    result = _chat(ws, script, db, "你好\n/new\n再来\nexit\n")

    assert result.exit_code == 0, result.output
    assert "已重置对话上下文" in result.output
    assert "旧会话" in result.output and "新会话" in result.output
    store = SqliteStore(db)
    try:
        sessions = store.list_sessions()
    finally:
        store.close()
    assert len(sessions) == 2  # /new really opened a second session


def test_chat_model_switch_routes_through_catalog(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "model_script.json", turns=[{"text": "好的"}])
    env = {
        "COMMANDCODE_API_KEY": "test-key",
        "COMMANDCODE_BASE_URL": "https://gw.test/provider/v1",
    }
    result = _chat(
        ws,
        script,
        tmp_path / "db.sqlite3",
        "/model\n/model z.ai/glm-5.3-flash\nexit\n",
        env=env,
    )

    assert result.exit_code == 0, result.output
    # No-arg /model lists the catalog with context-window sizes...
    assert "z.ai/glm-5.3-flash" in result.output
    assert "1,000,000" in result.output
    # ...and the explicit switch confirms old -> new.
    assert "已切换模型" in result.output
    assert "fake → z.ai/glm-5.3-flash" in result.output


def test_chat_permissions_switch_updates_mode(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "perm_script.json", turns=[{"text": "好"}])
    result = _chat(
        ws, script, tmp_path / "db.sqlite3", "/permissions\n/permissions accept_edits\nexit\n"
    )

    assert result.exit_code == 0, result.output
    assert "当前权限模式: default" in result.output
    assert "权限模式已切换 default → accept_edits" in result.output


def test_chat_effort_requires_capable_provider(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "effort_script.json", turns=[{"text": "好"}])
    result = _chat(ws, script, tmp_path / "db.sqlite3", "/effort high\nexit\n")

    assert result.exit_code == 0, result.output
    # The fake provider cannot carry a reasoning budget; the command says so
    # instead of silently dropping the setting.
    assert "不支持推理预算调整" in result.output


def test_run_prints_banner_with_version_and_env(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    script = _write_script(tmp_path / "banner_script.json", turns=[{"text": "完成"}])
    result = runner.invoke(
        app,
        [
            "run",
            "任务",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(script),
            "--yes",
            "--db",
            str(tmp_path / "db.sqlite3"),
        ],
    )
    assert result.exit_code == 0, result.output
    assert "minicode v" in result.output
    assert "Python" in result.output
    assert "fake/fake" in result.output


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
        {"call_id": "t1", "name": "edit", "arguments": {"path": "a.py"}},
    )
    store.append_event(
        session_id,
        EventType.TOOL_CALL_RESULT,
        {
            "call_id": "t1",
            "name": "edit",
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
    assert "edit" in report_result.output
    assert "+hello" in report_result.output  # diff from the event preview
    assert "事件统计" in report_result.output
    assert "总计" in report_result.output
    assert "已完成 (completed)" in report_result.output


def test_run_header_shows_uncapped_tokens_and_compact_counts(tmp_path):
    """Token display is compact (300k / 1M) and the default budget is 不限."""
    ws = tmp_path / "ws"
    ws.mkdir()
    db = tmp_path / "db.sqlite3"
    script = tmp_path / "big.json"
    script.write_text(
        json.dumps(
            {
                "turns": [
                    {
                        "text": "看完了。",
                        "input_tokens": 300_000,
                        "output_tokens": 84_009,
                    }
                ]
            }
        ),
        encoding="utf-8",
    )

    result = runner.invoke(
        app,
        [
            "run",
            "调研仓库",
            "--workspace",
            str(ws),
            "--provider",
            "fake",
            "--script",
            str(script),
            "--yes",
            "--db",
            str(db),
        ],
    )

    assert result.exit_code == 0, result.output
    assert "token 不限" in result.output  # default: no token cap
    # Counts are compact, never raw six-digit numbers.
    assert "Token: 384k" in result.output
    assert "输入 300k" in result.output
    assert "输出 84k" in result.output
    assert "300000" not in result.output
    assert "单次轮次 不限" in result.output
    assert "时长 不限" in result.output


def test_run_max_tokens_flag_restores_a_hard_cap(tmp_path):
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
            "--max-tokens",
            "1",
            "--db",
            str(db),
        ],
    )

    # An explicit cap still works: the session stops on the budget and says so.
    assert "token ≤ 1" in result.output
    assert "显式 Token 上限已暂停" in result.output
    assert result.exit_code == 2
    assert "token_budget" in result.output


def test_run_default_continues_past_twenty_rounds(tmp_path):
    ws = tmp_path / "ws"
    ws.mkdir()
    turns = []
    for index in range(21):
        (ws / f"{index}.txt").write_text(str(index), encoding="utf-8")
        turns.append({"tool_calls": [{"name": "read", "arguments": {
            "path": f"{index}.txt",
        }}]})
    turns.append({"text": "finished"})
    script = tmp_path / "long.json"
    script.write_text(json.dumps({"turns": turns}), encoding="utf-8")
    db = tmp_path / "sessions.sqlite3"
    result = runner.invoke(app, [
        "run", "inspect all files", "--workspace", str(ws),
        "--provider", "fake", "--script", str(script),
        "--yes", "--db", str(db),
    ])
    assert result.exit_code == 0, result.output
    assert "finished" in result.output
    store = SqliteStore(db)
    try:
        summary = store.list_sessions()[0]
        assert summary.status == "completed"
        assert summary.rounds == 22
    finally:
        store.close()
