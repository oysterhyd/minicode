"""Read-only research subagents: a sandboxed AgentRuntime over three tools.

:class:`SubagentRunner` spins up a child :class:`~minicode.runtime.loop.AgentRuntime`
whose registry only contains the read-only tools (``read_file``,
``list_files``, ``search_text``), so the model physically cannot modify the
workspace. The child shares the parent's :class:`~minicode.storage.SqliteStore`
(child events/messages land in the same database, in their own session row)
and runs under a dedicated system prompt that forces the final answer to be a
single JSON object, which :meth:`SubagentRunner.run` parses into a
structured :class:`SubagentResult`.

Import direction note: this module imports ``runtime.loop`` one-way — the
runtime never imports ``tasks`` — so there is no circular dependency.
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from minicode.core.models import Budget, TextBlock
from minicode.providers.base import Provider
from minicode.runtime.loop import AgentRuntime, TextDeltaCallback
from minicode.runtime.events import EventCallback
from minicode.security.policy import DefaultPolicy, PermissionPolicy
from minicode.storage import SessionStore, SqliteStore
from minicode.tools.files import ListFilesTool, ReadFileTool
from minicode.tools.registry import ToolRegistry
from minicode.tools.search import SearchTextTool

#: Dedicated system prompt: read-only researcher that must answer in JSON.
SUBAGENT_SYSTEM_PROMPT = (
    "你是一个只读研究子代理（read-only research subagent），在父代理的工作区内"
    "完成调研、代码定位和信息汇总任务。\n"
    "硬性约束：\n"
    "- 你只能使用只读工具（read_file / list_files / search_text）；\n"
    "- 禁止修改、创建或删除任何文件，禁止执行任何命令；\n"
    "- 不要输出与任务无关的对话内容。\n"
    "完成研究后，最终回复必须且只能是一个 JSON 对象（不要包 markdown 围栏、"
    "不要附加任何其他文字），格式如下：\n"
    '{"summary": "研究结论（一到几句话）",'
    ' "findings": ["关键发现", ...],'
    ' "evidence_refs": ["文件:行号 或 artifact 引用", ...],'
    ' "unresolved": ["未能解决的问题", ...]}'
)

#: An evidence reference shaped like ``path:line`` (workspace-relative path).
_PATH_LINE_RE = re.compile(r"^(?P<path>[^:]+):(?P<line>\d+)$")


class SubagentResult(BaseModel):
    """Structured outcome of one read-only subagent run."""

    summary: str
    findings: list[str] = []
    evidence_refs: list[str] = []
    unresolved: list[str] = []
    input_tokens: int = 0
    output_tokens: int = 0
    rounds: int = 0


def validate_refs(result: SubagentResult, workspace: Path) -> SubagentResult:
    """Verify ``path:line`` evidence refs against *workspace*, in place.

    Refs shaped like ``path:line`` must point at an existing file inside the
    workspace; invalid ones are moved to ``unresolved``. Refs in any other
    shape (e.g. artifact references) are passed through untouched.
    """
    kept: list[str] = []
    unresolved = list(result.unresolved)
    workspace_root = Path(workspace).resolve()
    for ref in result.evidence_refs:
        match = _PATH_LINE_RE.match(ref.strip())
        if match is None:
            kept.append(ref)
            continue
        candidate = match.group("path")
        try:
            resolved = (workspace_root / candidate).resolve()
            inside = resolved.is_relative_to(workspace_root)
            exists = resolved.exists()
        except OSError:
            inside = exists = False
        if inside and exists:
            kept.append(ref)
        else:
            unresolved.append(ref)
    result.evidence_refs = kept
    result.unresolved = unresolved
    return result


def _extract_json_object(text: str) -> dict[str, Any] | None:
    """Best-effort extraction of the first JSON object embedded in *text*.

    Tolerates markdown fences and any prose around the object: each ``{`` is
    tried as the start of a JSON value until one decodes cleanly.
    """
    decoder = json.JSONDecoder()
    idx = text.find("{")
    while idx != -1:
        try:
            value, _end = decoder.raw_decode(text[idx:])
        except json.JSONDecodeError:
            idx = text.find("{", idx + 1)
            continue
        return value if isinstance(value, dict) else None
    return None


def _string_list(value: Any) -> list[str]:
    """Coerce a decoded JSON field into a list of strings (or empty)."""
    if not isinstance(value, list):
        return []
    return [str(item) for item in value]


def _final_assistant_text(store: SessionStore, session_id: str | None) -> str:
    """Concatenated text of the session's last assistant message."""
    if session_id is None:
        return ""
    for message in reversed(store.get_messages(session_id)):
        if message.role == "assistant":
            return "".join(
                block.text for block in message.content if isinstance(block, TextBlock)
            )
    return ""


class SubagentRunner:
    """Runs one read-only child agent and returns a structured result.

    Args:
        provider: model provider, shared with the parent agent.
        workspace: workspace root the child may read from.
        parent_budget: the parent's budget; the child inherits its token and
            wall-clock caps while getting its own round budget.
        store: parent session store, so child messages/events land in the
            same database. ``None`` runs the child on a throwaway in-memory
            store (useful for tests and one-off research).
        session_id: parent session this run is attached to (informational
            only — the child always creates its own session row).
        on_event: optional event sink mirroring the child's event stream.
        on_text_delta: optional callback for the child's streamed text.
        policy: permission policy for the child; defaults to
            :class:`~minicode.security.policy.DefaultPolicy`, which allows
            all three read-only tools.
    """

    def __init__(
        self,
        *,
        provider: Provider,
        workspace: Path,
        parent_budget: Budget,
        store: SessionStore | None = None,
        session_id: str | None = None,
        on_event: EventCallback | None = None,
        on_text_delta: TextDeltaCallback | None = None,
        policy: PermissionPolicy | None = None,
    ) -> None:
        self._provider = provider
        self._workspace = Path(workspace)
        self._parent_budget = parent_budget
        self._store = store
        self._session_id = session_id
        self._on_event = on_event
        self._on_text_delta = on_text_delta
        self._policy = policy

    async def run(self, task: str, max_rounds: int = 8) -> SubagentResult:
        """Run *task* to completion on a read-only registry and parse the
        final answer into a :class:`SubagentResult`.

        If the final text contains a JSON object it is used verbatim
        (``summary`` / ``findings`` / ``evidence_refs`` / ``unresolved``);
        otherwise the whole text becomes ``summary`` with empty lists.
        Evidence references are validated against the workspace afterwards.
        ``asyncio.CancelledError`` propagates untouched to the caller.
        """
        own_store: SqliteStore | None = None
        store = self._store
        if store is None:
            own_store = SqliteStore(":memory:")
            store = own_store

        registry = ToolRegistry()
        for tool in (ReadFileTool(), ListFilesTool(), SearchTextTool()):
            registry.register(tool)

        budget = Budget(
            max_rounds=max_rounds,
            max_total_tokens=self._parent_budget.max_total_tokens,
            max_seconds=self._parent_budget.max_seconds,
        )
        runtime = AgentRuntime(
            provider=self._provider,
            registry=registry,
            store=store,
            policy=self._policy if self._policy is not None else DefaultPolicy(),
            workspace=self._workspace,
            provider_name=str(getattr(self._provider, "name", "unknown")),
            model=str(getattr(self._provider, "model", "unknown")),
            budget=budget,
            on_event=self._on_event,
            on_text_delta=self._on_text_delta,
            system_prompt=SUBAGENT_SYSTEM_PROMPT,
        )
        try:
            await runtime.run_turn(task)
            final_text = _final_assistant_text(store, runtime.session_id)
        finally:
            if own_store is not None:
                own_store.close()

        result = SubagentResult(summary=final_text.strip(), rounds=runtime.rounds)
        parsed = _extract_json_object(final_text)
        if parsed is not None:
            result.summary = str(parsed.get("summary", ""))
            result.findings = _string_list(parsed.get("findings"))
            result.evidence_refs = _string_list(parsed.get("evidence_refs"))
            result.unresolved = _string_list(parsed.get("unresolved"))
        result.input_tokens = runtime.usage.input_tokens
        result.output_tokens = runtime.usage.output_tokens
        return validate_refs(result, self._workspace)
