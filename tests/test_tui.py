"""Pilot-driven tests for the minicode Textual TUI.

Every test runs the real :class:`MiniCodeApp` headless via ``App.run_test()``
against the deterministic FakeProvider and a tmp SqliteStore — no network and
no API keys. The app is constructed directly (setup / store / services
injected) instead of through ``run_tui``, which only adds ``app.run()`` plus
store teardown and is verified here via its signature.
"""

from __future__ import annotations

import asyncio
import inspect
import time
from pathlib import Path

import pytest
from rich.console import Console

from minicode.cli import _Setup, _build_services
from minicode.core.models import Budget
from minicode.providers import FakeProvider, FakeProviderOptions, FakeTurn
from minicode.storage import SqliteStore
from minicode.ui.app import ApprovalModal, MiniCodeApp, ToolCard, run_tui

#: Generous timeout so a busy CI machine cannot flake, small enough to fail fast.
_WAIT_TIMEOUT_S = 20.0


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_app(tmp_path: Path, turns: list[dict], yes: bool = False) -> tuple[MiniCodeApp, Path]:
    """Build the TUI app around a FakeProvider script and a tmp workspace."""
    ws = tmp_path / "ws"
    ws.mkdir()
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(**t) for t in turns]))
    setup = _Setup(
        workspace=ws,
        provider=provider,
        provider_name="fake",
        model_label="fake",
        budget=Budget(max_rounds=10, max_total_tokens=100_000, max_seconds=60.0),
        acceptance=None,
    )
    store = SqliteStore(tmp_path / "db.sqlite3")
    services = _build_services(setup, store, Console(), yes)
    app = MiniCodeApp(setup=setup, store=store, services=services, yes=yes)
    return app, ws


def _run(coro) -> None:
    """Run one async Pilot scenario to completion (sync test entry point)."""
    asyncio.run(coro)


async def _wait_turn_done(app: MiniCodeApp) -> None:
    """Wait until the current turn worker is finished (or fail loudly)."""
    deadline = time.monotonic() + _WAIT_TIMEOUT_S
    while app._busy or (
        app._turn_worker is not None and not app._turn_worker.is_finished
    ):
        if time.monotonic() > deadline:
            raise AssertionError("回合未在限时内结束")
        await asyncio.sleep(0.02)


async def _wait_for_modal(app: MiniCodeApp) -> ApprovalModal:
    """Wait until the approval modal is on the screen stack."""
    deadline = time.monotonic() + _WAIT_TIMEOUT_S
    while True:
        for screen in app.screen_stack:
            if isinstance(screen, ApprovalModal):
                return screen
        if time.monotonic() > deadline:
            raise AssertionError("审批弹窗未在限时内出现")
        await asyncio.sleep(0.02)


async def _submit(pilot, text: str) -> None:
    """Type *text* into the focused prompt and press Enter."""
    await pilot.press(*text)
    await pilot.pause()
    await pilot.press("enter")


async def _wait_log_empty(app: MiniCodeApp) -> None:
    """/clear removes children asynchronously; wait for the removal to land."""
    deadline = time.monotonic() + _WAIT_TIMEOUT_S
    while list(app.query_one("#log").children):
        if time.monotonic() > deadline:
            raise AssertionError("日志区未在限时内清空")
        await asyncio.sleep(0.02)


def _create_file_script() -> list[dict]:
    """apply_patch creates new.txt, then the model gives a final answer."""
    return [
        {
            "tool_calls": [
                {
                    "name": "apply_patch",
                    "arguments": {"path": "new.txt", "new_text": "hello"},
                }
            ]
        },
        {"text": "文件已创建，任务完成。"},
    ]


# ---------------------------------------------------------------------------
# Startup, layout and /help
# ---------------------------------------------------------------------------


def test_run_tui_signature() -> None:
    """run_tui keeps the contract cli.tui relies on."""
    params = inspect.signature(run_tui).parameters
    assert list(params) == ["setup", "db_path", "yes"]
    assert all(p.kind is inspect.Parameter.KEYWORD_ONLY for p in params.values())


def test_app_starts_and_help(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "ok"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.pause()
            assert app.query_one("Header") is not None
            assert app.query_one("#prompt") is not None
            assert app.query_one("#status-left") is not None
            assert app.query_one("#status-right") is not None
            assert app.query_one("#spinner") is not None
            # Idle status line
            assert "就绪" in str(app.query_one("#status-left").content)
            await _submit(pilot, "/help")
            await pilot.pause()
            joined = "\n".join(app._log_texts())
            assert "帮助" in joined or "可用命令" in joined
            assert "/resume" in joined
            assert "/compact" in joined

    _run(scenario())


def test_unknown_slash_command(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "/nope")
            await pilot.pause()
            assert any("未知命令" in t for t in app._log_texts())

    _run(scenario())


# ---------------------------------------------------------------------------
# Plain input -> user message + streamed assistant reply
# ---------------------------------------------------------------------------


def test_plain_input_streams_reply(tmp_path):
    reply = "你好，这是助手的流式回复。"
    app, ws = _make_app(tmp_path, turns=[{"text": reply}])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "hi")
            await _wait_turn_done(app)
            await pilot.pause()
            texts = app._log_texts()
            assert any("❯ hi" in t for t in texts)
            assert any("助手的流式回复" in t for t in texts)
            # Turn summary line written after the turn.
            assert any("退出原因: 已完成" in t for t in texts)
            # Status bar refreshed after the turn.
            right = str(app.query_one("#status-right").content)
            assert right.startswith("轮数")
            assert "会话" in right
            # One session persisted in the store.
            assert app._runtime.session_id is not None
            assert len(app._store.list_sessions()) == 1

    _run(scenario())


# ---------------------------------------------------------------------------
# Slash commands: /clear, /sessions, /resume
# ---------------------------------------------------------------------------


def test_slash_commands_on_empty_store(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "/sessions")
            await pilot.pause()
            assert any("暂无会话记录" in t for t in app._log_texts())

            await _submit(pilot, "/resume deadbeef")
            await pilot.pause()
            assert any("未找到会话" in t for t in app._log_texts())

            await _submit(pilot, "/help")
            await pilot.pause()
            assert any("可用命令" in t for t in app._log_texts())

            # /clear empties the display without touching the store.
            await _submit(pilot, "/clear")
            await _wait_log_empty(app)
            assert len(app.query_one("#log").children) == 0

    _run(scenario())


def test_sessions_and_resume_after_turn(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "第一轮回复"}])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "hello")
            await _wait_turn_done(app)
            sid = app._runtime.session_id
            assert sid is not None

            await _submit(pilot, "/sessions")
            await pilot.pause()
            assert any(sid[:8] in t for t in app._log_texts())

            # /resume swaps in a new runtime bound to the same session.
            old_runtime = app._runtime
            await _submit(pilot, f"/resume {sid[:8]}")
            await pilot.pause()
            assert any("已恢复会话" in t for t in app._log_texts())
            assert app._runtime is not old_runtime
            assert app._runtime.session_id == sid
            assert app._runtime.rounds == 1  # restored cumulative rounds

    _run(scenario())


# ---------------------------------------------------------------------------
# Approval modal (yes=False + DefaultPolicy + apply_patch)
# ---------------------------------------------------------------------------


def test_approval_modal_allow(tmp_path):
    app, ws = _make_app(tmp_path, turns=_create_file_script(), yes=False)

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "创建 new.txt")
            modal = await _wait_for_modal(app)
            await pilot.pause()
            # The modal names the tool and the pending change.
            body = str(modal.query_one("#approval-body").content)
            assert "apply_patch" in body
            assert "new.txt" in body

            await pilot.click("#approve")
            await _wait_turn_done(app)
            await pilot.pause()

            # The file was written and the tool card finalized to ✓.
            assert (ws / "new.txt").read_text(encoding="utf-8") == "hello"
            cards = list(app.query(ToolCard))
            assert cards, "缺少工具卡片"
            assert any(
                "✓" in str(card.header_text) and "apply_patch" in str(card.header_text)
                for card in cards
            )
            assert any("退出原因: 已完成" in t for t in app._log_texts())

    _run(scenario())


def test_approval_modal_deny(tmp_path):
    app, ws = _make_app(tmp_path, turns=_create_file_script(), yes=False)

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "创建 new.txt")
            await _wait_for_modal(app)
            await pilot.click("#deny")
            await _wait_turn_done(app)
            await pilot.pause()

            # Denied: no write happened; the card shows ✗ and the model was
            # told about the denial (turn still completes).
            assert not (ws / "new.txt").exists()
            cards = list(app.query(ToolCard))
            assert cards, "缺少工具卡片"
            assert any("✗" in str(card.header_text) for card in cards)
            assert any("退出原因: 已完成" in t for t in app._log_texts())

    _run(scenario())


def test_auto_allow_skips_modal(tmp_path):
    """--yes wiring: AutoAllowPolicy + no approval handler -> no modal."""
    app, ws = _make_app(tmp_path, turns=_create_file_script(), yes=True)

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "创建 new.txt")
            await _wait_turn_done(app)
            await pilot.pause()
            assert (ws / "new.txt").read_text(encoding="utf-8") == "hello"
            assert all(
                not isinstance(screen, ApprovalModal) for screen in app.screen_stack
            )

    _run(scenario())
