"""Layered context compaction: archive old units, shrink old tool results,
then fall back to a structured summary — all deterministic, no model calls.

Three layers run in order inside :meth:`ContextCompactor.compact`:

A. **Archive** — whole early interaction units (a fresh user request, the
   assistant's reply and the tool results it produced) are JSON-serialized
   and persisted through ``spill_fn``; the messages are replaced by a single
   summary user message carrying an ``[artifact:<id>]`` reference that the
   session report can retain as a full archive. Without a
   ``spill_fn`` the same units are replaced by a pure truncation-style
   summary (no reference, original text dropped).
B. **Shrink** — old tool results in the retained region (every result except
   the most recent ``keep_recent_tool_results``) are cut down to
   ``shrink_preview_chars`` plus a marker pointing at the execution report.
C. **Structured summary** — if the estimate is still above the trigger
   threshold and at least two early units remain, they collapse into one
   compact goal / key-files / failures / todos message.

Invariants:
* A ``tool_use`` block and its ``tool_result`` block always move together:
  units are cut only at fresh human input (a user message without
  tool_result), and every archive/merge span is pairing-checked before it is
  removed.
* The most recent ``tail_keep_rounds`` units are never archived or merged;
  their structure, text and tool_use blocks stay byte-identical.
* ``compact`` never mutates its input: untouched messages are reused as-is,
  modified ones are rebuilt as new objects.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable, Sequence
from dataclasses import dataclass

from pydantic import BaseModel, Field

from minicode.context.estimate import estimate_messages_tokens
from minicode.core.models import (
    Block,
    Message,
    TextBlock,
    ToolResultBlock,
    ToolSpec,
    ToolUseBlock,
)

__all__ = [
    "CompactConfig",
    "CompactResult",
    "CompactStats",
    "ContextCompactor",
]

# Appended to a shrunken tool_result content (spec-mandated marker text).
SHRINK_MARKER = "\n...[已压缩；完整内容见执行报告]"

# [artifact:<id>] references embedded in text; carried over into structured
# summaries so archive references survive a merge.
_ARTIFACT_REF = re.compile(r"\[artifact:[A-Za-z0-9_\-]+\]")

# Deterministic "remaining todo" extraction: checklist items or todo keywords.
_TODO_LINE = re.compile(r"\[ \]|\btodo\b|待办", re.IGNORECASE)


class CompactConfig(BaseModel):
    """Tuning knobs for :class:`ContextCompactor`."""

    max_context_tokens: int = Field(gt=0, description="estimated prompt budget")
    trigger_fraction: float = Field(
        default=0.8, gt=0.0, le=1.0, description="compact when estimate exceeds max * fraction"
    )
    tail_keep_rounds: int = Field(
        default=4, ge=0, description="most recent interaction units kept verbatim"
    )
    shrink_preview_chars: int = Field(
        default=200, ge=0, description="kept length of a shrunken tool result"
    )
    min_archive_units: int = Field(
        default=2, ge=1, description="archive only when at least this many early units exist"
    )
    # Step B needs a notion of "old" results: this many most-recent
    # tool_result blocks are never shrunk. (Referenced by the P1 spec's
    # step B but missing from its config field list, so declared here.)
    keep_recent_tool_results: int = Field(default=2, ge=0)


class CompactStats(BaseModel):
    """What one compaction pass actually did."""

    tokens_before: int = 0
    tokens_after: int = 0
    archived_units: int = 0
    shrunk_results: int = 0
    summarized_units: int = 0


@dataclass
class CompactResult:
    """Compacted message list plus bookkeeping; ``changed`` is False when the
    input was already small enough and was returned untouched. A plain
    dataclass on purpose: an unchanged run returns the caller's list object
    itself, which pydantic validation would silently re-wrap."""

    messages: list[Message]
    stats: CompactStats
    changed: bool


# ---------------------------------------------------------------------------
# Segmentation: interaction units
# ---------------------------------------------------------------------------


def _is_fresh_input(message: Message) -> bool:
    """A new human input: a user message that carries no tool_result blocks.

    Unit boundaries only land here, which is what keeps every tool_use
    paired with its tool_result inside one unit.
    """
    return message.role == "user" and not any(
        block.type == "tool_result" for block in message.content
    )


def _segment(messages: list[Message]) -> tuple[int, list[tuple[int, int]]]:
    """Split *messages* into a retained head and ``[start, end)`` index units.

    Returns ``(head_end, units)``. The head is everything before the first
    fresh human input (e.g. a leading assistant message); it is always
    retained. Each unit starts at one fresh input and swallows every
    following non-fresh message (assistant replies, tool_result batches), so
    a tool_use and its tool_result can never land in different units.
    """
    n = len(messages)
    head_end = 0
    while head_end < n and not _is_fresh_input(messages[head_end]):
        head_end += 1
    units: list[tuple[int, int]] = []
    i = head_end
    while i < n:
        start = i
        i += 1
        while i < n and not _is_fresh_input(messages[i]):
            i += 1
        units.append((start, i))
    return head_end, units


def _early_units(units: list[tuple[int, int]], keep: int) -> list[tuple[int, int]]:
    """Units older than the ``keep``-unit tail; empty when all are tail."""
    if keep <= 0:
        return list(units)
    return units[:-keep] if len(units) > keep else []


def _pairing_ok(messages: list[Message], lo: int, hi: int) -> bool:
    """True when removing the span ``[lo, hi)`` cannot split a pair.

    Every tool_result inside the span must reference a tool_use inside it,
    and every tool_result outside the span must reference a tool_use outside
    it. Well-formed histories always pass; the check is a safety net for
    pathological orderings (a result arriving after a new human input).
    """
    uses_inside: set[str] = set()
    uses_outside: set[str] = set()
    for index, message in enumerate(messages):
        for block in message.content:
            if isinstance(block, ToolUseBlock):
                (uses_inside if lo <= index < hi else uses_outside).add(block.id)
    for index, message in enumerate(messages):
        for block in message.content:
            if isinstance(block, ToolResultBlock):
                pool = uses_inside if lo <= index < hi else uses_outside
                if block.tool_use_id not in pool:
                    return False
    return True


# ---------------------------------------------------------------------------
# Deterministic summary rendering
# ---------------------------------------------------------------------------


def _shorten(text: str, limit: int) -> str:
    text = text.strip()
    return text if len(text) <= limit else text[:limit] + "…"


def _summarize_call(call: ToolUseBlock, limit: int = 40) -> str:
    """``name(key=value, ...)`` digest of one tool call."""
    if not call.input:
        return f"{call.name}()"
    parts: list[str] = []
    for key, value in call.input.items():
        if isinstance(value, str):
            rendered = repr(_shorten(value, limit))
        else:
            try:
                rendered = json.dumps(value, ensure_ascii=False)
            except (TypeError, ValueError):  # pragma: no cover - inputs are JSON-safe
                rendered = str(value)
            if len(rendered) > limit:
                rendered = rendered[:limit] + "…"
        parts.append(f"{key}={rendered}")
    return f"{call.name}({', '.join(parts)})"


def _unit_line(unit_messages: Sequence[Message], ordinal: int) -> str:
    """One digest line for one archived unit: assistant excerpt, tool calls,
    modified files (edit / write ``path`` arguments)."""
    calls: list[str] = []
    files: list[str] = []
    texts: list[str] = []
    for message in unit_messages:
        for block in message.content:
            if isinstance(block, ToolUseBlock):
                calls.append(_summarize_call(block))
                if block.name in ("edit", "write"):
                    path = block.input.get("path")
                    if isinstance(path, str) and path not in files:
                        files.append(path)
            elif isinstance(block, TextBlock) and message.role == "assistant" and block.text.strip():
                texts.append(block.text.strip())
    excerpt = _shorten(" ".join(texts), 120) or "（无文本）"
    tools = "; ".join(calls) if calls else "无"
    touched = ", ".join(files) if files else "无"
    return f"- 单元 {ordinal}: 助手摘录: {excerpt} | 工具: {tools} | 修改文件: {touched}"


def _archive_summary(groups: list[list[Message]], artifact_id: str | None) -> str:
    """Replacement text for the archived prefix of units."""
    count = len(groups)
    lines = [f"[context compacted] 最早的 {count} 个交互单元（单元 1-{count}）已归档："]
    for ordinal, group in enumerate(groups, start=1):
        lines.append(_unit_line(group, ordinal))
    if artifact_id is not None:
        lines.append(f"归档引用: [artifact:{artifact_id}]（完整内容保存在会话归档中）")
    else:
        lines.append("（未配置 artifact 存储，原始内容未保留）")
    return "\n".join(lines)


def _structured_summary(groups: list[list[Message]]) -> str:
    """Step C fallback: one line each for goal, key files, failures, todos."""
    goal = ""
    files: list[str] = []
    failures: list[str] = []
    todos: list[str] = []
    refs: list[str] = []
    message_count = 0
    for group in groups:
        for message in group:
            message_count += 1
            for block in message.content:
                if isinstance(block, TextBlock):
                    if not goal and message.role == "user" and block.text.strip():
                        goal = _shorten(block.text, 120)
                    for line in block.text.splitlines():
                        stripped = line.strip()
                        if stripped and _TODO_LINE.search(stripped) and stripped not in todos:
                            todos.append(_shorten(stripped, 80))
                    refs.extend(match.group(0) for match in _ARTIFACT_REF.finditer(block.text))
                elif isinstance(block, ToolUseBlock):
                    if block.name in ("edit", "write"):
                        path = block.input.get("path")
                        if isinstance(path, str) and path not in files:
                            files.append(path)
                elif isinstance(block, ToolResultBlock) and block.is_error:
                    stripped = block.content.strip()
                    first_line = stripped.splitlines()[0] if stripped else ""
                    if first_line and first_line not in failures:
                        failures.append(_shorten(first_line, 100))
    lines = [
        f"[context summary] {message_count} 条消息 / {len(groups)} 个早期交互单元已合并为结构化摘要：",
        f"- 目标: {goal or '（未记录）'}",
        f"- 关键文件: {', '.join(files) if files else '无'}",
        f"- 失败原因: {'; '.join(failures[:3]) if failures else '无'}",
        f"- 剩余 todo: {'; '.join(todos[:3]) if todos else '无'}",
    ]
    unique_refs = list(dict.fromkeys(refs))
    if unique_refs:
        lines.append(f"- 归档引用: {' '.join(unique_refs[:3])}（完整内容保存在会话归档中）")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# The compactor
# ---------------------------------------------------------------------------


class ContextCompactor:
    """Stateless per-call compactor; bind the artifact store on the loop side
    by passing ``spill_fn = lambda kind, content: store.spill(session_id,
    kind, content).artifact_id``.

    ``context_tokens_fn`` supplies the prompt budget to trigger on — normally
    the active model's ``context_window - max_output_tokens`` (see
    :meth:`~minicode.runtime.AgentRuntime.prompt_budget_tokens`), read on every
    check so a mid-session ``/model`` switch is honoured. Without it the static
    ``config.max_context_tokens`` is used.
    """

    def __init__(
        self,
        config: CompactConfig,
        spill_fn: Callable[[str, str], str] | None = None,
        context_tokens_fn: Callable[[], int] | None = None,
    ) -> None:
        self.config = config
        self._spill_fn = spill_fn
        self._context_tokens_fn = context_tokens_fn

    @property
    def context_tokens(self) -> int:
        """Prompt budget currently in force (tokens)."""
        if self._context_tokens_fn is None:
            return self.config.max_context_tokens
        return self._context_tokens_fn()

    @property
    def _threshold(self) -> float:
        return self.context_tokens * self.config.trigger_fraction

    def needs_compaction(
        self,
        system: str | None,
        messages: list[Message],
        tool_specs: list[ToolSpec] | None = None,
    ) -> bool:
        """True when the estimated next prompt exceeds the trigger threshold."""
        return estimate_messages_tokens(system, messages, tool_specs) > self._threshold

    def compact(
        self,
        system: str | None,
        messages: list[Message],
        tool_specs: list[ToolSpec] | None = None,
    ) -> CompactResult:
        """Run the three layers; ``system`` is only used for estimation.

        Empty, tiny or all-tail inputs come back unchanged (``changed=False``)
        with the original message list object.
        """
        stats = CompactStats(tokens_before=estimate_messages_tokens(system, messages, tool_specs))
        current = list(messages)

        current, archived = self._archive_early_units(current)
        stats.archived_units = archived

        current, shrunk = self._shrink_old_results(current)
        stats.shrunk_results = shrunk

        current, summarized = self._structured_summarize(system, current, tool_specs)
        stats.summarized_units = summarized

        changed = bool(archived or shrunk or summarized)
        stats.tokens_after = estimate_messages_tokens(system, current, tool_specs)
        if not changed:
            return CompactResult(messages=messages, stats=stats, changed=False)
        return CompactResult(messages=current, stats=stats, changed=True)

    # -- step A: archive the early prefix of units ---------------------------

    def _archive_early_units(self, messages: list[Message]) -> tuple[list[Message], int]:
        _head_end, units = _segment(messages)
        early = _early_units(units, self.config.tail_keep_rounds)
        if len(early) < self.config.min_archive_units:
            return messages, 0

        # Largest prefix whose removal keeps every use/result pair together.
        lo = early[0][0]
        k = len(early)
        while k > 0 and not _pairing_ok(messages, lo, early[k - 1][1]):
            k -= 1
        if k <= 0:
            return messages, 0

        span_start = early[0][0]
        span_end = early[k - 1][1]
        groups = [messages[start:end] for start, end in early[:k]]

        artifact_id: str | None = None
        if self._spill_fn is not None:
            payload = json.dumps(
                [message.model_dump() for message in messages[span_start:span_end]],
                ensure_ascii=False,
            )
            artifact_id = self._spill_fn("archive", payload)

        replacement = Message(
            role="user", content=[TextBlock(text=_archive_summary(groups, artifact_id))]
        )
        return messages[:span_start] + [replacement] + messages[span_end:], k

    # -- step B: shrink old tool results in the retained region --------------

    def _shrink_old_results(self, messages: list[Message]) -> tuple[list[Message], int]:
        positions = [
            (message_index, block_index)
            for message_index, message in enumerate(messages)
            for block_index, block in enumerate(message.content)
            if isinstance(block, ToolResultBlock)
        ]
        keep_recent = self.config.keep_recent_tool_results
        protected = (
            set(positions[len(positions) - keep_recent :]) if keep_recent > 0 else set()
        )
        preview = self.config.shrink_preview_chars
        shrunk = 0
        rebuilt: list[Message] = []
        for message_index, message in enumerate(messages):
            needs_rebuild = False
            blocks: list[Block] = []
            for block_index, block in enumerate(message.content):
                if (
                    isinstance(block, ToolResultBlock)
                    and (message_index, block_index) not in protected
                    and len(block.content) > preview
                ):
                    blocks.append(
                        ToolResultBlock(
                            tool_use_id=block.tool_use_id,
                            content=block.content[:preview] + SHRINK_MARKER,
                            is_error=block.is_error,
                        )
                    )
                    shrunk += 1
                    needs_rebuild = True
                else:
                    blocks.append(block)
            rebuilt.append(
                Message(role=message.role, content=blocks) if needs_rebuild else message
            )
        return rebuilt, shrunk

    # -- step C: structured summary of the remaining early units -------------

    def _structured_summarize(
        self,
        system: str | None,
        messages: list[Message],
        tool_specs: list[ToolSpec] | None,
    ) -> tuple[list[Message], int]:
        if estimate_messages_tokens(system, messages, tool_specs) <= self._threshold:
            return messages, 0
        _head_end, units = _segment(messages)
        early = _early_units(units, self.config.tail_keep_rounds)
        if len(early) < 2:
            return messages, 0

        # Same pairing safety net as step A; need at least two mergeable units.
        lo = early[0][0]
        m = len(early)
        while m >= 2 and not _pairing_ok(messages, lo, early[m - 1][1]):
            m -= 1
        if m < 2:
            return messages, 0

        span_start = early[0][0]
        span_end = early[m - 1][1]
        groups = [messages[start:end] for start, end in early[:m]]
        replacement = Message(
            role="user", content=[TextBlock(text=_structured_summary(groups))]
        )
        return messages[:span_start] + [replacement] + messages[span_end:], m
