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
from minicode.slash import filter_commands
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


def _create_file_script() -> list[dict]:
    """write creates new.txt, then the model gives a final answer."""
    return [
        {
            "tool_calls": [
                {
                    "name": "write",
                    "arguments": {"path": "new.txt", "content": "hello"},
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
            # Status bar refreshed after the turn (model + tokens + session).
            right = str(app.query_one("#status-right").content)
            assert "输入 100" in right
            assert "输出 20" in right
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

            # /clear empties the display and leaves only the semantics note;
            # the store is untouched.
            await _submit(pilot, "/clear")
            await pilot.pause()
            await pilot.pause()
            assert len(app.query_one("#log").children) == 1
            assert "已清屏" in app._log_texts()[0]
            assert len(app._store.list_sessions()) == 0

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
# Approval modal (yes=False + DefaultPolicy + edit)
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
            assert "write" in body
            assert "new.txt" in body

            await pilot.click("#approve")
            await _wait_turn_done(app)
            await pilot.pause()

            # The file was written and the tool card finalized to ✓.
            assert (ws / "new.txt").read_text(encoding="utf-8") == "hello"
            cards = list(app.query(ToolCard))
            assert cards, "缺少工具卡片"
            assert any(
                "✓" in str(card.header_text) and "write" in str(card.header_text)
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


# ---------------------------------------------------------------------------
# Slash autocomplete menu (Tab / arrows / Esc / Enter)
# ---------------------------------------------------------------------------


def test_slash_menu_opens_filters_and_completes(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press("/")
            await pilot.pause()
            assert app.slash_visible
            assert len(app._slash_items) == len(filter_commands("/"))

            await pilot.press("h")
            await pilot.pause()
            assert [c.name for c in app._slash_items] == ["/help"]
            assert app.query_one("#slash-menu").has_class("slash-visible")

            # Tab completes the highlighted entry into the prompt.
            await pilot.press("tab")
            await pilot.pause()
            assert app.query_one("#prompt").text == "/help "
            assert not app.slash_visible

            # Enter submits; the help block is rendered into the log.
            await pilot.press("enter")
            await pilot.pause()
            texts = app._log_texts()
            assert any("可用命令" in line for line in texts)

    _run(scenario())


def test_slash_menu_navigation_and_escape(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press("/")
            await pilot.pause()
            first = app._slash_index

            await pilot.press("down")
            await pilot.pause()
            assert app._slash_index == (first + 1) % len(app._slash_items)

            await pilot.press("up")
            await pilot.pause()
            assert app._slash_index == first

            await pilot.press("escape")
            await pilot.pause()
            assert not app.slash_visible
            assert not app.query_one("#slash-menu").has_class("slash-visible")

            # Esc on a non-slash prompt is a no-op (no crash, menu stays shut).
            await pilot.press("x")
            await pilot.pause()
            assert not app.slash_visible

    _run(scenario())


def test_slash_menu_enter_confirms_highlighted_command(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press("/")
            await pilot.pause()
            # Move the highlight onto /clear, then confirm with Enter.
            index = [c.name for c in app._slash_items].index("/clear")
            app._slash_index = index
            app._menu().show_items(app._slash_items, index)
            await pilot.pause()
            await pilot.press("enter")
            await pilot.pause()
            texts = app._log_texts()
            assert any("已清屏" in line for line in texts)

    _run(scenario())


# ---------------------------------------------------------------------------
# /model, /effort, /permissions, /new and the enhanced status bar
# ---------------------------------------------------------------------------


def test_permissions_command_switches_mode_and_status_bar(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "/permissions accept_edits")
            await pilot.pause()
            texts = app._log_texts()
            assert any("权限模式已切换 default → accept_edits" in line for line in texts)
            # The status bar mirrors the active mode in real time.
            left = str(app.query_one("#status-left").content)
            assert "权限:自动编辑" in left

    _run(scenario())


def test_new_command_resets_runtime_and_log(tmp_path):
    turns = [{"text": "旧回复"}, {"text": "新回复"}]
    app, _ws = _make_app(tmp_path, turns=turns)

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "第一句")
            await _wait_turn_done(app)
            old_runtime = app._runtime
            assert old_runtime.session_id is not None

            await _submit(pilot, "/new")
            await pilot.pause()
            assert app._runtime is not old_runtime
            assert app._runtime.session_id is None  # brand-new session
            assert not list(app.query_one("#log").children) or any(
                "已重置对话上下文" in t for t in app._log_texts()
            )

            await _submit(pilot, "第二句")
            await _wait_turn_done(app)
            # Two distinct sessions were persisted.
            assert len(app._store.list_sessions()) == 2

    _run(scenario())


def test_effort_command_reports_unsupported_provider(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "/effort high")
            await pilot.pause()
            assert any("不支持推理预算调整" in t for t in app._log_texts())

    _run(scenario())


def test_banner_and_status_bar_content(tmp_path):
    app, ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.pause()
            texts = app._log_texts()
            # The banner's info line carries version, platform and model.
            assert any("minicode v" in line and "Python" in line for line in texts)
            # Status bar: workspace, permission mode, model, context window.
            left = str(app.query_one("#status-left").content)
            assert ws.name in left
            assert "权限:默认" in left
            right = str(app.query_one("#status-right").content)
            assert "fake" in right
            assert "ctx" in right and "200k" in right
            assert "缓存" in right

    _run(scenario())
