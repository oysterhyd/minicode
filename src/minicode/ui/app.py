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

from minicode.cli import (
    _Setup,
    _attach_compactor,
    _build_services,
    _exit_label,
    _format_timestamp,
    _resolve_session_id,
    _status_label,
    _success_brief,
    _tool_args_summary,
)
from minicode.core.models import (
    ApprovalDecision,
    ApprovalRequest,
    Event,
    EventType,
    RunResult,
)
from minicode.runtime import AgentRuntime

#: Seconds between two idle Ctrl+C presses that quits the app.
_DOUBLE_CTRL_C_WINDOW_S = 2.0

#: How many lines/chars of a tool output preview are shown in a tool card.
_PREVIEW_MAX_LINES = 6
_PREVIEW_MAX_CHARS_PER_LINE = 160

_HELP_TEXT = Text(
    "可用命令：\n"
    "  /help             显示本帮助\n"
    "  /exit             退出（或 Ctrl+Q）\n"
    "  /sessions         列出历史会话\n"
    "  /resume <id8>     恢复一个历史会话\n"
    "  /compact          手动压缩当前会话上下文\n"
    "  /clear            清空界面显示（不影响会话）\n"
    "快捷键：Enter 提交 · Shift+Enter 换行 · Ctrl+C 取消回合/再按一次退出 · Ctrl+L 清屏"
)


# ---------------------------------------------------------------------------
# Input widgets
# ---------------------------------------------------------------------------


class PromptArea(TextArea):
    """Multi-line prompt: Enter submits, Shift+Enter / Alt+Enter newline."""

    class Submitted(Message):
        """Posted when the user presses Enter to submit the prompt."""

        def __init__(self, text: str, prompt_area: "PromptArea") -> None:
            super().__init__()
            self.text = text
            self.prompt_area = prompt_area

    async def _on_key(self, event: events.Key) -> None:
        """Intercept Enter (submit) before TextArea turns it into a newline."""
        if event.key == "enter":
            event.stop()
            event.prevent_default()
            self.post_message(self.Submitted(self.text, self))
            return
        if event.key in ("shift+enter", "alt+enter"):
            event.stop()
            event.prevent_default()
            self.insert("\n")
            return
        await super()._on_key(event)


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
            f"  ◆ 上下文已压缩：估算 {data.get('tokens_before')} → "
            f"{data.get('tokens_after')} token",
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
            f"  ◆ 后台任务 {data.get('job_id')} 失联（进程已不在）", style="yellow"
        )
    if event.type is EventType.SIDE_EFFECT_UNKNOWN:
        return Text(
            f"  ◆ {data.get('name')} 的副作用状态未知，需要先核实", style="yellow"
        )
    if event.type is EventType.SUBAGENT_FINISHED:
        usage = data.get("usage") or {}
        return Text(
            f"  ◆ 子代理完成（token {usage.get('input_tokens', 0)}+"
            f"{usage.get('output_tokens', 0)}）",
            style="dim",
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
        f" · Token: {usage.total_tokens}"
        f"（输入 {usage.input_tokens} / 输出 {usage.output_tokens}）"
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
    .tool-card { height: auto; margin-top: 1; }
    .tool-card > Static { height: auto; }
    .tool-body { color: $text-muted; height: auto; }
    .msg-event { color: $text-muted; margin-top: 1; }
    .msg-system { margin-top: 1; }
    .msg-warn { color: $warning; margin-top: 1; }
    .msg-summary { color: $text-muted; margin-top: 1; }
    #input-dock { height: auto; padding: 0 1 1 1; }
    #spinner { display: none; height: 1; }
    #prompt { height: 4; border: round $primary; margin-bottom: 1; }
    #prompt:focus { border: round $accent; }
    #statusbar { height: 1; }
    #status-left { width: 1fr; color: $text-muted; }
    #status-right { width: auto; color: $text-muted; }
    ApprovalModal { align: center middle; }
    #approval-dialog {
        width: 64; height: auto;
        border: round $warning; background: $surface; padding: 1 2;
    }
    .approval-title { text-style: bold; color: $warning; margin-bottom: 1; }
    #approval-body { margin-bottom: 1; }
    #approval-buttons { height: auto; align-horizontal: right; }
    #approval-buttons Button { margin-left: 1; }
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
            approval_handler=None if self._yes else self._approval_handler,
            on_text_delta=self._on_text_delta,
            on_event=self._on_event,
            background_manager=services.background_manager,
            artifact_store=services.artifact_store,
            goal_checker=services.goal_checker,
            evidence_ledger=services.evidence_ledger,
        )
        _attach_compactor(
            runtime, services.artifact_store, self._setup.budget.max_total_tokens
        )
        self._runtime = runtime

    # -- layout ---------------------------------------------------------------

    def compose(self) -> ComposeResult:
        yield Header()
        with VerticalScroll(id="log"):
            pass
        with Vertical(id="input-dock"):
            yield LoadingIndicator(id="spinner")
            yield PromptArea(
                placeholder="输入任务或问题，/help 查看命令…",
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
        self.query_one("#log", VerticalScroll).anchor()
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

    def _stats_text(self) -> Text:
        runtime = self._runtime
        session_id = runtime.session_id
        return Text(
            f"轮数 {runtime.rounds} · Token {runtime.usage.total_tokens}"
            f" · 会话 {session_id[:8] if session_id else '未开始'}",
            style="dim",
        )

    def _refresh_stats(self) -> None:
        self.query_one("#status-right", Static).update(self._stats_text())

    def _refresh_status(self, status: str) -> None:
        self.query_one("#status-left", Static).update(Text(status))
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
            EventType.SUBAGENT_FINISHED,
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

    # -- slash commands -------------------------------------------------------

    def _handle_slash(self, text: str) -> None:
        """Intercepted at the input layer; never sent to the model."""
        verb, _, arg = text.partition(" ")
        verb = verb.lower()
        arg = arg.strip()
        if verb in ("/help", "/?"):
            self._add_line(_HELP_TEXT, "msg-system")
        elif verb in ("/exit", "/quit"):
            self.exit()
        elif verb == "/clear":
            self._clear_log()
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
                f"  · Token {session.input_tokens + session.output_tokens}"
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
        services = self._services
        try:
            runtime = AgentRuntime.resume(
                store=self._store,
                session_id=resolved,
                provider=self._setup.provider,
                registry=services.registry,
                policy=services.policy,
                workspace=None,  # keep the stored session's workspace
                provider_name=self._setup.provider_name,
                model=self._setup.model_label,
                budget=self._setup.budget,
                approval_handler=None if self._yes else self._approval_handler,
                on_text_delta=self._on_text_delta,
                on_event=self._on_event,
                compactor=None,  # re-attached below, bound to this runtime
                goal_checker=services.goal_checker,
                evidence_ledger=services.evidence_ledger,
                background_manager=services.background_manager,
                artifact_store=services.artifact_store,
            )
        except ValueError as exc:
            self._add_line(Text(f"恢复会话失败: {exc}", style="red"), "msg-warn")
            return
        _attach_compactor(
            runtime, services.artifact_store, self._setup.budget.max_total_tokens
        )
        self._runtime = runtime
        self._cards.clear()
        self._add_line(
            Text(
                f"已恢复会话 {resolved[:8]}"
                f" · 轮数 {runtime.rounds}"
                f" · Token {runtime.usage.total_tokens}",
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
                f"上下文已压缩：估算 {stats.tokens_before} → {stats.tokens_after} token"
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
