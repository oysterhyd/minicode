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
from rich.markdown import Markdown
from rich.text import Text

from minicode.cli import _Setup, _build_services
from minicode.core.catalog import MODEL_CATALOG
from minicode.slash import filter_commands
from minicode.core.models import Budget, ModelResponse, TextBlock, Usage
from minicode.providers import FakeProvider, FakeProviderOptions, FakeTurn, ResponseDone, TextDelta
from minicode.security import PermissionMode
from minicode.storage import SqliteStore
from minicode.ui.app import ApprovalModal, MiniCodeApp, ToolCard, run_tui

#: Generous timeout so a busy CI machine cannot flake, small enough to fail fast.
_WAIT_TIMEOUT_S = 20.0


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_app(
    tmp_path: Path, turns: list[dict], yes: bool = False, model_label: str = "fake"
) -> tuple[MiniCodeApp, Path]:
    """Build the TUI app around a FakeProvider script and a tmp workspace."""
    ws = tmp_path / "ws"
    ws.mkdir()
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(**t) for t in turns]))
    setup = _Setup(
        workspace=ws,
        provider=provider,
        provider_name="fake",
        model_label=model_label,
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
            assert isinstance(app.query_one(".msg-assistant").content, Markdown)
            # Turn summary line written after the turn.
            assert any("退出原因: 已完成" in t for t in texts)
            # Status bar refreshed after the turn (model + tokens + session).
            right = str(app.query_one("#status-right").content)
            assert "token 120" in right
            assert "缓存" in right
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
    """--yes bypasses policy approval while retaining the handler."""
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


def test_tool_details_expand_on_click(tmp_path):
    app, ws = _make_app(tmp_path, turns=[
        {"tool_calls": [{"name": "read", "arguments": {"path": "sample.txt"}}]},
        {"text": "已读取。"},
    ], yes=True)
    (ws / "sample.txt").write_text("detail visible on demand", encoding="utf-8")

    async def scenario():
        async with app.run_test(size=(120, 32)) as pilot:
            await _submit(pilot, "读取")
            await _wait_turn_done(app)
            card = list(app.query(ToolCard))[0]
            assert not card._body.display
            assert "展开" in str(card.header_text)
            await pilot.click(card)
            await pilot.pause()
            assert card._body.display
            assert "detail visible on demand" in str(card._body.content)

    _run(scenario())


@pytest.mark.parametrize("width", [80, 120, 160])
def test_timeline_remains_usable_at_common_terminal_widths(tmp_path, width):
    app, _ws = _make_app(tmp_path, turns=[{"text": "回应"}])

    async def scenario():
        async with app.run_test(size=(width, 30)) as pilot:
            await _submit(pilot, "请求")
            await _wait_turn_done(app)
            assert any("回应" in line for line in app._log_texts())
            assert app.query_one("#prompt").size.width > 40
            assert app.query_one("#activity").size.width > 40
            assert app.has_class("narrow") is (width < 100)

    _run(scenario())


def test_scrolling_up_is_preserved_when_new_content_arrives(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[])

    async def scenario():
        async with app.run_test(size=(80, 24)) as pilot:
            for i in range(40):
                app._add_line(Text(f"line {i}"), "msg-system")
            await pilot.pause()
            log = app.query_one("#log")
            log.scroll_home(animate=False, immediate=True)
            await pilot.pause()
            assert not log.is_vertical_scroll_end
            before = log.scroll_y
            app._add_line(Text("new line"), "msg-system")
            await pilot.pause()
            assert log.scroll_y == before
            assert app.query_one("#new-content").display
            app.action_follow_log()
            await pilot.pause()
            assert log.is_vertical_scroll_end
            assert not app.query_one("#new-content").display

    _run(scenario())


def test_input_is_queued_while_a_turn_is_running(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[])
    started = asyncio.Event()
    release = asyncio.Event()

    class PausingProvider:
        name = "fake"

        def __init__(self):
            self.turns_consumed = 0

        async def stream(self, *, system, messages, tools):
            self.turns_consumed += 1
            if self.turns_consumed == 1:
                started.set()
                yield TextDelta("first ")
                await release.wait()
                yield TextDelta("reply")
                text = "first reply"
            else:
                text = "second"
                yield TextDelta(text)
            yield ResponseDone(response=ModelResponse(
                blocks=[TextBlock(text=text)], usage=Usage(available=True)
            ))

    provider = PausingProvider()
    app._setup.provider = provider
    app._runtime._provider = provider

    async def scenario():
        async with app.run_test() as pilot:
            await _submit(pilot, "one")
            await asyncio.wait_for(started.wait(), 2)
            assert app._busy
            assert "接收回复" in str(app.query_one("#activity").content)
            await _submit(pilot, "two")
            assert list(app._queued_turns) == ["two"]
            assert "排队 1" in str(app.query_one("#activity").content)
            release.set()
            deadline = time.monotonic() + _WAIT_TIMEOUT_S
            while app._busy or app._queued_turns or provider.turns_consumed < 2:
                if time.monotonic() > deadline:
                    raise AssertionError("排队回合未完成")
                await asyncio.sleep(0.02)
            assert any("second" in line for line in app._log_texts())

    _run(scenario())


@pytest.mark.parametrize("restored", [False, True])
def test_yes_then_tighten_permissions_still_shows_approval(tmp_path, restored, monkeypatch):
    app, ws = _make_app(tmp_path, turns=_create_file_script(), yes=True)
    if restored:
        sid = app._store.create_session(workspace=str(ws), provider="fake", model="fake")
        provider = app._setup.provider
        monkeypatch.setattr("minicode.cli._build_provider", lambda *args: (provider, "fake", "fake"))

    async def scenario():
        async with app.run_test() as pilot:
            if restored:
                app._cmd_resume(sid)
            app._cmd_permissions("default")
            await _submit(pilot, "create")
            await _wait_for_modal(app)
            assert not (ws / "new.txt").exists()
            await pilot.click("#approve")
            await _wait_turn_done(app)
            assert (ws / "new.txt").read_text() == "hello"

    try:
        _run(scenario())
    finally:
        app._store.close()


def test_resume_other_workspace_restores_all_services_and_model(tmp_path, monkeypatch):
    from minicode.goals import AcceptanceSpec, GoalChecker, ProtectedSnapshot
    from minicode.runtime import AgentRuntime
    from minicode.security import AutoAllowPolicy

    app, old_ws = _make_app(tmp_path, turns=[{"text": "old"}], yes=True)
    target = tmp_path / "target"
    target.mkdir()
    (target / "guard.txt").write_text("original")
    spec = AcceptanceSpec(items=[{"id": "protected", "type": "protected", "path": "guard.txt"}])
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="ok")]))
    provider.name, provider.model = "anthropic", "saved-model"
    previous = AgentRuntime(
        provider=provider, provider_name="anthropic", model="saved-model",
        workspace=target, store=app._store, registry=app._services.registry,
        policy=AutoAllowPolicy(), goal_checker=GoalChecker(spec, target, ProtectedSnapshot(target, ["guard.txt"])),
    )
    sid = asyncio.run(previous.run_turn("start")).session_id
    (target / "guard.txt").write_text("modified")
    monkeypatch.setattr("minicode.cli._build_provider", lambda choice, model, script: (provider, choice.value, model))

    async def scenario():
        async with app.run_test():
            app._cmd_resume(sid)
            runtime = app._runtime
            assert app._setup.workspace == runtime.workspace == target
            assert app._services.goal_checker.workspace == target
            assert runtime._goal_checker.protected_snapshot.workspace == target
            assert runtime._goal_checker.protected_snapshot.violations()
            assert runtime.model == app._setup.model_label == "saved-model"
            assert runtime.provider.name == runtime.provider_name == app._setup.provider_name == "anthropic"
            assert str(target) in app.sub_title and str(old_ws) not in runtime._system_prompt

    try:
        _run(scenario())
    finally:
        app._store.close()


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
            assert len(app._root_items) == len(filter_commands("/"))

            await pilot.press("h")
            await pilot.pause()
            assert [c.name for c in app._root_items] == ["/help"]
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
            assert app._slash_index == (first + 1) % len(app._root_items)

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
            index = [c.name for c in app._root_items].index("/clear")
            app._slash_index = index
            app._menu().show_root(app._root_items, index)
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


# ---------------------------------------------------------------------------
# Cascading two-level submenu (/model, /effort)
# ---------------------------------------------------------------------------


def _gateway_env(monkeypatch):
    """Hermetic gateway credentials so /model can build a real provider."""
    monkeypatch.setenv("COMMANDCODE_API_KEY", "test-key")
    monkeypatch.setenv("COMMANDCODE_BASE_URL", "https://gw.test/provider/v1")


def test_model_submenu_enter_selects_and_applies(tmp_path, monkeypatch):
    _gateway_env(monkeypatch)
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            # Typing the full command drops straight into the submenu.
            await pilot.press(*"/model")
            await pilot.pause()
            assert app.slash_view == "submenu"
            assert [e.value for e in app._submenu_entries] == list(MODEL_CATALOG)
            # No model is active yet (fake) -> highlight the first entry.
            assert app._slash_index == 0

            # Enter applies the highlighted model immediately.
            await pilot.press("enter")
            await pilot.pause()
            assert app._runtime.model == "deepseek/deepseek-v4.1-flash"
            assert app.slash_view is None  # menu closed
            assert app.query_one("#prompt").text == ""  # input idle again
            texts = app._log_texts()
            assert any("✔ 已切换模型" in line and "deepseek/deepseek-v4.1-flash" in line
                       for line in texts)

    _run(scenario())


def test_model_submenu_navigation_marks_current(tmp_path, monkeypatch):
    _gateway_env(monkeypatch)
    app, _ws = _make_app(
        tmp_path, turns=[{"text": "好"}], model_label="z.ai/glm-5.3-flash"
    )

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/model")
            await pilot.pause()
            # The active model is highlighted and badged in the submenu.
            current_index = [e.value for e in app._submenu_entries].index("z.ai/glm-5.3-flash")
            assert app._slash_index == current_index
            entry = app._submenu_entries[current_index]
            assert "当前" in entry.badges and "上下文 1M" in entry.detail

            # Navigate to a neighbour (up wraps to the first entry) and apply.
            await pilot.press("up")
            await pilot.pause()
            await pilot.press("enter")
            await pilot.pause()
            expected = list(MODEL_CATALOG)[(current_index - 1) % len(MODEL_CATALOG)]
            assert app._runtime.model == expected

    _run(scenario())


def test_model_submenu_esc_returns_to_root_then_closes(tmp_path, monkeypatch):
    _gateway_env(monkeypatch)
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/model")
            await pilot.pause()
            assert app.slash_view == "submenu"

            # Esc: back to the root command list, input untouched.
            await pilot.press("escape")
            await pilot.pause()
            assert app.slash_view == "root"
            assert [c.name for c in app._root_items] == ["/model"]
            assert app.query_one("#prompt").text == "/model"

            # Esc again: the floating menu closes completely.
            await pilot.press("escape")
            await pilot.pause()
            assert app.slash_view is None
            # Nothing was executed and the model is unchanged.
            assert app._runtime.model == "fake"

    _run(scenario())


def test_model_submenu_backspace_returns_to_root(tmp_path, monkeypatch):
    _gateway_env(monkeypatch)
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/model")
            await pilot.pause()
            assert app.slash_view == "submenu"

            await pilot.press("backspace")
            await pilot.pause()
            # Back to the root list; the typed command is kept intact.
            assert app.slash_view == "root"
            assert app.query_one("#prompt").text == "/model"

            # Typing again re-enters the submenu seamlessly.
            await pilot.press("backspace")
            await pilot.pause()
            await pilot.press("l")
            await pilot.pause()
            assert app.slash_view == "submenu"

    _run(scenario())


def test_model_submenu_tab_fills_value_for_direct_execution(tmp_path, monkeypatch):
    _gateway_env(monkeypatch)
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/model")
            await pilot.pause()
            value = app._submenu_entries[app._slash_index].value

            await pilot.press("tab")
            await pilot.pause()
            # Tab fills "/model <value>" and closes the menu (direct-exec form).
            assert app.slash_view is None
            assert app.query_one("#prompt").text == f"/model {value} "

            # Enter now bypasses the menu and executes directly.
            await pilot.press("enter")
            await pilot.pause()
            assert app._runtime.model == value

    _run(scenario())


def test_effort_submenu_six_levels_and_selection(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])
    # The fake provider carries the attribute once the feature is in play.
    app._setup.provider.reasoning_effort = None

    async def scenario():
        async with app.run_test() as pilot:
            # Enter on the highlighted /effort command opens the submenu.
            await pilot.press(*"/effort")
            await pilot.pause()
            assert app.slash_view == "submenu"
            assert [e.value for e in app._submenu_entries] == [
                "off", "low", "medium", "high", "xhigh", "max",
            ]
            assert app._slash_index == 0  # nothing set -> "off" highlighted

            await pilot.press(*["down"] * 5)  # highlight "max"
            await pilot.pause()
            await pilot.press("enter")
            await pilot.pause()
            assert app._setup.provider.reasoning_effort == "max"
            assert any("✔ 推理预算已调整为 max" in t for t in app._log_texts())

    _run(scenario())


def test_effort_submenu_esc_back_keeps_provider_unchanged(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])
    app._setup.provider.reasoning_effort = None

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/effort")
            await pilot.pause()
            await pilot.press("escape")
            await pilot.pause()
            assert app.slash_view == "root"
            assert app._setup.provider.reasoning_effort is None

    _run(scenario())


def test_direct_argument_bypass_skips_menu(tmp_path):
    """Full "/effort high" with an argument executes directly, menu closed."""
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])
    app._setup.provider.reasoning_effort = None

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/effort high")
            await pilot.pause()
            assert app.slash_view is None  # argument typed -> no menu
            await pilot.press("enter")
            await pilot.pause()
            assert app._setup.provider.reasoning_effort == "high"

    _run(scenario())


# ---------------------------------------------------------------------------
# Cascading submenu for every command that takes an enumerated value
# ---------------------------------------------------------------------------


def test_permissions_submenu_lists_modes_and_applies(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/permissions")
            await pilot.pause()
            assert app.slash_view == "submenu"
            assert [e.value for e in app._submenu_entries] == [
                mode.value for mode in PermissionMode
            ]
            assert app._submenu_entries[0].badges == ("当前",)

            # Descend one option and confirm it with Enter.
            await pilot.press("down")
            await pilot.pause()
            await pilot.press("enter")
            await pilot.pause()
            assert app._services.policy.mode is PermissionMode.ACCEPT_EDITS
            assert app.slash_view is None
            assert any("accept_edits" in line for line in app._log_texts())

    _run(scenario())


def test_permissions_submenu_esc_keeps_mode_unchanged(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/permissions")
            await pilot.pause()
            await pilot.press("escape")  # submenu -> root list
            await pilot.pause()
            assert app.slash_view == "root"
            assert [c.name for c in app._root_items] == ["/permissions"]
            await pilot.press("escape")  # root -> closed
            await pilot.pause()
            assert app.slash_view is None
            assert app._services.policy.mode is PermissionMode.DEFAULT

    _run(scenario())


def test_resume_submenu_lists_sessions_and_applies(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            # Produce one stored session first.
            app.query_one("#prompt").text = "写点什么"
            await pilot.press("enter")
            await _wait_turn_done(app)
            session_id = app._runtime.session_id
            assert session_id is not None

            await pilot.press(*"/resume")
            await pilot.pause()
            assert app.slash_view == "submenu"
            entries = app._submenu_entries
            assert [e.value for e in entries] == [session_id[:8]]
            # The live session is marked, and Enter resumes the picked one.
            assert entries[0].badges == ("当前",)

            await pilot.press("enter")
            await pilot.pause()
            assert app.slash_view is None
            assert app._runtime.session_id == session_id
            assert any(
                line.startswith(f"已恢复会话 {session_id[:8]}")
                for line in app._log_texts()
            )

    _run(scenario())


def test_resume_submenu_closes_when_no_sessions(tmp_path):
    """No candidates -> no submenu; Enter still runs the command directly."""
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/resume")
            await pilot.pause()
            assert app.slash_view is None  # nothing to pick from
            await pilot.press("enter")
            await pilot.pause()
            assert any("用法: /resume" in line for line in app._log_texts())

    _run(scenario())


def test_tab_in_permissions_submenu_fills_direct_command(tmp_path):
    app, _ws = _make_app(tmp_path, turns=[{"text": "好"}])

    async def scenario():
        async with app.run_test() as pilot:
            await pilot.press(*"/permissions")
            await pilot.pause()
            await pilot.press("down")  # accept_edits
            await pilot.pause()
            await pilot.press("tab")
            await pilot.pause()
            assert app.query_one("#prompt").text == "/permissions accept_edits "
            assert app.slash_view is None  # menu closed, Enter runs directly

    _run(scenario())
