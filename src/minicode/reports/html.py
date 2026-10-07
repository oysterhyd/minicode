"""Single-file offline HTML execution report for minicode sessions.

:func:`render_session_html` turns everything persisted about one session —
the :class:`~minicode.storage.SessionSummary` row, the append-only event
trace and the stored conversation messages — into one self-contained HTML
page: inline CSS only, dark theme, no JavaScript required (collapsible
blocks use native ``<details>`` elements) and zero external resources, so
the report can be double-clicked open offline in any browser.

Security posture: tool outputs, tool arguments, paths — none of these are
trusted. Every dynamic value passes through :func:`html.escape` (with
attribute quoting), so injected markup such as ``<script>`` survives only
as escaped entities. Event payloads are read defensively (``dict.get``
style access throughout): a partially-written or future-format event must
never crash the renderer.
"""

from __future__ import annotations

import html
import json

from minicode.core.models import Event, EventType, Message, ToolResultBlock
from minicode.storage import SessionSummary

__all__ = ["render_session_html"]

# ---------------------------------------------------------------------------
# Tuning constants
# ---------------------------------------------------------------------------

#: Tool-argument JSON longer than this collapses behind a <details>.
_ARG_SUMMARY_CHARS = 200

#: Raw event-data fallback dumps longer than this are truncated.
_GENERIC_DUMP_CHARS = 1200

#: Background-job output previews are capped at this length.
_OUTPUT_PREVIEW_CAP = 400

# ---------------------------------------------------------------------------
# Label tables
# ---------------------------------------------------------------------------

#: Exit reason (or terminal status) -> Chinese label.
_EXIT_REASON_LABELS: dict[str, str] = {
    "completed": "已完成",
    "max_tokens": "模型输出被截断",
    "max_rounds": "达到最大轮数",
    "token_budget": "Token 预算耗尽",
    "time_budget": "时长预算耗尽",
    "context_limit": "上下文超过模型窗口",
    "cancelled": "已取消",
    "goal_not_met": "验收未通过",
    "provider_error": "模型调用失败",
    "internal_error": "内部错误",
    "stalled": "重复工具循环",
}

#: Non-terminal status labels (status is free-form in the store).
_STATUS_LABELS: dict[str, str] = {
    "running": "运行中",
    "paused": "已暂停，可继续",
}

#: EventType -> (badge label, badge color class suffix).
_EVENT_META: dict[EventType, tuple[str, str]] = {
    EventType.SESSION_START: ("会话开始", "start"),
    EventType.BUDGET_CHANGED: ("预算更新", "round"),
    EventType.ROUND_START: ("轮开始", "round"),
    EventType.ASSISTANT_MESSAGE: ("助手消息", "assistant"),
    EventType.TOOL_CALL_START: ("工具调用", "tool"),
    EventType.TOOL_CALL_RESULT: ("工具结果", "tool"),
    EventType.TOOL_OUTPUT: ("工具实时输出", "tool"),
    EventType.APPROVAL_REQUEST: ("审批请求", "approval"),
    EventType.APPROVAL_DECISION: ("审批决定", "approval"),
    EventType.ROUND_END: ("轮结束", "round"),
    EventType.SESSION_END: ("会话结束", "end"),
    EventType.CONTEXT_COMPACTED: ("上下文压缩", "compact"),
    EventType.GOAL_CHECK: ("目标验收", "goal"),
    EventType.SIDE_EFFECT_UNKNOWN: ("副作用未知", "warn"),
    EventType.BACKGROUND_JOB_STARTED: ("后台任务启动", "bg"),
    EventType.BACKGROUND_JOB_COMPLETED: ("后台任务完成", "bg"),
    EventType.BACKGROUND_JOB_LOST: ("后台任务丢失", "warn"),
    EventType.PROJECT_INSTRUCTIONS: ("项目指令", "compact"),
    EventType.SKILL_ACTIVATED: ("技能激活", "compact"),
    EventType.SKILL_DEACTIVATED: ("技能停用", "compact"),
    EventType.SUBAGENT_START: ("子任务启动", "bg"),
    EventType.SUBAGENT_RESULT: ("子任务结果", "bg"),
    EventType.MCP_DISCOVERY: ("MCP 发现", "bg"),
    EventType.PROVIDER_RETRY: ("模型请求重试", "warn"),
}

# ---------------------------------------------------------------------------
# Inline stylesheet (dark theme, zero external dependencies)
# ---------------------------------------------------------------------------

_CSS = """
:root{--bg:#0d1117;--panel:#161b22;--deep:#010409;--border:#30363d;
--text:#e6edf3;--muted:#8b949e;--accent:#58a6ff;--green:#3fb950;
--red:#f85149;--yellow:#d29922;--purple:#bc8cff;--cyan:#39c5cf;}
*{box-sizing:border-box;}
body{margin:0;background:var(--bg);color:var(--text);
font:14px/1.6 -apple-system,"Segoe UI",Roboto,"Helvetica Neue",
"PingFang SC","Microsoft YaHei",sans-serif;}
.wrap{max-width:980px;margin:0 auto;padding:24px 16px 48px;}
h1{font-size:20px;margin:0 0 12px;}
h2{font-size:16px;margin:0 0 10px;color:var(--accent);}
h3{font-size:14px;margin:12px 0 6px;color:var(--text);}
.panel{background:var(--panel);border:1px solid var(--border);
border-radius:8px;padding:14px 16px;margin:0 0 16px;}
table{border-collapse:collapse;width:100%;}
th,td{text-align:left;padding:4px 12px 4px 0;vertical-align:top;}
.meta th{color:var(--muted);font-weight:400;white-space:nowrap;width:90px;}
code{font-family:ui-monospace,SFMono-Regular,Consolas,"Cascadia Mono",
"Courier New",monospace;font-size:12.5px;background:var(--deep);
border:1px solid var(--border);border-radius:4px;padding:1px 5px;
word-break:break-all;}
.mono{font-family:ui-monospace,SFMono-Regular,Consolas,"Cascadia Mono",
"Courier New",monospace;font-size:12.5px;word-break:break-all;}
.muted{color:var(--muted);}
.badge{display:inline-block;border-radius:10px;padding:1px 9px;
font-size:12px;font-weight:600;border:1px solid;white-space:nowrap;}
.b-start{color:var(--accent);border-color:var(--accent);}
.b-round{color:var(--muted);border-color:var(--border);}
.b-assistant{color:var(--purple);border-color:var(--purple);}
.b-tool{color:var(--accent);border-color:var(--accent);}
.b-ok{color:var(--green);border-color:var(--green);}
.b-fail{color:var(--red);border-color:var(--red);}
.b-approval{color:var(--yellow);border-color:var(--yellow);}
.b-end{color:var(--purple);border-color:var(--purple);}
.b-compact{color:var(--cyan);border-color:var(--cyan);}
.b-goal{color:var(--green);border-color:var(--green);}
.b-warn{color:var(--yellow);border-color:var(--yellow);}
.b-bg{color:var(--cyan);border-color:var(--cyan);}
.timeline{display:flex;flex-direction:column;gap:8px;}
.event{background:var(--panel);border:1px solid var(--border);
border-left:3px solid var(--border);border-radius:6px;padding:8px 12px;}
.event.ok{border-left-color:var(--green);}
.event.fail{border-left-color:var(--red);}
.ev-head{display:flex;align-items:baseline;gap:10px;flex-wrap:wrap;}
.ev-type{color:var(--muted);background:none;border:none;padding:0;}
.ev-time{color:var(--muted);font-size:12px;}
.ev-body{margin-top:6px;}
.kv{display:flex;gap:8px;flex-wrap:wrap;align-items:baseline;}
.kv .k{color:var(--muted);min-width:60px;}
.pass{color:var(--green);font-weight:600;}
.notpass{color:var(--red);font-weight:600;}
.err{color:var(--red);}
.msg-text{white-space:pre-wrap;word-break:break-word;}
pre{background:var(--deep);border:1px solid var(--border);
border-radius:6px;padding:8px 10px;overflow-x:auto;white-space:pre;
margin:6px 0;font-family:ui-monospace,SFMono-Regular,Consolas,
"Cascadia Mono","Courier New",monospace;font-size:12.5px;}
details{margin-top:6px;}
summary{cursor:pointer;color:var(--accent);}
summary:hover{text-decoration:underline;}
.d-add{color:var(--green);}
.d-del{color:var(--red);}
.d-hunk{color:var(--cyan);}
.d-meta{color:var(--muted);font-weight:600;}
.goal-table th{color:var(--muted);font-weight:400;
border-bottom:1px solid var(--border);}
.goal-table td{border-bottom:1px solid var(--border);}
"""


# ---------------------------------------------------------------------------
# Small escaping / formatting helpers
# ---------------------------------------------------------------------------


def _esc(value: object) -> str:
    """Escape any dynamic value for safe interpolation into HTML.

    ``quote=True`` also escapes quotes so values interpolated into
    attributes cannot break out of them.
    """
    return html.escape(str(value), quote=True)


def _fmt_ts(ts: str) -> str:
    """Truncate an ISO-8601 timestamp to ``YYYY-MM-DD HH:MM:SS``.

    Purely textual (no datetime parsing) so odd shapes degrade gracefully.
    """
    if not isinstance(ts, str):
        return ""
    return ts.replace("T", " ", 1)[:19]


def _first(data: dict, *keys: str) -> object:
    """Return the first present, non-None value among ``keys`` (defensive)."""
    for key in keys:
        value = data.get(key)
        if value is not None:
            return value
    return None


def _clip(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[:limit] + "…"


def _json_text(value: object, indent: int | None = None) -> str:
    """Best-effort JSON rendering; never raises."""
    try:
        return json.dumps(value, ensure_ascii=False, default=str, indent=indent)
    except (TypeError, ValueError):
        return str(value)


def _kv(key: str, value_html: str) -> str:
    return (
        '<div class="kv"><span class="k">'
        f"{_esc(key)}</span><span>{value_html}</span></div>"
    )


def _generic_data_html(data: dict) -> str:
    """Fallback body: escaped pretty-printed JSON of the raw event data."""
    if not data:
        return ""
    text = _clip(_json_text(data, indent=2), _GENERIC_DUMP_CHARS)
    return f'<pre class="raw">{_esc(text)}</pre>'


def _pass_mark(passed: object) -> str:
    """Colored pass/fail marker for goal-check items."""
    if passed is None:
        return '<span class="muted">未知</span>'
    if passed:
        return '<span class="pass">✓ 通过</span>'
    return '<span class="notpass">✗ 失败</span>'


def _looks_like_diff(content: str) -> bool:
    """Heuristic: unified-diff or git-diff shaped text."""
    head = content.splitlines()[:50]
    has_minus = any(line.startswith("--- ") for line in head)
    has_plus = any(line.startswith("+++ ") for line in head)
    return (has_minus and has_plus) or any(
        line.startswith("diff --git") for line in head
    )


def _diff_pre(content: str) -> str:
    """Render diff text with simple line coloring (+ green, - red)."""
    out: list[str] = []
    for line in content.split("\n"):
        if line.startswith("@@"):
            cls = "d-hunk"
        elif line.startswith("+++") or line.startswith("---"):
            cls = "d-meta"
        elif line.startswith("+"):
            cls = "d-add"
        elif line.startswith("-"):
            cls = "d-del"
        else:
            cls = ""
        body = _esc(line)
        out.append(f'<span class="{cls}">{body}</span>' if cls else body)
    return "\n".join(out)


# ---------------------------------------------------------------------------
# Message / event cross-referencing
# ---------------------------------------------------------------------------


def _tool_result_contents(messages: list[Message]) -> dict[str, str]:
    """Map ``tool_use_id -> content`` over every ToolResultBlock stored in
    the conversation — the fallback source for full edit/write diffs when
    ``full_outputs`` does not carry the call."""
    by_id: dict[str, str] = {}
    for message in messages:
        for block in message.content:
            if isinstance(block, ToolResultBlock) and block.content:
                by_id.setdefault(block.tool_use_id, block.content)
    return by_id


def _call_names(events: list[Event]) -> dict[str, str]:
    """Map ``call_id -> tool name`` from tool_call_start/result events."""
    names: dict[str, str] = {}
    for event in events:
        data = event.data if isinstance(event.data, dict) else {}
        call_id = data.get("call_id")
        name = data.get("name")
        if isinstance(call_id, str) and isinstance(name, str) and name:
            names.setdefault(call_id, name)
    return names


# ---------------------------------------------------------------------------
# Per-event body renderers (each returns "" when nothing known is present;
# the dispatcher then falls back to the generic JSON dump)
# ---------------------------------------------------------------------------


def _session_start_body(data: dict) -> str:
    rows = []
    workspace = _first(data, "workspace")
    if workspace:
        rows.append(_kv("工作区", f"<code>{_esc(workspace)}</code>"))
    provider = _first(data, "provider")
    if provider:
        rows.append(_kv("提供商", _esc(provider)))
    model = _first(data, "model")
    if model:
        rows.append(_kv("模型", _esc(model)))
    return "\n".join(rows)


def _round_body(data: dict) -> str:
    rnd = _first(data, "round")
    if rnd is None:
        return ""
    return _kv("轮次", f"第 {_esc(rnd)} 轮")


def _assistant_body(data: dict) -> str:
    rows = []
    text = data.get("text")
    if isinstance(text, str) and text:
        rows.append(f'<div class="msg-text">{_esc(text)}</div>')
    calls = data.get("tool_calls")
    if isinstance(calls, list) and calls:
        chips = "".join(f"<code>{_esc(call)}</code>" for call in calls)
        rows.append(_kv("工具调用", chips))
    usage = data.get("usage")
    if isinstance(usage, dict):
        if usage.get("available") is False:
            usage_text = "未知（provider 未返回 usage）"
        else:
            usage_text = (
                f"输入 {_esc(usage.get('input_tokens', 0))}"
                f" · 输出 {_esc(usage.get('output_tokens', 0))}"
                f" · 缓存读 {_esc(usage.get('cache_read_tokens', 0))}"
                f" · 缓存写 {_esc(usage.get('cache_write_tokens', 0))}"
            )
        rows.append(
            _kv("Token", usage_text)
        )
    stop = data.get("stop_reason")
    if stop:
        rows.append(_kv("停止原因", _esc(stop)))
    return "\n".join(rows)


def _tool_start_body(data: dict) -> str:
    rows = []
    name = _first(data, "name")
    if name:
        rows.append(_kv("工具", f"<code>{_esc(name)}</code>"))
    source = data.get("source")
    if isinstance(source, dict) and source.get("plugin"):
        rows.append(_kv("来源", f"<code>{_esc(source.get('plugin'))}@{_esc(source.get('plugin_version'))}"
                        f" sha256:{_esc(str(source.get('plugin_sha256', ''))[:12])}</code>"))
    args = data.get("arguments")
    if args is None:
        args = {}
    args_json = _json_text(args)
    if args_json and args_json != "{}":
        short = _clip(args_json, _ARG_SUMMARY_CHARS)
        rows.append(_kv("参数", f"<code>{_esc(short)}</code>"))
        if len(args_json) > _ARG_SUMMARY_CHARS:
            pretty = _json_text(args, indent=2)
            rows.append(
                "<details><summary>展开完整参数</summary>"
                f"<pre>{_esc(pretty)}</pre></details>"
            )
    return "\n".join(rows)


def _tool_result_body(
    data: dict,
    full_outputs: dict[str, str],
    results_by_id: dict[str, str],
    call_names: dict[str, str],
) -> str:
    rows = []
    ok = True if data.get("success") is None else bool(data.get("success"))
    rows.append(
        _kv(
            "结果",
            '<span class="pass">✓ 成功</span>'
            if ok
            else '<span class="notpass">✗ 失败</span>',
        )
    )
    call_id = data.get("call_id")
    call_id = call_id if isinstance(call_id, str) else None
    name = _first(data, "name")
    if not name and call_id is not None:
        name = call_names.get(call_id) or ""
    if name:
        rows.append(_kv("工具", f"<code>{_esc(name)}</code>"))
    source = data.get("source")
    if isinstance(source, dict) and source.get("plugin"):
        rows.append(_kv("来源", f"<code>{_esc(source.get('plugin'))}@{_esc(source.get('plugin_version'))}"
                        f" sha256:{_esc(str(source.get('plugin_sha256', ''))[:12])}</code>"))
    exit_code = data.get("exit_code")
    if exit_code is not None:
        rows.append(_kv("退出码", f"<code>{_esc(exit_code)}</code>"))
    error = data.get("error")
    if isinstance(error, str) and error:
        rows.append(_kv("错误", f'<code class="err">{_esc(error)}</code>'))
    preview = data.get("output_preview")
    if isinstance(preview, str) and preview:
        rows.append(f'<pre class="preview">{_esc(preview)}</pre>')

    # Full output: explicit full_outputs first; for edit/write fall back to
    # the ToolResultBlock persisted in the conversation messages.
    full: object = None
    if call_id is not None:
        candidate = full_outputs.get(call_id)
        if isinstance(candidate, str) and candidate:
            full = candidate
    if not full and name in ("edit", "write") and call_id is not None:
        full = results_by_id.get(call_id)
    if isinstance(full, str) and full and full.strip() != str(preview or "").strip():
        is_diff = name in ("edit", "write") or _looks_like_diff(full)
        label = "展开完整 diff" if is_diff else "展开完整输出"
        pre_cls = "diff" if is_diff else "full"
        inner = _diff_pre(full) if is_diff else _esc(full)
        rows.append(
            f"<details><summary>{_esc(label)}</summary>"
            f'<pre class="{pre_cls}">{inner}</pre></details>'
        )
    return "\n".join(rows)


def _approval_request_body(data: dict) -> str:
    rows = []
    tool = _first(data, "tool_name", "tool")
    if tool:
        rows.append(_kv("工具", f"<code>{_esc(tool)}</code>"))
    summary = _first(data, "summary")
    if summary:
        rows.append(_kv("内容", _esc(summary)))
    return "\n".join(rows)


def _approval_decision_body(data: dict) -> str:
    rows = []
    granted = bool(data.get("granted"))
    rows.append(
        _kv(
            "决定",
            '<span class="pass">✓ 允许</span>'
            if granted
            else '<span class="notpass">✗ 拒绝</span>',
        )
    )
    tool = _first(data, "tool_name", "tool")
    if tool:
        rows.append(_kv("工具", f"<code>{_esc(tool)}</code>"))
    reason = data.get("reason")
    if isinstance(reason, str) and reason:
        rows.append(_kv("原因", _esc(reason)))
    return "\n".join(rows)


def _session_end_body(data: dict) -> str:
    rows = []
    exit_reason = _first(data, "exit_reason")
    if exit_reason:
        label = _EXIT_REASON_LABELS.get(str(exit_reason), str(exit_reason))
        rows.append(
            _kv("退出原因", f"{_esc(label)} <code>{_esc(exit_reason)}</code>")
        )
    rounds = data.get("rounds")
    if rounds is not None:
        rows.append(_kv("轮数", _esc(rounds)))
    usage = data.get("total_usage")
    if isinstance(usage, dict):
        rows.append(
            _kv(
                "Token",
                f"输入 {_esc(usage.get('input_tokens', 0))}"
                f" · 输出 {_esc(usage.get('output_tokens', 0))}"
                f" · 缓存读 {_esc(usage.get('cache_read_tokens', 0))}"
                f" · 缓存写 {_esc(usage.get('cache_write_tokens', 0))}"
                f"{' · 用量不完整' if usage.get('available') is False else ''}",
            )
        )
    error = data.get("error")
    if isinstance(error, str) and error:
        rows.append(_kv("错误", f'<code class="err">{_esc(error)}</code>'))
    return "\n".join(rows)


def _context_compacted_body(data: dict) -> str:
    before = _first(data, "before_tokens", "tokens_before", "before")
    after = _first(data, "after_tokens", "tokens_after", "after")
    if before is None and after is None:
        return ""
    return _kv("Token", f"{_esc(before)} → {_esc(after)}")


def _goal_items(data: dict) -> list[dict]:
    """Extract goal-check items from an event payload (tolerates both the
    ``items: [...]`` list shape and a single flat check)."""
    items = data.get("items")
    if isinstance(items, list) and items:
        return [item if isinstance(item, dict) else {"item": item} for item in items]
    if any(
        key in data for key in ("item", "name", "check", "description", "passed", "ok", "success")
    ):
        return [data]
    return []


def _goal_check_body(data: dict) -> str:
    rows = []
    for item in _goal_items(data):
        text = _first(item, "item", "name", "check", "description")
        kind = _first(item, "kind")
        passed = _first(item, "passed", "ok", "success")
        exit_code = item.get("exit_code")
        bits = [_pass_mark(passed)]
        if text:
            bits.append(f'<span class="mono">{_esc(text)}</span>')
        if kind:
            bits.append(f'<span class="muted">kind={_esc(kind)}</span>')
        if exit_code is not None:
            bits.append(f"退出码 <code>{_esc(exit_code)}</code>")
        rows.append('<div class="kv">' + " ".join(bits) + "</div>")
    return "\n".join(rows)


def _side_effect_body(data: dict) -> str:
    rows = []
    tool = _first(data, "tool", "tool_name", "name")
    if tool:
        rows.append(_kv("工具", f"<code>{_esc(tool)}</code>"))
    detail = _first(data, "detail", "description", "reason", "error")
    if detail:
        rows.append(_kv("说明", _esc(detail)))
    rows.append(_kv("状态", '<span class="muted">副作用结果未知</span>'))
    return "\n".join(rows)


def _background_started_body(data: dict) -> str:
    rows = []
    job = _first(data, "job_id", "job", "id")
    if job:
        rows.append(_kv("任务", f"<code>{_esc(job)}</code>"))
    command = _first(data, "command", "cmd", "name", "description")
    if isinstance(command, str) and command:
        rows.append(_kv("内容", _esc(_clip(command, _ARG_SUMMARY_CHARS))))
    return "\n".join(rows)


def _background_completed_body(data: dict) -> str:
    rows = []
    job = _first(data, "job_id", "job", "id")
    if job:
        rows.append(_kv("任务", f"<code>{_esc(job)}</code>"))
    exit_code = data.get("exit_code")
    if exit_code is not None:
        rows.append(_kv("退出码", f"<code>{_esc(exit_code)}</code>"))
    output = _first(data, "output", "output_preview")
    if isinstance(output, str) and output:
        rows.append(f'<pre class="preview">{_esc(_clip(output, _OUTPUT_PREVIEW_CAP))}</pre>')
    return "\n".join(rows)


def _background_lost_body(data: dict) -> str:
    job = _first(data, "job_id", "job", "id")
    if not job:
        return ""
    return _kv("任务", f"<code>{_esc(job)}</code> 结果丢失")


def _event_body(
    event: Event,
    data: dict,
    full_outputs: dict[str, str],
    results_by_id: dict[str, str],
    call_names: dict[str, str],
) -> str:
    kind = event.type
    if kind is EventType.SESSION_START:
        body = _session_start_body(data)
    elif kind is EventType.ROUND_START or kind is EventType.ROUND_END:
        body = _round_body(data)
    elif kind is EventType.ASSISTANT_MESSAGE:
        body = _assistant_body(data)
    elif kind is EventType.TOOL_CALL_START:
        body = _tool_start_body(data)
    elif kind is EventType.TOOL_CALL_RESULT:
        body = _tool_result_body(data, full_outputs, results_by_id, call_names)
    elif kind is EventType.APPROVAL_REQUEST:
        body = _approval_request_body(data)
    elif kind is EventType.APPROVAL_DECISION:
        body = _approval_decision_body(data)
    elif kind is EventType.SESSION_END:
        body = _session_end_body(data)
    elif kind is EventType.CONTEXT_COMPACTED:
        body = _context_compacted_body(data)
    elif kind is EventType.GOAL_CHECK:
        body = _goal_check_body(data)
    elif kind is EventType.SIDE_EFFECT_UNKNOWN:
        body = _side_effect_body(data)
    elif kind is EventType.BACKGROUND_JOB_STARTED:
        body = _background_started_body(data)
    elif kind is EventType.BACKGROUND_JOB_COMPLETED:
        body = _background_completed_body(data)
    elif kind is EventType.BACKGROUND_JOB_LOST:
        body = _background_lost_body(data)
    else:
        body = ""
    if not body:
        body = _generic_data_html(data)
    return body if body else '<span class="muted">（无数据）</span>'


def _render_event(
    event: Event,
    full_outputs: dict[str, str],
    results_by_id: dict[str, str],
    call_names: dict[str, str],
) -> str:
    """One timeline block: badge + timestamp + data summary."""
    data = event.data if isinstance(event.data, dict) else {}
    label, category = _EVENT_META.get(event.type, (event.type.value, "round"))
    badge_cls = f"b-{category}"
    state = ""
    if event.type is EventType.TOOL_CALL_RESULT:
        ok = True if data.get("success") is None else bool(data.get("success"))
        badge_cls = "b-ok" if ok else "b-fail"
        state = "ok" if ok else "fail"
    elif event.type is EventType.APPROVAL_DECISION:
        granted = bool(data.get("granted"))
        badge_cls = "b-ok" if granted else "b-fail"
        state = "ok" if granted else "fail"
    body = _event_body(event, data, full_outputs, results_by_id, call_names)
    return (
        f'<div class="event {state}">\n'
        '<div class="ev-head">'
        f'<span class="badge {badge_cls}">{_esc(label)}</span>'
        f'<code class="ev-type">{_esc(event.type.value)}</code>'
        f'<span class="ev-time">#{_esc(event.seq)} · {_esc(_fmt_ts(event.timestamp))}</span>'
        "</div>\n"
        f'<div class="ev-body">{body}</div>\n'
        "</div>"
    )


# ---------------------------------------------------------------------------
# Section renderers
# ---------------------------------------------------------------------------


def _status_html(summary: SessionSummary) -> str:
    status = summary.status or "running"
    label = _EXIT_REASON_LABELS.get(status) or _STATUS_LABELS.get(status) or status
    out = f'{_esc(label)} <span class="muted">({_esc(status)})</span>'
    exit_reason = summary.exit_reason
    if exit_reason and exit_reason != status:
        er_label = _EXIT_REASON_LABELS.get(exit_reason, exit_reason)
        out += (
            f' <span class="muted">· 退出原因：{_esc(er_label)}'
            f" ({_esc(exit_reason)})</span>"
        )
    return out


def _header_html(summary: SessionSummary) -> str:
    total = summary.input_tokens + summary.output_tokens
    return (
        '<header class="panel">'
        "<h1>Mini Claude Code 执行报告</h1>"
        '<table class="meta">'
        f"<tr><th>会话 ID</th><td><code>{_esc(summary.session_id)}</code></td></tr>"
        f"<tr><th>状态</th><td>{_status_html(summary)}</td></tr>"
        f'<tr><th>工作区</th><td><code>{_esc(summary.workspace)}</code></td></tr>'
        f"<tr><th>模型</th><td>{_esc(summary.provider)} / {_esc(summary.model)}</td></tr>"
        f"<tr><th>创建时间</th><td>{_esc(_fmt_ts(summary.created_at))}</td></tr>"
        f"<tr><th>轮数</th><td>{_esc(summary.rounds)}</td></tr>"
        f"<tr><th>Token</th><td>输入 {_esc(summary.input_tokens)}"
        f" · 输出 {_esc(summary.output_tokens)} · 总计 {_esc(total)}</td></tr>"
        f"<tr><th>缓存 Token</th><td>读取 {_esc(summary.cache_read_tokens)}"
        f" · 写入 {_esc(summary.cache_write_tokens)}"
        f"{' · 用量不完整' if not summary.usage_available else ''}</td></tr>"
        "</table></header>"
    )


def _goal_section_html(ordered_events: list[Event]) -> str:
    """Acceptance-evidence table aggregating every goal_check event."""
    rows: list[str] = []
    dash = '<span class="muted">—</span>'
    for event in ordered_events:
        if event.type is not EventType.GOAL_CHECK:
            continue
        data = event.data if isinstance(event.data, dict) else {}
        items = _goal_items(data) or [{}]
        for item in items:
            text = _first(item, "item", "name", "check", "description")
            kind = _first(item, "kind")
            passed = _first(item, "passed", "ok", "success")
            exit_code = item.get("exit_code")
            rows.append(
                "<tr>"
                f"<td>{_esc(text) if text else dash}</td>"
                f"<td>{_esc(kind) if kind else dash}</td>"
                f"<td>{_pass_mark(passed)}</td>"
                f"<td>{'<code>' + _esc(exit_code) + '</code>' if exit_code is not None else dash}</td>"
                f'<td class="ev-time">{_esc(_fmt_ts(event.timestamp))}</td>'
                "</tr>"
            )
    if rows:
        body = (
            '<table class="goal-table"><thead><tr>'
            "<th>item</th><th>kind</th><th>通过</th><th>退出码</th><th>时间</th>"
            "</tr></thead><tbody>"
            + "\n".join(rows)
            + "</tbody></table>"
        )
    else:
        body = '<p class="muted">（无 goal_check 事件）</p>'
    return '<section class="panel"><h2>目标验收（Goal Checks）</h2>' + body + "</section>"


def _timeline_section_html(ordered_events: list[Event], full_outputs: dict[str, str], results_by_id: dict[str, str], call_names: dict[str, str]) -> str:
    if ordered_events:
        body = '<div class="timeline">' + "\n".join(
            _render_event(event, full_outputs, results_by_id, call_names)
            for event in ordered_events
        ) + "</div>"
    else:
        body = '<p class="muted">（无事件记录）</p>'
    return '<section class="panel"><h2>时间线</h2>' + body + "</section>"


def _footer_html(summary: SessionSummary) -> str:
    total = summary.input_tokens + summary.output_tokens
    return (
        '<footer class="panel">'
        "<h2>用量</h2>"
        f"<p>输入 Token <strong>{_esc(summary.input_tokens)}</strong>"
        f" · 输出 Token <strong>{_esc(summary.output_tokens)}</strong>"
        f" · 总计 <strong>{_esc(total)}</strong>"
        f" · 缓存读取 <strong>{_esc(summary.cache_read_tokens)}</strong>"
        f" · 缓存写入 <strong>{_esc(summary.cache_write_tokens)}</strong>"
        f" · 轮数 <strong>{_esc(summary.rounds)}</strong></p>"
        '<p class="muted">单文件离线报告：无外部 CSS / JS / 字体依赖，'
        "无需 JavaScript 即可查看全部内容（折叠区使用原生 details 元素）。</p>"
        "</footer>"
    )


# ---------------------------------------------------------------------------
# Public entry point
# ---------------------------------------------------------------------------


def render_session_html(
    summary: SessionSummary,
    events: list[Event],
    messages: list[Message],
    full_outputs: dict[str, str] | None = None,
) -> str:
    """Render one session as a single self-contained offline HTML page.

    Args:
        summary: The stored session projection (id, status, tokens, ...).
        events: The append-only event trace; rendered in ``seq`` order.
        messages: Stored conversation; provides the edit/write full diff
            fallback when ``full_outputs`` lacks the call.
        full_outputs: ``call_id -> full tool output`` (from ToolResultBlock)
            shown in expandable ``<details>`` blocks.

    Returns:
        A complete HTML document string: inline CSS, dark theme, no
        JavaScript, no external resources.
    """
    outputs = dict(full_outputs) if full_outputs else {}
    results_by_id = _tool_result_contents(messages or [])
    call_names = _call_names(events or [])
    ordered = sorted(events or [], key=lambda event: event.seq)

    title = f"Mini Claude Code 执行报告 — {_esc(summary.session_id)}"
    return (
        "<!DOCTYPE html>\n"
        '<html lang="zh-CN">\n'
        "<head>\n"
        '<meta charset="utf-8">\n'
        '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
        f"<title>{title}</title>\n"
        f"<style>{_CSS}</style>\n"
        "</head>\n"
        "<body>\n"
        '<div class="wrap">\n'
        + _header_html(summary)
        + _goal_section_html(ordered)
        + _timeline_section_html(ordered, outputs, results_by_id, call_names)
        + _footer_html(summary)
        + "\n</div>\n</body>\n</html>\n"
    )
