"""Full-screen Textual TUI for minicode (Claude Code style).

The TUI is a thin presentation layer over the same backend pieces the CLI
uses: :func:`minicode.cli._build_services` assembles registry / policy /
P1 services, :class:`minicode.runtime.AgentRuntime` runs the turns, and this
module only adds widgets, the approval modal, slash commands and the status
bar. The runtime runs inside a Textual worker on the app's own event loop,
so the approval adapter can park on an ``asyncio.Future`` and let a modal
screen resolve it — no threads, no ``call_from_thread``.

User-facing strings are Chinese, code is English (project convention).
"""

from __future__ import annotations

import asyncio
import time
from typer import Exit
from pathlib import Path
from typing import Any

from rich.console import Console
from rich.text import Text
from textual import events, on
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.message import Message
from textual.screen import ModalScreen
from textual.widget import Widget
from textual.widgets import Button, Header, LoadingIndicator, Static, TextArea

from minicode.banner import banner_text
from minicode.cli import (
    _Setup,
    _attach_compactor,
    _build_services,
    _exit_label,
    _format_timestamp,
    _provider_for_model,
    _prepare_resume,
    _resolve_session_id,
    _status_label,
    _success_brief,
    _tool_args_summary,
)
from minicode.core.catalog import (
    DEFAULT_MODEL,
    EFFORT_LEVELS,
    MODEL_CATALOG,
    parse_effort,
)
from minicode.core.format import format_tokens
from minicode.core.models import (
    ApprovalDecision,
    ApprovalRequest,
    Event,
    EventType,
    RunResult,
)
from minicode.providers import ProviderRequestError
from minicode.runtime import AgentRuntime
from minicode.security import (
    PERMISSION_MODE_LABELS,
    ModePolicy,
    PermissionMode,
    parse_permission_mode,
    permission_mode_lines,
)
from minicode.slash import (
    SUBMENU_COMMANDS,
    SlashCommand,
    SubmenuItem,
    filter_commands,
    format_command_lines,
)

#: Seconds between two idle Ctrl+C presses that quits the app.
_DOUBLE_CTRL_C_WINDOW_S = 2.0

#: How many lines/chars of a tool output preview are shown in a tool card.
_PREVIEW_MAX_LINES = 6
_PREVIEW_MAX_CHARS_PER_LINE = 160

#: Maximum slash-command candidates rendered in the floating menu at once.
_SLASH_MENU_MAX_ITEMS = 8

#: Short permission-mode labels for the status bar.
_MODE_SHORT: dict[PermissionMode, str] = {
    PermissionMode.DEFAULT: "默认",
    PermissionMode.ACCEPT_EDITS: "自动编辑",
    PermissionMode.BYPASS: "全部允许",
}


def _help_text() -> Text:
    """Help block assembled from the shared slash-command registry."""
    lines = ["可用命令：", *format_command_lines()]
    lines.append("快捷键：Enter 提交 · Shift+Enter 换行 · Ctrl+C 取消回合/再按一次退出 · Ctrl+L 清屏")
    return Text("\n".join(lines))


# ---------------------------------------------------------------------------
# Input widgets and the slash-command menu
# ---------------------------------------------------------------------------


def _set_text_caret_end(area: "TextArea", value: str) -> None:
    """Replace *area*'s content with *value* and park the cursor at the end."""
    area.load_text(value)
    area.move_cursor(area.document.end)


class SlashMenu(Static):
    """Floating menu above the prompt for slash commands.

    Two cascading views rendered in place: the root command list and the
    second-level option submenu (``命令 › /model``). Hidden (``display:
    none``) unless the ``slash-visible`` class is set; content is a plain
    :class:`~rich.text.Text` so highlight styling never parses user input
    as markup.
    """

    def show_root(self, items: list[SlashCommand], index: int) -> None:
        """Render the root command list with the entry at *index* highlighted."""
        text = Text()
        start = self._window_start(len(items), index)
        for offset, cmd in enumerate(items[start : start + _SLASH_MENU_MAX_ITEMS]):
            selected = start + offset == index
            marker = "❯ " if selected else "  "
            text.append(marker + cmd.name, style="bold reverse cyan" if selected else "cyan")
            text.append(f"  {cmd.summary}\n", style="dim")
        text.append("Tab 补全 · ↑↓ 选择 · Enter 确认 · Esc 关闭", style="dim italic")
        self.update(text)
        self.add_class("slash-visible")

    def show_submenu(self, crumb: str, items: list[SubmenuItem], index: int) -> None:
        """Render the second-level option menu under a breadcrumb header."""
        text = Text()
        text.append(crumb + "\n", style="bold")
        start = self._window_start(len(items), index)
        for offset, item in enumerate(items[start : start + _SLASH_MENU_MAX_ITEMS]):
            selected = start + offset == index
            marker = "❯ " if selected else "  "
            text.append(marker + item.value, style="bold reverse cyan" if selected else "cyan")
            if item.detail:
                text.append(f"  ({item.detail})", style="dim")
            for badge in item.badges:
                text.append(f"  [{badge}]", style="green")
            text.append("\n")
        text.append("Enter 确认 · Tab 填入参数 · Esc 返回上级", style="dim italic")
        self.update(text)
        self.add_class("slash-visible")

    @staticmethod
    def _window_start(count: int, index: int) -> int:
        """First visible entry so the highlighted one stays inside the cap."""
        if count <= _SLASH_MENU_MAX_ITEMS:
            return 0
        return max(0, min(index - _SLASH_MENU_MAX_ITEMS // 2, count - _SLASH_MENU_MAX_ITEMS))

    def hide(self) -> None:
        self.remove_class("slash-visible")


class PromptArea(TextArea):
    """Multi-line prompt: Enter submits, Shift+Enter / Alt+Enter newline.

    While the app's slash menu is visible, ``Tab`` completes the highlighted
    candidate, ``up``/``down`` move the selection, ``Enter`` confirms it —
    descending into the cascading submenu for ``/model`` / ``/effort``, or
    completing + executing plain commands — and ``Esc`` steps back one menu
    level (submenu -> root -> closed). In the submenu, ``Backspace`` also
    steps back to the root list instead of deleting text. Every other key
    falls through to the TextArea and then refreshes the menu.
    """

    class Submitted(Message):
        """Posted when the user presses Enter to submit the prompt."""

        def __init__(self, text: str, prompt_area: "PromptArea") -> None:
            super().__init__()
            self.text = text
            self.prompt_area = prompt_area

    async def _on_key(self, event: events.Key) -> None:
        """Intercept submit / autocomplete keys before TextArea handles them."""
        app = self.app
        menu_visible = getattr(app, "slash_visible", False)
        if event.key == "enter":
            event.stop()
            event.prevent_default()
            if menu_visible:
                app.slash_confirm()
            else:
                self.post_message(self.Submitted(self.text, self))
            return
        if event.key in ("shift+enter", "alt+enter"):
            event.stop()
            event.prevent_default()
            self.insert("\n")
            return
        if menu_visible:
            if event.key == "tab":
                event.stop()
                event.prevent_default()
                app.slash_complete()
                return
            if event.key == "down":
                event.stop()
                event.prevent_default()
                app.slash_move(1)
                return
            if event.key == "up":
                event.stop()
                event.prevent_default()
                app.slash_move(-1)
                return
            if event.key == "escape":
                event.stop()
                event.prevent_default()
                app.slash_back()
                return
            if event.key == "backspace":
                # Menu visible: Backspace steps back a level instead of
                # editing text. In the submenu it returns to the root list;
                # in the root list it deletes one char (synchronously, since
                # the TextArea's binding-driven delete lands after this
                # handler returns and the menu must re-derive on fresh text).
                event.stop()
                event.prevent_default()
                if app.slash_view == "submenu":
                    app.slash_back()
                else:
                    self.action_delete_left()
                    app.refresh_slash_menu()
                return
        await super()._on_key(event)
        # The text may have changed: re-filter the candidate list.
        app.refresh_slash_menu()

    async def _on_paste(self, event: events.Paste) -> None:
        """Pasted text bypasses key events; keep the candidate list in sync."""
        await super()._on_paste(event)
        self.app.refresh_slash_menu()


class StreamedReply(Static):
    """Assistant bubble that grows as ``TextDelta`` chunks arrive."""

    def __init__(self) -> None:
        super().__init__(Text(""), classes="msg-assistant")
        self._buffer = ""

    def append(self, delta: str) -> None:
        self._buffer += delta
        # Text (not str) so model output is never parsed as rich markup.
        self.update(Text(self._buffer))


def _preview_text(preview: str) -> Text:
    """Trim a tool output preview to a few dim lines for the card body."""
    lines = preview.splitlines()
    shown = lines[:_PREVIEW_MAX_LINES]
    if len(lines) > _PREVIEW_MAX_LINES:
        shown.append(f"…（共 {len(lines)} 行）")
    trimmed = [
        line[:_PREVIEW_MAX_CHARS_PER_LINE] + ("…" if len(line) > _PREVIEW_MAX_CHARS_PER_LINE else "")
        for line in shown
    ]
    return Text("\n".join(trimmed), style="dim")


class ToolCard(Vertical):
    """One tool call: a start line that is later finalized to ✓/✗ plus an
    output preview (details-style body, shown only when there is output)."""

    def __init__(self, call_id: str, name: str, summary: str) -> None:
        super().__init__(classes="tool-card")
        self.call_id = call_id
        line = Text("  ▸ ", style="dim")
        line.append(name, style="bold cyan")
        if summary:
            line.append(f" {summary}")
        self.header_text = line
        self._header = Static(line)
        self._body = Static(Text(""), classes="tool-body")
        self._body.display = False

    def compose(self) -> ComposeResult:
        yield self._header
        yield self._body

    def set_result(self, data: dict[str, Any]) -> None:
        """Finalize the card from a ``TOOL_CALL_RESULT`` event payload."""
        name = str(data.get("name", "?"))
        line = Text("  ")
        if data.get("success"):
            line.append("✓ ", style="green")
            line.append(name, style="green")
            line.append(f" {_success_brief(name, data)}")
        else:
            error = str(data.get("error") or "执行失败")
            line.append("✗ ", style="red")
            line.append(name, style="red")
            line.append(f" {error}", style="red")
        self.header_text = line
        self._header.update(line)
        preview = str(data.get("output_preview") or "")
        if preview.strip():
            self._body.update(_preview_text(preview))
            self._body.display = True


# ---------------------------------------------------------------------------
# Approval modal
# ---------------------------------------------------------------------------


class ApprovalModal(ModalScreen[ApprovalDecision | None]):
    """Blocking approval dialog; resolves the app's waiting future via
    ``dismiss`` (Escape dismisses with ``None`` which counts as a denial)."""

    BINDINGS = [Binding("escape", "dismiss_dialog", "取消")]

    def __init__(self, request: ApprovalRequest) -> None:
        super().__init__()
        self._request = request

    def compose(self) -> ComposeResult:
        body = Text()
        body.append(self._request.tool_name, style="bold")
        body.append(f"\n{self._request.summary}")
        with Vertical(id="approval-dialog"):
            yield Static("需要审批", classes="approval-title")
            yield Static(body, id="approval-body")
            with Horizontal(id="approval-buttons"):
                yield Button("拒绝", variant="error", id="deny")
                yield Button("允许", variant="success", id="approve")

    def action_dismiss_dialog(self) -> None:
        self.dismiss(None)

    @on(Button.Pressed, "#approve")
    def _approve(self) -> None:
        self.dismiss(ApprovalDecision(granted=True))

    @on(Button.Pressed, "#deny")
    def _deny(self) -> None:
        self.dismiss(ApprovalDecision(granted=False))


class ModelPickerModal(ModalScreen[str | None]):
    """Interactive model selection (``/model`` with no argument).

    One button per catalog model, the active one marked; ``dismiss``es with
    the picked model name, or ``None`` on Escape.
    """

    BINDINGS = [Binding("escape", "dismiss_dialog", "取消")]

    def __init__(self, current: str) -> None:
        super().__init__()
        self._current = current
        self._names: list[str] = list(MODEL_CATALOG)

    def compose(self) -> ComposeResult:
        body = Text()
        for name in self._names:
            info = MODEL_CATALOG[name]
            mark = " ← 当前" if name == self._current else ""
            body.append(
                f"{name}（上下文 {format_tokens(info.context_window)} token）{mark}\n",
                style="bold" if name == self._current else "",
            )
        with Vertical(id="model-dialog"):
            yield Static("选择模型", classes="approval-title")
            yield Static(body, id="model-body")
            with Horizontal(id="model-buttons"):
                for index, name in enumerate(self._names):
                    yield Button(name, id=f"model-{index}")

    def action_dismiss_dialog(self) -> None:
        self.dismiss(None)

    @on(Button.Pressed, "#model-buttons Button")
    def _pick(self, event: Button.Pressed) -> None:
        assert event.button.id is not None
        index = int(event.button.id.rsplit("-", 1)[1])
        self.dismiss(self._names[index])


# ---------------------------------------------------------------------------
# Presentation helpers (mirrors of the CLI one-liners, adapted to Text)
# ---------------------------------------------------------------------------


def _p1_line(event: Event) -> Text:
    """One-line presentation for the P1 events worth surfacing live."""
    data = event.data
    if event.type is EventType.GOAL_CHECK:
        passed = bool(data.get("passed"))
        line = Text("  ◆ 验收检查：", style="dim")
        line.append("通过" if passed else "未通过", style="green" if passed else "yellow")
        for item in data.get("items", []):
            ok = bool(item.get("passed"))
            line.append(
                f"\n    {'✓' if ok else '✗'} {item.get('item_id')}",
                style="green" if ok else "red",
            )
        return line
    if event.type is EventType.CONTEXT_COMPACTED:
        return Text(
            f"  ◆ 上下文已压缩：估算 {format_tokens(int(data.get('tokens_before') or 0))} → "
            f"{format_tokens(int(data.get('tokens_after') or 0))} token",
            style="dim",
        )
    if event.type is EventType.BACKGROUND_JOB_COMPLETED:
        ok = data.get("status") == "completed"
        return Text(
            f"  ◆ 后台任务 {data.get('job_id')} 退出码 {data.get('exit_code')}",
            style="green" if ok else "red",
        )
    if event.type is EventType.BACKGROUND_JOB_LOST:
        return Text(
            f"  ◆ 后台任务 {data.get('job_id')} 已终止或失联，结果不完整", style="yellow"
        )
    if event.type is EventType.SIDE_EFFECT_UNKNOWN:
        return Text(
            f"  ◆ {data.get('name')} 的副作用状态未知，需要先核实", style="yellow"
        )
    raise AssertionError(f"unhandled P1 event: {event.type}")


def _turn_summary_text(result: RunResult) -> Text:
    """The dim summary line written after each finished turn."""
    usage = result.total_usage
    line = Text(style="dim")
    line.append("── ")
    line.append(
        f"退出原因: {_exit_label(result.exit_reason.value)}"
        f" · 轮数: {result.rounds}"
        f" · Token: {format_tokens(usage.total_tokens)}"
        f"（输入 {format_tokens(usage.input_tokens)} / 输出 {format_tokens(usage.output_tokens)}）"
        f" · 耗时: {result.duration_s:.1f}s"
        f" · 会话: {result.session_id[:8]}"
    )
    return line


# ---------------------------------------------------------------------------
# The app
# ---------------------------------------------------------------------------


class MiniCodeApp(App[None]):
    """minicode 全屏交互界面：流式回复、工具卡片、审批弹窗、斜杠命令。"""

    TITLE = "minicode"

    BINDINGS = [
        # priority: win over the focused TextArea's own ctrl+c (copy) binding.
        Binding("ctrl+c", "cancel_or_exit", "取消/退出", priority=True),
        Binding("ctrl+q", "quit", "退出", priority=True),
        Binding("ctrl+l", "clear_log", "清屏"),
    ]

    CSS = """
    Screen { layout: vertical; }
    #log { height: 1fr; padding: 1 1 0 1; }
    .msg-user { margin-top: 1; }
    .msg-assistant { margin-top: 1; }
    .msg-banner { margin-bottom: 1; }
    .tool-card { height: auto; margin-top: 1; }
    .tool-card > Static { height: auto; }
    .tool-body { color: $text-muted; height: auto; }
    .msg-event { color: $text-muted; margin-top: 1; }
    .msg-system { margin-top: 1; }
    .msg-warn { color: $warning; margin-top: 1; }
    .msg-summary { color: $text-muted; margin-top: 1; }
    #input-dock { height: auto; padding: 0 1 1 1; }
    #spinner { display: none; height: 1; }
    #slash-menu {
        display: none; height: auto; max-height: 12;
        border: round $primary; background: $surface; padding: 0 1;
        margin-bottom: 1;
    }
    #slash-menu.slash-visible { display: block; }
    #prompt { height: 4; border: round $primary; margin-bottom: 1; }
    #prompt:focus { border: round $accent; }
    #statusbar { height: 1; }
    #status-left { width: 1fr; color: $text-muted; }
    #status-right { width: auto; color: $text-muted; }
    ApprovalModal { align: center middle; }
    ModelPickerModal { align: center middle; }
    #approval-dialog {
        width: 64; height: auto;
        border: round $warning; background: $surface; padding: 1 2;
    }
    .approval-title { text-style: bold; color: $warning; margin-bottom: 1; }
    #approval-body { margin-bottom: 1; }
    #approval-buttons { height: auto; align-horizontal: right; }
    #approval-buttons Button { margin-left: 1; }
    #model-dialog {
        width: auto; max-width: 90%; height: auto;
        border: round $accent; background: $surface; padding: 1 2;
    }
    #model-body { margin-bottom: 1; }
    #model-buttons { height: auto; }
    #model-buttons Button { margin-right: 1; }
    """

    def __init__(
        self,
        *,
        setup: _Setup,
        store: Any,
        services: Any,
        yes: bool,
    ) -> None:
        """Build the app around an already-assembled services bundle.

        ``services`` comes from :func:`minicode.cli._build_services`; the TUI
        keeps its registry / policy / P1 services but installs its own
        approval adapter and streaming callbacks. Tests use this constructor
        to inject a FakeProvider-backed setup instead of calling ``run_tui``.
        """
        super().__init__()
        self._setup = setup
        self._store = store
        self._services = services
        self._yes = yes
        self._busy = False
        self._turn_worker: Any = None
        self._approval_future: asyncio.Future[ApprovalDecision] | None = None
        self._last_idle_ctrl_c = 0.0
        self._current_reply: StreamedReply | None = None
        self._cards: dict[str, ToolCard] = {}
        # Slash-menu state machine: ``root`` command list or a cascading
        # ``submenu`` (命令 › /model). The view is re-derived from the prompt
        # text on every change; ``_backed_out`` keeps Esc'd submenus closed
        # until the text changes again.
        self._root_items: list[SlashCommand] = []
        self._submenu_entries: list[SubmenuItem] = []
        self._submenu_cmd: str | None = None  # None = root view / closed
        self._menu_open = False
        self._backed_out = False
        self._slash_index = 0
        self._slash_prefix = ""
        self._build_runtime()

    # -- runtime wiring ------------------------------------------------------

    def _build_runtime(self) -> None:
        """Assemble a fresh AgentRuntime (or replace it on /resume)."""
        services = self._services
        runtime = AgentRuntime(
            provider=self._setup.provider,
            registry=services.registry,
            store=self._store,
            policy=services.policy,
            workspace=self._setup.workspace,
            provider_name=self._setup.provider_name,
            model=self._setup.model_label,
            budget=self._setup.budget,
            approval_handler=self._approval_handler,
            on_text_delta=self._on_text_delta,
            on_event=self._on_event,
            background_manager=services.background_manager,
            artifact_store=services.artifact_store,
            goal_checker=services.goal_checker,
            evidence_ledger=services.evidence_ledger,
        )
        _attach_compactor(runtime, services.artifact_store)
        self._runtime = runtime

    # -- layout ---------------------------------------------------------------

    def compose(self) -> ComposeResult:
        yield Header()
        with VerticalScroll(id="log"):
            pass
        with Vertical(id="input-dock"):
            yield LoadingIndicator(id="spinner")
            yield SlashMenu("", id="slash-menu")
            yield PromptArea(
                placeholder="输入任务或问题，/ 触发命令补全，/help 查看命令…",
                id="prompt",
            )
            with Horizontal(id="statusbar"):
                yield Static("", id="status-left")
                yield Static("", id="status-right")

    def on_mount(self) -> None:
        self.sub_title = (
            f"{self._setup.workspace} · {self._setup.provider_name}/"
            f"{self._setup.model_label}"
        )
        log = self.query_one("#log", VerticalScroll)
        log.mount(
            Static(
                banner_text(
                    provider_label=self._setup.provider_name,
                    model_label=self._setup.model_label,
                    workspace=str(self._setup.workspace.resolve()),
                ),
                classes="msg-banner",
            )
        )
        log.anchor()
        self._refresh_status("就绪")
        self.query_one("#prompt", PromptArea).focus()

    # -- log helpers ----------------------------------------------------------

    def _mount(self, widget: Widget) -> None:
        log = self.query_one("#log", VerticalScroll)
        log.mount(widget)
        log.scroll_end(force=True, animate=False)

    def _add_line(self, content: Text, cls: str) -> Static:
        widget = Static(content, classes=cls)
        self._mount(widget)
        return widget

    def _clear_log(self) -> None:
        self._end_streaming()
        self._cards.clear()
        log = self.query_one("#log", VerticalScroll)
        for child in list(log.children):
            child.remove()

    def _log_texts(self) -> list[str]:
        """Plain-text snapshot of the log (used by tests and debugging)."""
        texts: list[str] = []
        for child in self.query_one("#log", VerticalScroll).children:
            if isinstance(child, ToolCard):
                texts.append(str(child.header_text))
            elif isinstance(child, Static):
                texts.append(str(child.content))
        return texts

    # -- status bar -----------------------------------------------------------

    @staticmethod
    def _short_path(path: Path) -> str:
        """Home-abbreviated path for the status bar."""
        resolved = path.resolve()
        home = Path.home()
        try:
            return "~/" + resolved.relative_to(home).as_posix()
        except ValueError:
            return str(resolved)

    def _permission_mode(self) -> PermissionMode | None:
        """Active permission mode (None for policies without modes)."""
        policy = self._services.policy
        return policy.mode if isinstance(policy, ModePolicy) else None

    @staticmethod
    def _fmt_tokens(value: int) -> str:
        """Compact token count: 1M / 200k / 950 (no trailing ``.0``)."""
        return format_tokens(value)

    @classmethod
    def _context_bar(cls, used: int, window: int) -> str:
        """``ctx 12k/1M ██░░░░░░ 1.2%`` — load of the active context window."""
        pct = used / window if window > 0 else 0.0
        cells = 8
        filled = min(cells, round(pct * cells))
        bar = "█" * filled + "░" * (cells - filled)
        return f"ctx {cls._fmt_tokens(used)}/{cls._fmt_tokens(window)} {bar} {pct:.1%}"

    def _stats_text(self) -> Text:
        runtime = self._runtime
        usage = runtime.usage
        session_id = runtime.session_id
        line = Text(style="dim")
        line.append(runtime.model)
        line.append(" · ")
        line.append(
            self._context_bar(runtime.context_tokens_used(), runtime.context_window)
        )
        line.append(f" · 输入 {format_tokens(usage.input_tokens)} / 输出 {format_tokens(usage.output_tokens)}")
        line.append(f" · 缓存 {usage.cache_hit_rate:.0%}")
        line.append(f" · 会话 {session_id[:8] if session_id else '未开始'}")
        return line

    def _refresh_stats(self) -> None:
        self.query_one("#status-right", Static).update(self._stats_text())

    def _refresh_status(self, status: str) -> None:
        mode = self._permission_mode()
        mode_label = f" · 权限:{_MODE_SHORT[mode]}" if mode is not None else ""
        left = Text(
            f"{self._short_path(self._setup.workspace)} · {status}{mode_label}",
            style="dim",
        )
        self.query_one("#status-left", Static).update(left)
        self._refresh_stats()

    def _set_busy(self, busy: bool, status: str) -> None:
        self._busy = busy
        self.query_one("#spinner", LoadingIndicator).display = busy
        prompt = self.query_one("#prompt", PromptArea)
        prompt.disabled = busy
        if not busy:
            prompt.focus()
        self._refresh_status(status)

    # -- streaming callbacks (run inside the turn worker) ---------------------

    async def _on_text_delta(self, delta: str) -> None:
        if self._current_reply is None:
            self._current_reply = StreamedReply()
            self._mount(self._current_reply)
        self._current_reply.append(delta)

    def _end_streaming(self) -> None:
        """Freeze the current streaming bubble (next deltas open a new one)."""
        self._current_reply = None

    async def _on_event(self, event: Event) -> None:
        etype = event.type
        if etype is EventType.TOOL_CALL_START:
            self._end_streaming()
            data = event.data
            call_id = str(data.get("call_id", ""))
            name = str(data.get("name", "?"))
            card = ToolCard(call_id, name, _tool_args_summary(name, data.get("arguments") or {}))
            self._cards[call_id] = card
            self._mount(card)
            self._refresh_status(f"执行工具 {name}")
            return
        if etype is EventType.TOOL_CALL_RESULT:
            call_id = str(event.data.get("call_id", ""))
            card = self._cards.get(call_id)
            if card is None:
                # Result without a seen start (e.g. recovered calls).
                name = str(event.data.get("name", "?"))
                card = ToolCard(call_id, name, "")
                self._cards[call_id] = card
                self._mount(card)
            card.set_result(event.data)
            return
        if etype is EventType.ROUND_START:
            self._refresh_status("思考中…")
            return
        if etype is EventType.APPROVAL_REQUEST:
            self._refresh_status("等待审批")
            return
        if etype is EventType.ASSISTANT_MESSAGE:
            self._end_streaming()
            self._refresh_stats()
            return
        if etype is EventType.SESSION_END:
            self._refresh_stats()
            return
        if etype in (
            EventType.GOAL_CHECK,
            EventType.CONTEXT_COMPACTED,
            EventType.BACKGROUND_JOB_COMPLETED,
            EventType.BACKGROUND_JOB_LOST,
            EventType.SIDE_EFFECT_UNKNOWN,
        ):
            self._end_streaming()
            self._add_line(_p1_line(event), "msg-event")

    # -- turn execution -------------------------------------------------------

    def _append_user_message(self, text: str) -> None:
        line = Text()
        line.append("❯ ", style="bold cyan")
        line.append(text)
        self._add_line(line, "msg-user")

    async def _run_turn(self, text: str) -> None:
        """One model turn inside a Textual worker (input is disabled meanwhile)."""
        self._end_streaming()
        self._set_busy(True, "思考中…")
        try:
            result = await self._runtime.run_turn(text)
        except asyncio.CancelledError:
            # The runtime already persisted the session as cancelled.
            self._end_streaming()
            self._abort_pending_approval()
            self._add_line(Text("回合已取消，会话状态已保存。", style="yellow"), "msg-warn")
            self._set_busy(False, "已取消")
            raise
        except Exception as exc:  # noqa: BLE001 - presentation must survive runtime bugs
            self._end_streaming()
            self._add_line(Text(f"回合执行失败: {exc}", style="red"), "msg-warn")
            self._set_busy(False, "就绪")
            return
        self._end_streaming()
        self._add_line(_turn_summary_text(result), "msg-summary")
        self._set_busy(False, "就绪")

    # -- approval adapter -----------------------------------------------------

    async def _approval_handler(self, request: ApprovalRequest) -> ApprovalDecision:
        """Push the approval modal and park the turn on a future until the
        user picks a button (or the turn gets cancelled)."""
        loop = asyncio.get_running_loop()
        fut: asyncio.Future[ApprovalDecision] = loop.create_future()
        self._approval_future = fut

        def _closed(decision: ApprovalDecision | None) -> None:
            if not fut.done():
                fut.set_result(
                    decision
                    if decision is not None
                    else ApprovalDecision(granted=False, reason="审批弹窗被关闭")
                )

        self.push_screen(ApprovalModal(request), _closed)
        try:
            return await fut
        finally:
            self._approval_future = None

    def _abort_pending_approval(self) -> None:
        """Close a pending approval dialog after the turn was cancelled."""
        fut = self._approval_future
        if fut is not None and not fut.done():
            fut.cancel()
        self._approval_future = None
        if isinstance(self.screen, ApprovalModal):
            self.pop_screen()

    # -- message handling -----------------------------------------------------

    def on_prompt_area_submitted(self, event: PromptArea.Submitted) -> None:
        self.slash_close()
        text = event.text.strip()
        event.prompt_area.clear()
        if not text or self._busy:
            return
        if text.startswith("/"):
            self._handle_slash(text)
            return
        self._append_user_message(text)
        self._turn_worker = self.run_worker(
            self._run_turn(text), group="turn", exclusive=False, description="agent turn"
        )

    def action_cancel_or_exit(self) -> None:
        """Ctrl+C: cancel the running turn; when idle, press twice to quit."""
        if self._busy:
            if self._turn_worker is not None:
                self._turn_worker.cancel()
            return
        now = time.monotonic()
        if now - self._last_idle_ctrl_c <= _DOUBLE_CTRL_C_WINDOW_S:
            self.exit()
            return
        self._last_idle_ctrl_c = now
        self._refresh_status("再按一次 Ctrl+C 退出")

    def action_clear_log(self) -> None:
        self._clear_log()

    # -- slash autocomplete: cascading two-level menu -------------------------

    @property
    def slash_visible(self) -> bool:
        """Whether the menu (root list or submenu) is currently shown."""
        return self._menu_open and not self._busy

    @property
    def slash_view(self) -> str | None:
        """Active menu view: ``"root"``, ``"submenu"``, or None when closed."""
        if not self.slash_visible:
            return None
        return "submenu" if self._submenu_cmd is not None else "root"

    def _prompt(self) -> PromptArea:
        return self.query_one("#prompt", PromptArea)

    def _menu(self) -> SlashMenu:
        return self.query_one("#slash-menu", SlashMenu)

    def refresh_slash_menu(self) -> None:
        """Re-derive the menu view from the prompt text (after key/paste).

        Derivation rules: a bare ``/prefix`` shows the filtered root list; a
        complete submenu command with no argument yet (``/model``,
        ``/model ``) opens its cascading submenu; once an argument is being
        typed the menu closes (direct-execution bypass).
        """
        prefix = self._prompt().text.strip().lower()
        if self._busy or not prefix.startswith("/"):
            self.slash_close()
            return
        if prefix != self._slash_prefix:
            self._slash_prefix = prefix
            self._backed_out = False
            self._slash_index = 0
        tokens = prefix.split()
        verb = tokens[0] if tokens else prefix
        if len(tokens) > 1:  # argument being typed: direct execution path
            self.slash_close()
            return
        if self._submenu_cmd is not None and verb == self._submenu_cmd:
            self._open_submenu(self._submenu_cmd)  # stay in the open submenu
            return
        if verb in SUBMENU_COMMANDS and not self._backed_out:
            self._open_submenu(verb)
            return
        # Root list; after an Esc-back we keep the command itself visible.
        items = filter_commands(verb if self._backed_out else prefix)
        if not items:
            self.slash_close()
            return
        self._menu_open = True
        self._submenu_cmd = None
        self._submenu_entries = []
        self._root_items = items
        self._slash_index = min(self._slash_index, len(items) - 1)
        self._menu().show_root(items, self._slash_index)

    def _open_submenu(self, cmd: str) -> None:
        """Enter (or re-render) the submenu for *cmd* (``/model`` / ``/effort``)."""
        entries = self._build_submenu_entries(cmd)
        if not entries:
            # Nothing to offer (e.g. /resume with no stored session): fall back
            # to direct execution so Enter still runs the command and explains.
            self._submenu_cmd = None
            self._submenu_entries = []
            self.slash_close()
            return
        if self._submenu_cmd != cmd:
            # Entering: highlight the currently active value.
            self._submenu_cmd = cmd
            self._slash_index = next(
                (i for i, entry in enumerate(entries) if "当前" in entry.badges), 0
            )
        self._menu_open = True
        self._submenu_entries = entries
        self._root_items = []
        self._menu().show_submenu(
            self._submenu_crumb(cmd), entries, self._slash_index
        )

    def _submenu_crumb(self, cmd: str) -> str:
        """Breadcrumb header of a submenu (``命令 › /model · 选择模型``)."""
        return f"命令 › {cmd} · {SUBMENU_COMMANDS[cmd]}"

    def _build_submenu_entries(self, cmd: str) -> list[SubmenuItem]:
        """Submenu options for *cmd*, annotated with live runtime state.

        Every command in :data:`SUBMENU_COMMANDS` gets its option list here;
        an empty list means "no options available" and closes the menu.
        """
        if cmd == "/model":
            current = self._runtime.model
            entries = []
            for info in MODEL_CATALOG.values():
                badges = []
                if info.name == current:
                    badges.append("当前")
                if info.name == DEFAULT_MODEL:
                    badges.append("默认")
                entries.append(
                    SubmenuItem(
                        value=info.name,
                        detail=f"上下文 {self._fmt_tokens(info.context_window)}",
                        badges=tuple(badges),
                    )
                )
            return entries
        if cmd == "/effort":
            current = getattr(self._runtime.provider, "reasoning_effort", None) or "off"
            details = {
                "off": "不发送，跟随网关默认",
                "low": "低推理预算",
                "medium": "中推理预算（默认档）",
                "high": "高推理预算",
                "xhigh": "超高推理预算",
                "max": "最大推理预算",
            }
            return [
                SubmenuItem(
                    value=level,
                    detail=details.get(level, level),
                    badges=("当前",) if level == current else (),
                )
                for level in EFFORT_LEVELS
            ]
        if cmd == "/permissions":
            policy = self._services.policy
            current = policy.mode.value if isinstance(policy, ModePolicy) else None
            return [
                SubmenuItem(
                    value=mode.value,
                    detail=PERMISSION_MODE_LABELS[mode],
                    badges=("当前",) if mode.value == current else (),
                )
                for mode in PermissionMode
            ]
        if cmd == "/resume":
            return self._resume_entries()
        return []

    def _resume_entries(self) -> list[SubmenuItem]:
        """Stored sessions as pickable options (value = 8-char session id)."""
        current = self._runtime.session_id
        return [
            SubmenuItem(
                value=session.session_id[:8],
                detail=(
                    f"{_status_label(session.status, session.exit_reason)}"
                    f" · {session.rounds} 轮"
                    f" · {format_tokens(session.input_tokens + session.output_tokens)} token"
                    f" · {session.provider}/{session.model}"
                    f" · {_format_timestamp(session.created_at)}"
                ),
                badges=("当前",) if session.session_id == current else (),
            )
            for session in self._store.list_sessions()[:20]
        ]

    def slash_move(self, delta: int) -> None:
        """Move the highlight by *delta* (wrapping both ways)."""
        items: list[Any] = self._submenu_entries if self._submenu_cmd else self._root_items
        if not items:
            return
        self._slash_index = (self._slash_index + delta) % len(items)
        if self._submenu_cmd is not None:
            self._menu().show_submenu(
                self._submenu_crumb(self._submenu_cmd), self._submenu_entries, self._slash_index
            )
        else:
            self._menu().show_root(self._root_items, self._slash_index)

    def slash_complete(self) -> None:
        """Tab: complete the highlighted entry into the prompt.

        In the root list a submenu command descends into its submenu; in a
        submenu it fills ``/cmd value`` so Enter executes directly.
        """
        if not self.slash_visible:
            return
        prompt = self._prompt()
        if self._submenu_cmd is not None:
            entry = self._submenu_entries[self._slash_index]
            _set_text_caret_end(prompt, f"{self._submenu_cmd} {entry.value} ")
            self.slash_close()
            return
        cmd = self._root_items[self._slash_index]
        if cmd.name in SUBMENU_COMMANDS:
            self._backed_out = False
            _set_text_caret_end(prompt, cmd.name + " ")
            self.refresh_slash_menu()
            return
        _set_text_caret_end(prompt, cmd.name + " ")
        self.slash_close()

    def slash_confirm(self) -> None:
        """Enter: descend into a submenu, or complete + execute the command."""
        if not self.slash_visible:
            return
        prompt = self._prompt()
        if self._submenu_cmd is not None:
            if not self._submenu_entries:
                return
            entry = self._submenu_entries[self._slash_index]
            cmd = self._submenu_cmd
            self.slash_close()
            _set_text_caret_end(prompt, "")
            self._apply_submenu_choice(cmd, entry.value)
            return
        cmd = self._root_items[self._slash_index]
        if cmd.name in SUBMENU_COMMANDS:
            # Seamless switch: the popup becomes the submenu, nothing runs yet.
            self._backed_out = False
            _set_text_caret_end(prompt, cmd.name + " ")
            self.refresh_slash_menu()
            return
        _set_text_caret_end(prompt, cmd.name)
        self.slash_close()
        self.post_message(PromptArea.Submitted(cmd.name, prompt))

    def _apply_submenu_choice(self, cmd: str, value: str) -> None:
        """Apply the option picked in *cmd*'s submenu (one entry per command)."""
        if cmd == "/model":
            self._switch_model(value)
        elif cmd == "/effort":
            self._apply_effort(value)
        elif cmd == "/permissions":
            self._cmd_permissions(value)
        elif cmd == "/resume":
            self._cmd_resume(value)

    def slash_back(self) -> None:
        """Esc / Backspace: submenu -> root list -> closed."""
        if self._submenu_cmd is not None:
            self._backed_out = True
            self._submenu_cmd = None
            self._submenu_entries = []
            self._slash_index = 0
            # Keep _slash_prefix: the text is unchanged, so the derivation
            # must not treat this as a fresh edit and re-open the submenu.
            self.refresh_slash_menu()
            return
        self.slash_close()

    def slash_close(self) -> None:
        self._menu_open = False
        self._submenu_cmd = None
        self._root_items = []
        self._submenu_entries = []
        self._slash_index = 0
        self._slash_prefix = ""
        self._backed_out = False
        if self.is_mounted:
            self._menu().hide()

    # -- slash commands -------------------------------------------------------

    def _handle_slash(self, text: str) -> None:
        """Intercepted at the input layer; never sent to the model."""
        verb, _, arg = text.partition(" ")
        verb = verb.lower()
        arg = arg.strip()
        if verb in ("/help", "/?"):
            self._add_line(_help_text(), "msg-system")
        elif verb in ("/exit", "/quit"):
            self.exit()
        elif verb == "/clear":
            # 仅清空终端渲染；Agent 的会话上下文与用量保持不变。
            self._clear_log()
            self._add_line(
                Text("已清屏（会话上下文与 Token 用量保留）。", style="dim"),
                "msg-system",
            )
        elif verb == "/new":
            self._cmd_new()
        elif verb == "/model":
            self._cmd_model(arg)
        elif verb == "/effort":
            self._cmd_effort(arg)
        elif verb == "/permissions":
            self._cmd_permissions(arg)
        elif verb == "/sessions":
            self._cmd_sessions()
        elif verb == "/resume":
            self._cmd_resume(arg)
        elif verb == "/compact":
            self._cmd_compact()
        else:
            self._add_line(
                Text(f"未知命令 {verb}（/help 查看可用命令）", style="yellow"),
                "msg-warn",
            )

    def _cmd_new(self) -> None:
        """彻底重置上下文：重建 runtime，下一条消息开启全新会话。"""
        self._clear_log()
        self._build_runtime()
        self._add_line(
            Text("已重置对话上下文，新会话将在下一条消息时创建。", style="green"),
            "msg-system",
        )
        self._refresh_status("新会话")

    def _cmd_model(self, arg: str) -> None:
        if not arg:
            self.push_screen(
                ModelPickerModal(self._runtime.model), self._on_model_picked
            )
            return
        self._switch_model(arg)

    def _on_model_picked(self, name: str | None) -> None:
        if name:
            self._switch_model(name)

    def _switch_model(self, name: str) -> None:
        if name not in MODEL_CATALOG:
            self._add_line(
                Text(f"未知模型 {name}；可选：{'、'.join(MODEL_CATALOG)}", style="yellow"),
                "msg-warn",
            )
            return
        try:
            provider = _provider_for_model(name, carry_effort_from=self._setup.provider)
        except (ProviderRequestError, RuntimeError) as exc:
            self._add_line(Text(f"无法切换模型：{exc}", style="red"), "msg-warn")
            return
        info = MODEL_CATALOG[name]
        old = self._runtime.model
        # Keep /new consistent with the switched model.
        self._setup.provider = provider
        self._setup.provider_name = info.provider
        self._setup.model_label = info.name
        self._runtime.set_model(
            provider=provider, provider_name=info.provider, model=info.name
        )
        self.sub_title = f"{self._setup.workspace} · {info.provider}/{info.name}"
        self._add_line(
            Text(
                f"✔ 已切换模型 {old} → {info.name}"
                f"（上下文 {format_tokens(info.context_window)} token）",
                style="green",
            ),
            "msg-system",
        )
        self._refresh_status("就绪")

    def _cmd_effort(self, arg: str) -> None:
        provider = self._runtime.provider
        current = getattr(provider, "reasoning_effort", None)
        if not arg:
            label = current if current else "默认（跟随网关）"
            self._add_line(
                Text(f"当前推理预算: {label}；可选: {' / '.join(EFFORT_LEVELS)}"),
                "msg-system",
            )
            return
        level = parse_effort(arg)
        if level is None:
            self._add_line(
                Text(f"无效档位 {arg}；可选: {' / '.join(EFFORT_LEVELS)}", style="yellow"),
                "msg-warn",
            )
            return
        self._apply_effort(level)

    def _apply_effort(self, level: str) -> None:
        """Apply a validated effort level to the active provider (validated)."""
        provider = self._runtime.provider
        if not hasattr(provider, "reasoning_effort"):
            self._add_line(
                Text(
                    f"当前 provider（{getattr(provider, 'name', '?')}）"
                    "不支持推理预算调整。",
                    style="yellow",
                ),
                "msg-warn",
            )
            return
        provider.reasoning_effort = None if level == "off" else level
        shown = level if level != "off" else "off（跟随网关默认）"
        self._add_line(Text(f"✔ 推理预算已调整为 {shown}", style="green"), "msg-system")

    def _cmd_permissions(self, arg: str) -> None:
        policy = self._services.policy
        if not isinstance(policy, ModePolicy):
            self._add_line(
                Text("当前会话的权限策略不支持运行时切换。", style="yellow"), "msg-warn"
            )
            return
        if not arg:
            lines = [
                f"当前权限模式: {policy.mode.value}"
                f"（{PERMISSION_MODE_LABELS[policy.mode]}）",
                *permission_mode_lines(),
            ]
            self._add_line(Text("\n".join(lines)), "msg-system")
            return
        mode = parse_permission_mode(arg)
        if mode is None:
            options = " / ".join(m.value for m in PermissionMode)
            self._add_line(
                Text(f"无效模式 {arg}；可选: {options}", style="yellow"), "msg-warn"
            )
            return
        previous = policy.set_mode(mode)
        self._add_line(
            Text(
                f"权限模式已切换 {previous.value} → {mode.value}"
                f"（{PERMISSION_MODE_LABELS[mode]}）",
                style="green",
            ),
            "msg-system",
        )
        self._refresh_status("就绪")

    def _cmd_sessions(self) -> None:
        sessions = self._store.list_sessions()
        if not sessions:
            self._add_line(Text("暂无会话记录。"), "msg-system")
            return
        lines = ["会话列表（最新在前）："]
        for session in sessions[:20]:
            lines.append(
                f"  {session.session_id[:8]}"
                f"  {_status_label(session.status, session.exit_reason)}"
                f"  · 轮数 {session.rounds}"
                f"  · Token {format_tokens(session.input_tokens + session.output_tokens)}"
                f"  · {session.provider}/{session.model}"
                f"  · {session.workspace}"
                f"  · {_format_timestamp(session.created_at)}"
            )
        self._add_line(Text("\n".join(lines)), "msg-system")

    def _cmd_resume(self, arg: str) -> None:
        if not arg:
            self._add_line(Text("用法: /resume <会话ID8>", style="yellow"), "msg-warn")
            return
        resolved = _resolve_session_id(self._store, arg)
        if resolved is None:
            self._add_line(
                Text(f"未找到会话: {arg}（可用 /sessions 查看会话 ID）", style="red"),
                "msg-warn",
            )
            return
        try:
            summary = self._store.get_session(resolved)
            setup = _prepare_resume(summary, budget=self._setup.budget)
            services = _build_services(setup, self._store, Console(), self._yes)
            services.policy = self._services.policy
            runtime = AgentRuntime.resume(
                store=self._store,
                session_id=resolved,
                provider=setup.provider,
                registry=services.registry,
                policy=services.policy,
                workspace=None,  # keep the stored session's workspace
                provider_name=setup.provider_name,
                model=setup.model_label,
                budget=setup.budget,
                approval_handler=self._approval_handler,
                on_text_delta=self._on_text_delta,
                on_event=self._on_event,
                compactor=None,  # re-attached below, bound to this runtime
                goal_checker=services.goal_checker,
                evidence_ledger=services.evidence_ledger,
                background_manager=services.background_manager,
                artifact_store=services.artifact_store,
            )
        except (ValueError, RuntimeError, ProviderRequestError, Exit) as exc:
            self._add_line(Text(f"恢复会话失败: {exc}", style="red"), "msg-warn")
            return
        _attach_compactor(runtime, services.artifact_store)
        services.goal_checker = runtime._goal_checker
        services.evidence_ledger = runtime._evidence_ledger
        setup.workspace = runtime.workspace
        self._setup = setup
        self._services = services
        self._runtime = runtime
        self.sub_title = f"{setup.workspace} · {setup.provider_name}/{setup.model_label}"
        self._cards.clear()
        self._add_line(
            Text(
                f"已恢复会话 {resolved[:8]}"
                f" · 轮数 {runtime.rounds}"
                f" · Token {format_tokens(runtime.usage.total_tokens)}",
                style="green",
            ),
            "msg-system",
        )
        self._refresh_status("就绪")

    def _cmd_compact(self) -> None:
        """Manually run one compaction pass over the live session context."""
        runtime = self._runtime
        compactor = getattr(runtime, "_compactor", None)
        session_id = runtime.session_id
        if compactor is None or session_id is None:
            self._add_line(
                Text("压缩器不可用（会话尚未开始）。", style="yellow"), "msg-warn"
            )
            return
        system_prompt = getattr(runtime, "_system_prompt", "")
        messages = getattr(runtime, "_messages", [])
        specs = self._services.registry.specs()
        if not compactor.needs_compaction(system_prompt, messages, specs):
            self._add_line(Text("上下文未超过阈值，无需压缩。"), "msg-system")
            return
        result = compactor.compact(system_prompt, messages, specs)
        if not result.changed:
            self._add_line(Text("没有可压缩的内容。"), "msg-system")
            return
        stats = result.stats
        runtime._messages = result.messages
        self._store.replace_messages(session_id, result.messages)
        self._add_line(
            Text(
                f"上下文已压缩：估算 {format_tokens(stats.tokens_before)} → {format_tokens(stats.tokens_after)} token"
                f"（归档 {stats.archived_units} · 收缩 {stats.shrunk_results}"
                f" · 摘要 {stats.summarized_units}）"
            ),
            "msg-system",
        )


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def run_tui(*, setup: "_Setup", db_path: Path, yes: bool) -> None:
    """构建 SqliteStore + AgentRuntime（复用 cli 的 _build_services/_attach_compactor），
    启动 Textual App；store 在 App 退出后关闭。"""
    from minicode.storage import SqliteStore

    store = SqliteStore(db_path)
    try:
        services = _build_services(setup, store, Console(), yes)
        MiniCodeApp(setup=setup, store=store, services=services, yes=yes).run()
    finally:
        store.close()
