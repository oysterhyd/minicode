"""delegate: hand read-only research to a subagent from the tool surface.

The tool never creates a provider itself — one (plus the workspace, budget
and optional store/policy) is injected at construction time by the host
application, mirroring how :class:`~minicode.runtime.loop.AgentRuntime` is
wired. ``ToolContext`` is accepted for interface compatibility: the
constructed workspace wins over ``ctx.workspace`` (the context is only used
for ``session_id``, which is attached to the subagent run for correlation).
"""

from __future__ import annotations

from pathlib import Path

from pydantic import BaseModel, Field

from minicode.core.models import Budget, ToolOutcome, Usage
from minicode.providers.base import Provider
from minicode.security.policy import PermissionPolicy
from minicode.storage import SessionStore
from minicode.tasks.subagent import SubagentResult, SubagentRunner
from minicode.tools.base import BaseTool, ToolContext


class DelegateSubagentArgs(BaseModel):
    """Arguments for the ``delegate`` tool."""

    task: str
    max_rounds: int = Field(default=8, ge=1, le=16)


def _format_summary(result: SubagentResult) -> str:
    """One line each for summary, findings, unresolved questions and evidence."""
    findings = "；".join(result.findings) if result.findings else "（无）"
    unresolved = "；".join(result.unresolved) if result.unresolved else "（无）"
    evidence = ", ".join(result.evidence_refs) if result.evidence_refs else "（无）"
    return "\n".join(
        [
            f"摘要: {result.summary}",
            f"发现: {findings}",
            f"未解决: {unresolved}",
            f"证据: {evidence}",
        ]
    )


class DelegateSubagentTool(BaseTool):
    """Delegate a read-only research task; return the structured summary."""

    name = "delegate"
    description = (
        "Delegate a read-only research or code-location task to a subagent "
        "and get back a structured summary (summary, findings, evidence "
        "references, unresolved questions) instead of the intermediate "
        "conversation. The subagent can only read / list / search the "
        "workspace — it cannot modify files or run commands. Use it for "
        "surveying code and gathering facts before planning edits."
    )
    requires_approval = False  # read-only: the child registry has no write tools
    args_model = DelegateSubagentArgs

    def __init__(
        self,
        provider: Provider,
        workspace: Path,
        parent_budget: Budget,
        store: SessionStore | None = None,
        policy: PermissionPolicy | None = None,
    ) -> None:
        self._provider = provider
        self._workspace = Path(workspace)
        self._parent_budget = parent_budget
        self._store = store
        self._policy = policy

    async def execute(
        self, args: DelegateSubagentArgs, ctx: ToolContext
    ) -> ToolOutcome:
        """Run a fresh :class:`SubagentRunner` for this call.

        The constructed workspace is authoritative; ``ctx`` only contributes
        ``session_id`` for display/correlation purposes.
        """
        runner = SubagentRunner(
            provider=self._provider,
            workspace=self._workspace,
            parent_budget=self._parent_budget,
            store=self._store,
            session_id=ctx.session_id,
            policy=self._policy,
        )
        try:
            result = await runner.run(args.task, max_rounds=args.max_rounds)
        except Exception as exc:  # noqa: BLE001 - a subagent crash is an ordinary tool failure
            return ToolOutcome.failure(f"subagent failed: {exc}")
        return ToolOutcome(
            output=_format_summary(result),
            usage=Usage(
                input_tokens=result.input_tokens, output_tokens=result.output_tokens
            ),
        )
