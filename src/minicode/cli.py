"""minicode CLI: the user-facing layer of the minimal closed loop.

Commands:

- ``run``      one-shot task turn (streamed text, tool one-liners, diff summary)
- ``chat``     interactive REPL on one persistent session
- ``resume``   continue a persisted session (interrupted tool calls are settled)
- ``sessions`` session bookkeeping (``list``)
- ``report``   execution report for one session (text or offline HTML)
- ``eval``     run the local eval task set (delegates to evals/run_eval.py)
- ``tui``      full-screen Textual interface

The CLI only wires the backend packages (runtime / providers / tools /
security / storage) together and adds presentation; it never re-implements
runtime logic. User-facing strings are Chinese, code is English.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Annotated, Any, NoReturn

import typer
from pydantic import ValidationError
from rich.console import Console
from rich.panel import Panel
from rich.prompt import Confirm
from rich.table import Table
from rich.text import Text

from minicode.banner import print_banner
from minicode.core.catalog import (
    DEFAULT_MODEL,
    EFFORT_LEVELS,
    MODEL_CATALOG,
    lookup_model,
    parse_effort,
)
from minicode.core.models import (
    ApprovalDecision,
    ApprovalHandler,
    ApprovalRequest,
    Budget,
    Event,
    EventType,
    RunResult,
    ToolResultBlock,
)
from minicode.providers import (
    FakeProvider,
    FakeProviderOptions,
    FakeToolCall,
    FakeTurn,
    Provider,
    ProviderRequestError,
)
from minicode.runtime import AgentRuntime
from minicode.security import (
    PERMISSION_MODE_LABELS,
    ModePolicy,
    PermissionMode,
    PermissionPolicy,
    parse_permission_mode,
    permission_mode_lines,
)
from minicode.slash import format_command_lines
from minicode.storage import DEFAULT_DB_PATH, SessionStore, SessionSummary, SqliteStore
from minicode.tools.registry import default_registry

app = typer.Typer(
    help="minicode —— 轻量级 CLI 编码 Agent（P0 最小闭环）。",
    no_args_is_help=True,
    add_completion=False,
)
sessions_app = typer.Typer(help="查看历史会话。", no_args_is_help=True)
app.add_typer(sessions_app, name="sessions")


# ---------------------------------------------------------------------------
# Constants and small helpers
# ---------------------------------------------------------------------------

#: ExitReason value -> Chinese label (unknown values fall back to the raw one).
_EXIT_LABELS: dict[str, str] = {
    "completed": "已完成",
    "max_rounds": "达到最大轮数",
    "token_budget": "Token 预算耗尽",
    "time_budget": "时长预算耗尽",
    "cancelled": "已取消",
    "goal_not_met": "验收未通过",
    "provider_error": "模型调用失败",
    "internal_error": "内部错误",
}

#: Field allowlists for the FakeProvider script JSON (unknown fields are errors).
_SCRIPT_OPTION_FIELDS = frozenset(
    {"turns", "exhausted_text", "default_input_tokens", "default_output_tokens"}
)
_SCRIPT_TURN_FIELDS = frozenset(
    {"text", "tool_calls", "input_tokens", "output_tokens", "stop_reason"}
)
_SCRIPT_CALL_FIELDS = frozenset({"name", "arguments", "id"})

#: Built-in single-turn script when provider=fake and no --script is given.
_DEMO_REPLY = "(fake provider：未提供脚本，这里是一条演示回复。)"


def _fail(message: str) -> NoReturn:
    """Print a Chinese error and exit non-zero (errors go to stderr)."""
    typer.secho(message, fg=typer.colors.RED, err=True)
    raise typer.Exit(1)


def _exit_label(value: str) -> str:
    return _EXIT_LABELS.get(value, value)


def _status_label(status: str, exit_reason: str | None) -> str:
    if exit_reason:
        return _exit_label(exit_reason)
    return "进行中" if status == "running" else status


def _format_timestamp(iso_timestamp: str) -> str:
    """'2026-09-19T15:04:05.123+00:00' -> '2026-09-19 15:04:05'."""
    return iso_timestamp[:19].replace("T", " ")


def _session_list_lines(sessions: list[SessionSummary]) -> list[str]:
    """Formatted lines for the /sessions listing (latest first, capped at 20)."""
    return [
        f"  {session.session_id[:8]}"
        f"  {_status_label(session.status, session.exit_reason)}"
        f"  · 轮数 {session.rounds}"
        f"  · Token {session.input_tokens + session.output_tokens}"
        f"  · {session.provider}/{session.model}"
        f"  · {session.workspace}"
        f"  · {_format_timestamp(session.created_at)}"
        for session in sessions[:20]
    ]


# ---------------------------------------------------------------------------
# FakeProvider script loading
# ---------------------------------------------------------------------------


def _reject_unknown_fields(raw: dict[str, Any], allowed: frozenset[str], where: str) -> None:
    unknown = sorted(set(raw) - allowed)
    if unknown:
        _fail(
            f"{where} 包含未知字段: {', '.join(unknown)}"
            f"（允许的字段: {', '.join(sorted(allowed))}）"
        )


def _parse_tool_call(raw_call: Any, turn_index: int, call_index: int) -> FakeToolCall:
    where = f"turns[{turn_index}].tool_calls[{call_index}]"
    if not isinstance(raw_call, dict):
        _fail(f"{where} 必须是对象")
    _reject_unknown_fields(raw_call, _SCRIPT_CALL_FIELDS, where)
    try:
        return FakeToolCall(**raw_call)
    except ValidationError as exc:
        _fail(f"{where} 字段不合法: {exc}")


def _parse_turn(raw_turn: Any, turn_index: int) -> FakeTurn:
    where = f"turns[{turn_index}]"
    if not isinstance(raw_turn, dict):
        _fail(f"{where} 必须是对象")
    _reject_unknown_fields(raw_turn, _SCRIPT_TURN_FIELDS, where)
    raw_calls = raw_turn.get("tool_calls", [])
    if not isinstance(raw_calls, list):
        _fail(f"{where}.tool_calls 必须是数组")
    calls = [
        _parse_tool_call(raw_call, turn_index, call_index)
        for call_index, raw_call in enumerate(raw_calls)
    ]
    kwargs = {key: value for key, value in raw_turn.items() if key != "tool_calls"}
    try:
        return FakeTurn(tool_calls=calls, **kwargs)
    except ValidationError as exc:
        _fail(f"{where} 字段不合法: {exc}")


def _load_script(path: Path) -> FakeProviderOptions:
    """Parse a FakeProvider script JSON file, failing loudly on anything off."""
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        _fail(f"脚本文件不存在: {path}")
    except OSError as exc:
        _fail(f"无法读取脚本文件 {path}: {exc}")
    except json.JSONDecodeError as exc:
        _fail(f"脚本不是有效的 JSON: {path}: {exc}")
    if not isinstance(raw, dict):
        _fail("脚本根节点必须是 JSON 对象")
    _reject_unknown_fields(raw, _SCRIPT_OPTION_FIELDS, "脚本")
    raw_turns = raw.get("turns", [])
    if not isinstance(raw_turns, list):
        _fail("脚本的 turns 必须是数组")
    turns = [_parse_turn(raw_turn, index) for index, raw_turn in enumerate(raw_turns)]
    kwargs = {key: value for key, value in raw.items() if key != "turns"}
    try:
        return FakeProviderOptions(turns=turns, **kwargs)
    except ValidationError as exc:
        _fail(f"脚本字段不合法: {exc}")


class ProviderChoice(str, Enum):
    """``--provider`` choices; ``auto`` prefers commandcode (ZCode 网关),
    then anthropic (when a key exists), then the offline fake provider."""

    auto = "auto"
    fake = "fake"
    anthropic = "anthropic"
    commandcode = "commandcode"


def _commandcode_available() -> bool:
    try:
        from minicode.providers.zcode_config import discover_commandcode
    except ImportError:  # pragma: no cover - providers package always present
        return False
    return discover_commandcode() is not None


def _provider_for_model(
    name: str, *, carry_effort_from: Provider | None = None
) -> Provider:
    """Build a provider for a *catalog* model name (caller validates it).

    Shared by the ``/model`` command in both frontends; the current
    reasoning effort is carried over so switching models keeps it. Raises
    :class:`ProviderRequestError` (missing gateway credentials) or
    ``RuntimeError`` (missing ``ANTHROPIC_API_KEY``); callers surface the
    message.
    """
    info = MODEL_CATALOG[name]
    effort = getattr(carry_effort_from, "reasoning_effort", None)
    if info.provider == "commandcode":
        from minicode.providers.commandcode import CommandCodeProvider

        return CommandCodeProvider(model=info.name, reasoning_effort=effort)
    if info.provider == "anthropic":
        if not os.environ.get("ANTHROPIC_API_KEY"):
            raise RuntimeError("切换到 anthropic 模型需要设置 ANTHROPIC_API_KEY 环境变量。")
        from minicode.providers.anthropic_provider import AnthropicProvider

        return AnthropicProvider(model=info.name)
    return FakeProvider()


def _build_provider(
    provider_choice: ProviderChoice, model: str | None, script: Path | None
) -> tuple[Provider, str, str]:
    """Resolve ``--provider`` into ``(provider, provider_name, model_label)``.

    An explicit ``--model`` that names a catalog model pins the provider
    choice too: ``--model z.ai/glm-5.3-flash`` routes through the
    commandcode gateway even under ``--provider auto``.
    """
    if provider_choice is ProviderChoice.auto:
        if model is not None and lookup_model(model).provider != "anthropic":
            provider_choice = ProviderChoice.commandcode
        else:
            provider_choice = (
                ProviderChoice.commandcode
                if _commandcode_available()
                else (
                    ProviderChoice.anthropic
                    if os.environ.get("ANTHROPIC_API_KEY")
                    else ProviderChoice.fake
                )
            )

    if provider_choice is ProviderChoice.commandcode:
        try:
            # Lazy import: keeps startup light and test environments hermetic.
            from minicode.providers.commandcode import CommandCodeProvider
        except ImportError as exc:
            _fail(f"无法加载 commandcode provider：{exc}")
        chosen = model or DEFAULT_MODEL
        return CommandCodeProvider(model=chosen), "commandcode", chosen

    if provider_choice is ProviderChoice.anthropic:
        if not os.environ.get("ANTHROPIC_API_KEY"):
            _fail("provider 为 anthropic 时需要设置 ANTHROPIC_API_KEY 环境变量。")
        try:
            # Lazy import: the SDK is optional (minicode[real]).
            from minicode.providers.anthropic_provider import AnthropicProvider
        except ImportError as exc:
            _fail(f"未安装 anthropic SDK，请先执行 pip install 'minicode[real]'。({exc})")
        chosen = model or "claude-sonnet-4-5"
        return AnthropicProvider(model=chosen), "anthropic", chosen

    options = (
        _load_script(script)
        if script is not None
        else FakeProviderOptions(turns=[FakeTurn(text=_DEMO_REPLY)])
    )
    return FakeProvider(options), "fake", "fake"


# ---------------------------------------------------------------------------
# Shared option aliases (run / chat)
# ---------------------------------------------------------------------------

WorkspaceOpt = Annotated[Path, typer.Option("--workspace", help="工作区目录（默认当前目录）")]
ProviderOpt = Annotated[ProviderChoice, typer.Option(case_sensitive=False, help="模型提供方")]
ModelOpt = Annotated[str | None, typer.Option(help="模型名称（缺省按 provider 选择默认模型）")]
ScriptOpt = Annotated[
    Path | None,
    typer.Option(help="FakeProvider 脚本 JSON；fake 且未提供时使用内置演示脚本"),
]
MaxRoundsOpt = Annotated[int, typer.Option(help="会话最大轮数")]
MaxTokensOpt = Annotated[int, typer.Option(help="token 总预算")]
MaxSecondsOpt = Annotated[float, typer.Option(help="每轮时长预算（秒）")]
YesOpt = Annotated[bool, typer.Option("--yes", "-y", help="自动允许全部工具调用，不再逐个审批")]
DbOpt = Annotated[Path, typer.Option(help="会话数据库路径（默认 ~/.minicode/sessions.db）")]
AcceptanceOpt = Annotated[
    Path | None,
    typer.Option(
        "--acceptance",
        help="验收配置 YAML（command/artifact/protected 项）；设置后模型自述完成不等于通过",
    ),
]


@dataclass(slots=True)
class _Setup:
    """Everything run/chat need after option resolution."""

    workspace: Path
    provider: Provider
    provider_name: str
    model_label: str
    budget: Budget
    acceptance: Path | None = None


def _prepare(
    workspace: Path,
    provider_choice: ProviderChoice,
    model: str | None,
    script: Path | None,
    max_rounds: int,
    max_tokens: int,
    max_seconds: float,
    acceptance: Path | None = None,
) -> _Setup:
    """Validate options and build provider + budget (shared by run/chat)."""
    resolved = Path(workspace).expanduser()
    if not resolved.exists():
        _fail(f"工作区不存在: {resolved}")
    if not resolved.is_dir():
        _fail(f"工作区不是目录: {resolved}")
    if acceptance is not None and not acceptance.exists():
        _fail(f"验收配置不存在: {acceptance}")
    provider, provider_name, model_label = _build_provider(provider_choice, model, script)
    return _Setup(
        workspace=resolved,
        provider=provider,
        provider_name=provider_name,
        model_label=model_label,
        budget=Budget(
            max_rounds=max_rounds, max_total_tokens=max_tokens, max_seconds=max_seconds
        ),
        acceptance=acceptance,
    )


# ---------------------------------------------------------------------------
# Presentation helpers
# ---------------------------------------------------------------------------


class _StreamPrinter:
    """Renders streamed assistant text and tracks whether the current output
    line is still open, so tool output can start on a fresh line."""

    def __init__(self, console: Console) -> None:
        self._console = console
        self._line_open = False

    def write_delta(self, delta: str) -> None:
        # markup=False: model text must never be interpreted as rich markup.
        self._console.print(delta, end="", markup=False, highlight=False)
        self._line_open = True

    def end_line(self) -> None:
        if self._line_open:
            self._console.print()
            self._line_open = False


def _tool_args_summary(name: str, arguments: dict[str, Any]) -> str:
    """Compact one-line description of a tool call's arguments."""
    if name == "bash" and isinstance(arguments.get("command"), str):
        return str(arguments["command"])
    if name in ("edit", "write"):
        return str(arguments.get("path", ""))
    parts: list[str] = []
    for key, value in arguments.items():
        rendered = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False)
        parts.append(f"{key}={rendered}")
    summary = " ".join(parts)
    return summary if len(summary) <= 80 else summary[:77] + "..."


def _success_brief(name: str, data: dict[str, Any]) -> str:
    """Short excerpt shown after '✓ <tool>' on TOOL_CALL_RESULT."""
    preview = str(data.get("output_preview") or "")
    lines = [line for line in preview.splitlines() if line.strip()]
    if name == "bash" and lines:
        brief = lines[-1]  # e.g. pytest's "4 passed in 0.03s"
    elif name in ("edit", "write") and lines:
        brief = lines[0]  # "Applied change to x.py (+1 -1)"
    elif name == "read" and lines:
        brief = f"{len(lines)} 行"
    else:
        brief = "完成"
    return brief if len(brief) <= 64 else brief[:61] + "..."


def _print_tool_start(console: Console, data: dict[str, Any]) -> None:
    name = str(data.get("name", "?"))
    line = Text("  ▸ ", style="dim")
    line.append(name, style="bold cyan")
    summary = _tool_args_summary(name, data.get("arguments") or {})
    if summary:
        line.append(f" {summary}")
    console.print(line)


def _print_tool_result(console: Console, data: dict[str, Any]) -> None:
    name = str(data.get("name", "?"))
    if data.get("success"):
        line = Text("  ✓ ", style="green")
        line.append(name, style="green")
        line.append(f" {_success_brief(name, data)}")
        console.print(line)
        return
    error = str(data.get("error") or "执行失败")
    line = Text("  ✗ ", style="red")
    line.append(name, style="red")
    line.append(f" {error}", style="red")
    console.print(line)


def _make_interactive_approval(console: Console) -> ApprovalHandler:
    """Rich-interactive approval prompt used when ``--yes`` is absent.

    ``Confirm.ask`` blocks synchronously on stdin. minicode runs a single event loop
    in the main thread and nothing else needs it while the user answers, so
    briefly blocking inside this async callback is the simplest correct
    behavior. On EOF (non-interactive stdin) we deny instead of crashing.
    """

    async def handler(request: ApprovalRequest) -> ApprovalDecision:
        console.print(Text(f"  需要审批 {request.tool_name}: ", style="yellow").append(
            request.summary
        ))
        try:
            granted = Confirm.ask(
                f"允许 {request.tool_name}？", default=False, console=console
            )
        except EOFError:
            granted = False
        return ApprovalDecision(granted=granted)

    return handler


def _print_header(console: Console, goal: str, setup: _Setup) -> None:
    """Task header below the banner: goal plus the budget envelope."""
    console.print(f"[bold]目标[/]   {goal}")
    console.print(
        f"[bold]预算[/]   ≤ {setup.budget.max_rounds} 轮"
        f" · ≤ {setup.budget.max_total_tokens} token"
        f" · ≤ {setup.budget.max_seconds:g} 秒"
    )
    console.print()


def _print_turn_summary(console: Console, result: RunResult) -> None:
    usage = result.total_usage
    console.print()
    console.print(
        f"退出原因: {_exit_label(result.exit_reason.value)} ({result.exit_reason.value}) ·"
        f" 轮数: {result.rounds} ·"
        f" Token: {usage.total_tokens}（输入 {usage.input_tokens} / 输出 {usage.output_tokens}） ·"
        f" 耗时: {result.duration_s:.1f}s ·"
        f" 会话: {result.session_id[:8]}"
    )


def _print_diff_summary(console: Console, store: SessionStore, session_id: str) -> None:
    """Show the diffs produced by edit / write calls (from persisted events)."""
    patches = [
        event
        for event in store.get_events(session_id)
        if event.type is EventType.TOOL_CALL_RESULT
        and event.data.get("name") in ("edit", "write")
    ]
    if not patches:
        return
    console.print()
    console.print("[bold]修改摘要[/]")
    for event in patches:
        preview = str(event.data.get("output_preview") or "（无 diff 输出）")
        title = str(event.data.get("name") or "edit")
        console.print(Panel(Text(preview), title=title, border_style="cyan"))


# ---------------------------------------------------------------------------
# Runtime wiring and the turn runner
# ---------------------------------------------------------------------------


@dataclass(slots=True)
class _Services:
    """P1 services + presentation callbacks shared by fresh and resumed runtimes."""

    registry: Any
    policy: PermissionPolicy
    approval_handler: ApprovalHandler | None
    on_text_delta: Any
    on_event: Any
    background_manager: Any
    artifact_store: Any
    goal_checker: Any | None
    evidence_ledger: Any | None


def _build_services(setup: _Setup, store: SqliteStore, console: Console, yes: bool) -> _Services:
    """Assemble everything an AgentRuntime (fresh or resumed) needs."""
    from minicode.goals import AcceptanceSpec, EvidenceLedger, GoalChecker, ProtectedSnapshot
    from minicode.storage import ArtifactStore
    from minicode.tasks.background import BackgroundManager
    from minicode.tools.artifact import ReadArtifactTool

    printer = _StreamPrinter(console)

    async def on_text_delta(delta: str) -> None:
        printer.write_delta(delta)

    async def on_event(event: Event) -> None:
        if event.type is EventType.TOOL_CALL_START:
            printer.end_line()
            _print_tool_start(console, event.data)
        elif event.type is EventType.TOOL_CALL_RESULT:
            printer.end_line()
            _print_tool_result(console, event.data)
        else:
            _print_p1_event(console, event)

    registry = default_registry()
    registry.register(ReadArtifactTool())
    try:
        from minicode.tools.delegate import DelegateSubagentTool
    except ImportError:  # pragma: no cover - ships with minicode.tasks
        pass
    else:
        registry.register(
            DelegateSubagentTool(
                provider=setup.provider,
                workspace=setup.workspace,
                parent_budget=setup.budget,
                store=store,  # child events persist into the parent session
            )
        )

    policy: PermissionPolicy
    approval_handler: ApprovalHandler | None
    if yes:
        # --yes maps to the BYPASS permission mode; the interactive handler
        # is still created so /permissions can later tighten the mode.
        policy = ModePolicy(PermissionMode.BYPASS)
    else:
        policy = ModePolicy(PermissionMode.DEFAULT)
    approval_handler = _make_interactive_approval(console)

    goal_checker = None
    evidence_ledger = None
    if setup.acceptance is not None:
        spec = AcceptanceSpec.from_yaml(setup.acceptance)
        protected_paths = [item.path for item in spec.items if item.type == "protected"]
        snapshot = ProtectedSnapshot(setup.workspace, protected_paths)
        goal_checker = GoalChecker(spec, setup.workspace, protected_snapshot=snapshot)
        evidence_ledger = EvidenceLedger()

    return _Services(
        registry=registry,
        policy=policy,
        approval_handler=approval_handler,
        on_text_delta=on_text_delta,
        on_event=on_event,
        background_manager=BackgroundManager(),
        artifact_store=ArtifactStore(store),
        goal_checker=goal_checker,
        evidence_ledger=evidence_ledger,
    )


def _attach_compactor(runtime: Any, artifact_store: Any, max_total_tokens: int) -> None:
    """Attach the context compactor with artifact spill bound to the live
    session. The session id only exists after the first ``run_turn``
    (compaction runs strictly inside turns), so the closure reads it lazily
    off the runtime."""
    from minicode.context.compact import CompactConfig, ContextCompactor

    def spill(kind: str, content: str) -> str:
        session_id = runtime.session_id
        assert session_id is not None
        return artifact_store.spill(session_id, kind, content).artifact_id

    runtime._compactor = ContextCompactor(
        CompactConfig(max_context_tokens=max_total_tokens),
        spill_fn=spill,
    )


def _new_runtime(setup: _Setup, store: SqliteStore, services: Any) -> Any:
    """Build a fresh AgentRuntime from an already-assembled services bundle."""
    runtime = AgentRuntime(
        provider=setup.provider,
        registry=services.registry,
        store=store,
        policy=services.policy,
        workspace=setup.workspace,
        provider_name=setup.provider_name,
        model=setup.model_label,
        budget=setup.budget,
        approval_handler=services.approval_handler,
        on_text_delta=services.on_text_delta,
        on_event=services.on_event,
        background_manager=services.background_manager,
        artifact_store=services.artifact_store,
        goal_checker=services.goal_checker,
        evidence_ledger=services.evidence_ledger,
    )
    _attach_compactor(runtime, services.artifact_store, setup.budget.max_total_tokens)
    return runtime


def _print_p1_event(console: Console, event: Event) -> None:
    """One-line presentation for the P1 events worth surfacing live."""
    if event.type is EventType.GOAL_CHECK:
        passed = "通过" if event.data.get("passed") else "未通过"
        style = "green" if event.data.get("passed") else "yellow"
        console.print(Text(f"  ◆ 验收检查：{passed}", style=style))
        for item in event.data.get("items", []):
            mark = "✓" if item.get("passed") else "✗"
            item_style = "green" if item.get("passed") else "red"
            console.print(Text(f"    {mark} {item.get('item_id')}", style=item_style))
        return
    if event.type is EventType.CONTEXT_COMPACTED:
        before, after = event.data.get("tokens_before"), event.data.get("tokens_after")
        console.print(
            Text(f"  ◆ 上下文已压缩：估算 {before} → {after} token", style="dim")
        )
        return
    if event.type is EventType.BACKGROUND_JOB_COMPLETED:
        ok = event.data.get("status") == "completed"
        console.print(
            Text(
                f"  ◆ 后台任务 {event.data.get('job_id')} "
                f"退出码 {event.data.get('exit_code')}",
                style="green" if ok else "red",
            )
        )
        return
    if event.type is EventType.BACKGROUND_JOB_LOST:
        console.print(
            Text(f"  ◆ 后台任务 {event.data.get('job_id')} 失联（进程已不在）", style="yellow")
        )
        return
    if event.type is EventType.SIDE_EFFECT_UNKNOWN:
        console.print(
            Text(
                f"  ◆ {event.data.get('name')} 的副作用状态未知，需要先核实",
                style="yellow",
            )
        )
        return
    if event.type is EventType.SUBAGENT_FINISHED:
        usage = event.data.get("usage") or {}
        console.print(
            Text(
                f"  ◆ 子代理完成（token {usage.get('input_tokens', 0)}+"
                f"{usage.get('output_tokens', 0)}）",
                style="dim",
            )
        )


def _run_one_turn(runtime: AgentRuntime, user_message: str) -> RunResult:
    """Run one turn under ``asyncio.run`` with Ctrl+C wired to cancellation.

    When the user hits Ctrl+C, asyncio.run cancels the main task; the runtime
    finalizes the session as ``cancelled`` in its own CancelledError handler
    (persistence happens there), the inner guard prints a note and re-raises,
    and the resulting KeyboardInterrupt surfaces to the caller.
    """

    async def _guarded() -> RunResult:
        try:
            return await runtime.run_turn(user_message)
        except asyncio.CancelledError:
            typer.secho("已被用户取消，会话状态已保存。", fg=typer.colors.YELLOW)
            raise

    return asyncio.run(_guarded())


# ---------------------------------------------------------------------------
# Commands: run / chat
# ---------------------------------------------------------------------------


@app.command()
def run(
    task: Annotated[str, typer.Argument(help="要交给模型完成的任务描述")],
    workspace: WorkspaceOpt = Path("."),
    provider: ProviderOpt = ProviderChoice.auto,
    model: ModelOpt = None,
    script: ScriptOpt = None,
    max_rounds: MaxRoundsOpt = 20,
    max_tokens: MaxTokensOpt = 200_000,
    max_seconds: MaxSecondsOpt = 600.0,
    yes: YesOpt = False,
    acceptance: AcceptanceOpt = None,
    db: DbOpt = DEFAULT_DB_PATH,
) -> None:
    """执行一个单轮任务：流式展示回复、工具调用与修改摘要。"""
    setup = _prepare(
        workspace, provider, model, script, max_rounds, max_tokens, max_seconds, acceptance
    )
    console = Console()
    store = SqliteStore(db)
    try:
        runtime = _new_runtime(setup, store, _build_services(setup, store, console, yes))
        print_banner(
            console,
            provider_label=setup.provider_name,
            model_label=setup.model_label,
            workspace=str(setup.workspace.resolve()),
        )
        _print_header(console, task, setup)
        try:
            result = _run_one_turn(runtime, task)
        except (KeyboardInterrupt, asyncio.CancelledError):
            # Ctrl+C: the runtime already persisted the session as cancelled.
            raise typer.Exit(130) from None
        _print_turn_summary(console, result)
        _print_diff_summary(console, store, result.session_id)
    finally:
        store.close()


@app.command()
def chat(
    workspace: WorkspaceOpt = Path("."),
    provider: ProviderOpt = ProviderChoice.auto,
    model: ModelOpt = None,
    script: ScriptOpt = None,
    max_rounds: MaxRoundsOpt = 20,
    max_tokens: MaxTokensOpt = 200_000,
    max_seconds: MaxSecondsOpt = 600.0,
    yes: YesOpt = False,
    acceptance: AcceptanceOpt = None,
    db: DbOpt = DEFAULT_DB_PATH,
) -> None:
    """交互式多轮会话：/help 查看斜杠命令，exit / quit / Ctrl+D 退出。"""
    setup = _prepare(
        workspace, provider, model, script, max_rounds, max_tokens, max_seconds, acceptance
    )
    console = Console()
    store = SqliteStore(db)
    try:
        services = _build_services(setup, store, console, yes)
        _ChatRepl(setup=setup, store=store, console=console, services=services).loop()
    finally:
        store.close()


def _repl_help_text() -> str:
    """Slash-command help assembled from the shared registry."""
    lines = ["可用命令：", *format_command_lines()]
    lines.append("快捷键：Ctrl+C 取消当前回合 · Ctrl+D 退出")
    return "\n".join(lines)


class _ChatRepl:
    """Interactive chat loop with runtime slash commands (chat / resume).

    Slash commands are intercepted at the input layer and never sent to the
    model. ``/clear`` only clears the terminal rendering; the agent keeps
    its context. ``/new`` rebuilds the runtime so the next message starts a
    brand-new session.
    """

    def __init__(
        self,
        *,
        setup: _Setup,
        store: SqliteStore,
        console: Console,
        services: Any,
        initial_runtime: Any | None = None,
    ) -> None:
        self.setup = setup
        self.store = store
        self.console = console
        self.services = services
        self.runtime = initial_runtime if initial_runtime is not None else _new_runtime(
            setup, store, services
        )

    # -- loop -----------------------------------------------------------------

    def loop(self) -> None:
        print_banner(
            self.console,
            provider_label=self.setup.provider_name,
            model_label=self.runtime.model,
            workspace=str(self.setup.workspace.resolve()),
        )
        self.console.print(_repl_help_text())
        self.console.print()
        while True:
            try:
                user_input = self.console.input("[bold cyan]你 >[/] ")
            except EOFError:  # Ctrl+D
                self.console.print()
                break
            except KeyboardInterrupt:  # Ctrl+C at the prompt exits cleanly
                self.console.print()
                self.console.print("再见。")
                break
            text = user_input.strip()
            if not text:
                continue
            if text in {"exit", "quit"}:
                break
            if text.startswith("/"):
                if not self._handle_slash(text):
                    break
                continue
            try:
                result = _run_one_turn(self.runtime, text)
            except asyncio.CancelledError:  # pragma: no cover - defensive
                continue
            except KeyboardInterrupt:
                # Ctrl+C during a turn cancels that turn only; the session
                # was persisted and the REPL continues.
                self.console.print()
                continue
            _print_turn_summary(self.console, result)
            _print_diff_summary(self.console, self.store, result.session_id)

    # -- slash commands -------------------------------------------------------

    def _handle_slash(self, text: str) -> bool:
        """Dispatch one slash command; False means the REPL should exit."""
        verb, _, arg = text.partition(" ")
        verb = verb.lower()
        arg = arg.strip()
        if verb in ("/help", "/?"):
            self.console.print(_repl_help_text())
        elif verb in ("/exit", "/quit"):
            return False
        elif verb == "/clear":
            self._cmd_clear()
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
        elif verb == "/compact":
            self.console.print(Text("/compact 需要全屏界面，请使用 minicode tui。", style="yellow"))
        elif verb == "/resume":
            self.console.print(
                Text("请退出后使用 minicode resume <会话ID> 恢复会话。", style="yellow")
            )
        else:
            self.console.print(f"[yellow]未知命令 {verb}（/help 查看可用命令）[/]")
        return True

    def _cmd_clear(self) -> None:
        """Clear the terminal rendering only; the agent keeps its context.

        ANSI ``2J``/``3J`` clear the viewport and scrollback, ``H`` homes the
        cursor; Rich enables VT processing on Windows terminals, so this is
        portable without an extra dependency.
        """
        self.console.file.write("\033[2J\033[3J\033[H")
        self.console.file.flush()
        usage = self.runtime.usage
        self.console.print(
            Text(
                "已清屏（会话上下文与 Token 用量保留："
                f"输入 {usage.input_tokens} / 输出 {usage.output_tokens}）。",
                style="dim",
            )
        )

    def _cmd_new(self) -> None:
        """Reset the conversation: the next message starts a brand-new session."""
        self.runtime = _new_runtime(self.setup, self.store, self.services)
        self.console.print(
            "[green]已重置对话上下文，新会话将在下一条消息时创建。[/]"
        )

    def _cmd_model(self, arg: str) -> None:
        if not arg:
            lines = ["可用模型："]
            for info in MODEL_CATALOG.values():
                mark = " ← 当前" if info.name == self.runtime.model else ""
                effort = " · 支持推理预算" if info.supports_effort else ""
                lines.append(
                    f"  {info.name}（上下文 {info.context_window:,}{effort}）{mark}"
                )
            lines.append("用法: /model <名称>")
            self.console.print("\n".join(lines))
            return
        if arg not in MODEL_CATALOG:
            self.console.print(
                f"[yellow]未知模型 {arg}；可选：{'、'.join(MODEL_CATALOG)}[/]"
            )
            return
        provider = self._provider_for(MODEL_CATALOG[arg])
        if provider is None:
            return
        info = MODEL_CATALOG[arg]
        old = self.runtime.model
        # Keep /new consistent with the switched model.
        self.setup.provider = provider
        self.setup.provider_name = info.provider
        self.setup.model_label = info.name
        self.runtime.set_model(
            provider=provider, provider_name=info.provider, model=info.name
        )
        self.console.print(
            f"[green]已切换模型[/] {old} → {info.name}"
            f"（上下文 {info.context_window:,} token）"
        )

    def _provider_for(self, info: Any) -> Provider | None:
        """Build a provider for a catalog entry, surfacing failures inline."""
        try:
            return _provider_for_model(
                info.name, carry_effort_from=self.runtime.provider
            )
        except (ProviderRequestError, RuntimeError) as exc:
            self.console.print(f"[red]无法切换 provider：{exc}[/]")
            return None

    def _cmd_effort(self, arg: str) -> None:
        provider = self.runtime.provider
        current = getattr(provider, "reasoning_effort", None)
        if not arg:
            label = current if current else "默认（跟随网关）"
            self.console.print(
                f"当前推理预算: {label}；可选: {' / '.join(EFFORT_LEVELS)}"
            )
            return
        level = parse_effort(arg)
        if level is None:
            self.console.print(
                f"[yellow]无效档位 {arg}；可选: {' / '.join(EFFORT_LEVELS)}[/]"
            )
            return
        if not hasattr(provider, "reasoning_effort"):
            self.console.print(
                f"[yellow]当前 provider（{getattr(provider, 'name', '?')}）"
                "不支持推理预算调整。[/]"
            )
            return
        provider.reasoning_effort = None if level == "off" else level
        shown = level if level != "off" else "off（跟随网关默认）"
        self.console.print(f"[green]推理预算已调整为 {shown}[/]")

    def _cmd_permissions(self, arg: str) -> None:
        policy = self.services.policy
        if not isinstance(policy, ModePolicy):
            self.console.print("[yellow]当前会话的权限策略不支持运行时切换。[/]")
            return
        if not arg:
            self.console.print(
                f"当前权限模式: {policy.mode.value}"
                f"（{PERMISSION_MODE_LABELS[policy.mode]}）"
            )
            for line in permission_mode_lines():
                self.console.print(line)
            return
        mode = parse_permission_mode(arg)
        if mode is None:
            options = " / ".join(m.value for m in PermissionMode)
            self.console.print(f"[yellow]无效模式 {arg}；可选: {options}[/]")
            return
        previous = policy.set_mode(mode)
        self.console.print(
            f"[green]权限模式已切换[/] {previous.value} → {mode.value}"
            f"（{PERMISSION_MODE_LABELS[mode]}）"
        )

    def _cmd_sessions(self) -> None:
        sessions = self.store.list_sessions()
        if not sessions:
            self.console.print("暂无会话记录。")
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
        self.console.print("\n".join(lines))


@app.command()
def resume(
    session_id: Annotated[str, typer.Argument(help="要恢复的会话 ID（可先用 sessions list 查看）")],
    workspace: WorkspaceOpt | None = None,
    provider: ProviderOpt = ProviderChoice.auto,
    model: ModelOpt = None,
    script: ScriptOpt = None,
    max_rounds: MaxRoundsOpt = 30,
    max_tokens: MaxTokensOpt = 400_000,
    max_seconds: MaxSecondsOpt = 600.0,
    yes: YesOpt = False,
    acceptance: AcceptanceOpt = None,
    db: DbOpt = DEFAULT_DB_PATH,
) -> None:
    """恢复历史会话并继续交互：已完成的结果不重复执行，未知副作用先核实。"""
    from minicode.runtime import AgentRuntime

    console = Console()
    store = SqliteStore(db)
    try:
        resolved = _resolve_session_id(store, session_id)
        if resolved is None:
            _fail(f"未找到会话: {session_id}（可用 minicode sessions list 查看会话 ID）")
        assert resolved is not None
        summary = store.get_session(resolved)
        assert summary is not None
        if workspace is not None:
            ws = Path(workspace).expanduser()
            if not ws.is_dir():
                _fail(f"工作区不存在: {ws}")
        else:
            ws = Path(summary.workspace)

        setup = _prepare(
            ws, provider, model, script, max_rounds, max_tokens, max_seconds, acceptance
        )
        services = _build_services(setup, store, console, yes)
        runtime = _make_resumed_runtime(setup, store, summary, services)
        console.print(f"[bold]恢复会话[/] {summary.session_id[:8]} ·"
                      f" 轮数 {summary.rounds} ·"
                      f" Token {summary.input_tokens + summary.output_tokens}")
        _ChatRepl(
            setup=setup,
            store=store,
            console=console,
            services=services,
            initial_runtime=runtime,
        ).loop()
    finally:
        store.close()


def _make_resumed_runtime(
    setup: _Setup, store: SqliteStore, summary: Any, services: Any
) -> Any:
    """Build the runtime for ``resume`` from shared services and validate
    the stored session against it."""
    try:
        runtime = AgentRuntime.resume(
            store=store,
            session_id=summary.session_id,
            provider=setup.provider,
            registry=services.registry,
            policy=services.policy,
            workspace=setup.workspace,
            budget=setup.budget,
            approval_handler=services.approval_handler,
            on_text_delta=services.on_text_delta,
            on_event=services.on_event,
            compactor=None,  # attached below, bound to this runtime
            goal_checker=services.goal_checker,
            evidence_ledger=services.evidence_ledger,
            background_manager=services.background_manager,
            artifact_store=services.artifact_store,
        )
    except ValueError as exc:
        _fail(str(exc))
    _attach_compactor(runtime, services.artifact_store, setup.budget.max_total_tokens)
    return runtime


# ---------------------------------------------------------------------------
# Commands: sessions list / report
# ---------------------------------------------------------------------------


@sessions_app.command("list")
def sessions_list(db: DbOpt = DEFAULT_DB_PATH) -> None:
    """列出所有会话（最新在前）。"""
    console = Console()
    store = SqliteStore(db)
    try:
        sessions = store.list_sessions()
    finally:
        store.close()
    if not sessions:
        console.print("暂无会话记录。")
        return
    table = Table(title="会话列表")
    table.add_column("会话", style="cyan", no_wrap=True)
    table.add_column("创建时间", no_wrap=True)
    table.add_column("工作区")
    table.add_column("模型", no_wrap=True)
    table.add_column("状态", no_wrap=True)
    table.add_column("轮数", justify="right")
    table.add_column("Token", justify="right")
    for session in sessions:
        table.add_row(
            session.session_id[:8],
            _format_timestamp(session.created_at),
            session.workspace,
            f"{session.provider}/{session.model}",
            _status_label(session.status, session.exit_reason),
            str(session.rounds),
            str(session.input_tokens + session.output_tokens),
        )
    console.print(table)


def _resolve_session_id(store: SqliteStore, session_id: str) -> str | None:
    """Exact match first, then unique prefix match (the list shows 8 chars)."""
    if store.get_session(session_id) is not None:
        return session_id
    matches = [s for s in store.list_sessions() if s.session_id.startswith(session_id)]
    if len(matches) == 1:
        return matches[0].session_id
    return None


def _full_tool_outputs(store: SqliteStore, session_id: str) -> dict[str, str]:
    """Map tool_use_id -> full ToolResultBlock content from persisted messages."""
    outputs: dict[str, str] = {}
    for message in store.get_messages(session_id):
        for block in message.content:
            if isinstance(block, ToolResultBlock):
                outputs.setdefault(block.tool_use_id, block.content)
    return outputs


@app.command()
def report(
    session_id: Annotated[str, typer.Argument(help="会话 ID（可先用 sessions list 查看）")],
    full: Annotated[bool, typer.Option(help="打印完整工具输出，而不是 500 字符预览")] = False,
    format: Annotated[str, typer.Option(case_sensitive=False, help="输出格式：text 或 html")] = "text",
    output: Annotated[Path | None, typer.Option("--output", "-o", help="HTML 输出路径（缺省 ./minicode-report-<id>.html）")] = None,
    db: DbOpt = DEFAULT_DB_PATH,
) -> None:
    """打印一个会话的执行报告（text），或生成可离线查看的 HTML 报告。"""
    console = Console()
    store = SqliteStore(db)
    try:
        resolved = _resolve_session_id(store, session_id)
        if resolved is None:
            _fail(f"未找到会话: {session_id}（可用 minicode sessions list 查看会话 ID）")
        assert resolved is not None  # narrowed by _fail above
        summary = store.get_session(resolved)
        assert summary is not None
        events = store.get_events(resolved)
        messages = store.get_messages(resolved)
        full_outputs = _full_tool_outputs(store, resolved)
    finally:
        store.close()

    if format.lower() == "html":
        from minicode.reports import render_session_html

        target = output if output is not None else Path(f"minicode-report-{resolved[:8]}.html")
        html = render_session_html(summary, events, messages, full_outputs)
        target.write_text(html, encoding="utf-8")
        console.print(f"HTML 报告已生成: {target.resolve()}")
        return
    if format.lower() != "text":
        _fail(f"未知格式: {format}（可选 text / html）")
    _render_report(console, summary, events, full_outputs, full)


def _render_report(
    console: Console,
    summary: SessionSummary,
    events: list[Event],
    full_outputs: dict[str, str],
    full: bool,
) -> None:
    console.print(f"[bold]会话[/] {summary.session_id}")
    console.print(f"工作区: {summary.workspace}")
    console.print(
        f"模型: {summary.provider}/{summary.model}"
        f" · 创建: {_format_timestamp(summary.created_at)}"
    )
    console.print(
        f"状态: {_status_label(summary.status, summary.exit_reason)}"
        f" · 轮数: {summary.rounds}"
        f" · Token: {summary.input_tokens + summary.output_tokens}"
        f"（输入 {summary.input_tokens} / 输出 {summary.output_tokens}）"
    )

    counts: Counter[EventType] = Counter(event.type for event in events)
    parts = [f"{etype.value}={counts[etype]}" for etype in EventType if counts[etype]]
    console.print()
    console.print("事件统计: " + (" ".join(parts) if parts else "（无事件）"))

    tool_results = [
        event for event in events if event.type is EventType.TOOL_CALL_RESULT
    ]
    if tool_results:
        console.print()
        console.print("[bold]工具调用[/]")
        for event in tool_results:
            data = event.data
            name = str(data.get("name", "?"))
            ok = bool(data.get("success"))
            exit_code = data.get("exit_code")
            exit_label = "-" if exit_code is None else str(exit_code)
            line = Text("▸ ")
            line.append(name, style="cyan")
            line.append("  成功" if ok else "  失败", style="green" if ok else "red")
            line.append(f"  退出码: {exit_label}")
            if data.get("error"):
                line.append(f"  错误: {data['error']}", style="red")
            console.print(line)
            call_id = str(data.get("call_id", ""))
            if (name in ("edit", "write") or full) and call_id in full_outputs:
                output = full_outputs[call_id]
            else:
                output = str(data.get("output_preview") or "")
            if output:
                console.print(
                    Panel(Text(output), border_style="green" if ok else "red")
                )

    console.print()
    console.print(
        f"总计: 输入 {summary.input_tokens} token · 输出 {summary.output_tokens} token"
    )
    final = summary.exit_reason or summary.status
    console.print(f"退出原因: {_exit_label(final)} ({final})")


@app.command("eval")
def eval_cmd(
    tasks_dir: Annotated[Path, typer.Option("--tasks-dir", help="任务集目录（默认 evals/tasks）")] = Path("evals/tasks"),
    task: Annotated[list[str] | None, typer.Option("--task", help="只跑指定任务 ID（可重复）")] = None,
    baselines: Annotated[str, typer.Option(help="基线组合：b0,b1,b2")] = "b0,b2",
    output: Annotated[Path, typer.Option("--output", "-o", help="结果输出目录")] = Path("reports/eval"),
) -> None:
    """运行本地评测集（FakeProvider 离线跑通三基线对照）。"""
    runner = _find_eval_runner()
    if runner is None:
        _fail("找不到 evals/run_eval.py（请在仓库根目录运行，或先安装完整仓库）。")
    cmd = [
        sys.executable, str(runner),
        "--tasks-dir", str(tasks_dir),
        "--baselines", baselines,
        "--output", str(output),
    ]
    for task_id in task or []:
        cmd += ["--task", task_id]
    completed = subprocess.run(cmd)
    if completed.returncode != 0:
        _fail("评测运行失败，详见上方输出。")


def _find_eval_runner() -> Path | None:
    """Locate evals/run_eval.py relative to cwd or the package root."""
    for base in (Path.cwd(), Path(__file__).resolve().parent.parent.parent):
        candidate = base / "evals" / "run_eval.py"
        if candidate.is_file():
            return candidate
    return None


@app.command()
def tui(
    workspace: WorkspaceOpt = Path("."),
    provider: ProviderOpt = ProviderChoice.auto,
    model: ModelOpt = None,
    script: ScriptOpt = None,
    max_rounds: MaxRoundsOpt = 20,
    max_tokens: MaxTokensOpt = 200_000,
    max_seconds: MaxSecondsOpt = 600.0,
    yes: YesOpt = False,
    acceptance: AcceptanceOpt = None,
    db: DbOpt = DEFAULT_DB_PATH,
) -> None:
    """全屏交互界面（Textual）：流式回复、工具卡片、审批弹窗、斜杠命令。"""
    try:
        from minicode.ui.app import run_tui
    except ImportError as exc:
        _fail(f"TUI 依赖未安装：pip install 'minicode' 后重试。({exc})")
    setup = _prepare(
        workspace, provider, model, script, max_rounds, max_tokens, max_seconds, acceptance
    )
    run_tui(setup=setup, db_path=db, yes=yes)


def main() -> None:
    """Console-script entry point (pyproject: minicode = minicode.cli:main)."""
    app()


if __name__ == "__main__":
    main()
