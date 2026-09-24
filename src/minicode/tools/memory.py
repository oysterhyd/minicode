"""Read-only project memory available to the agent."""

from __future__ import annotations

import json

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext


class MemoryListArgs(BaseModel):
    path: str | None = None


class MemoryListTool(BaseTool):
    name = "memory_list"
    description = "Recall user-saved stable project facts with their sources and scopes. Optional path filters relevant facts."
    args_model = MemoryListArgs

    async def execute(self, args: MemoryListArgs, ctx: ToolContext) -> ToolOutcome:
        if ctx.project_memory is None:
            return ToolOutcome.failure("project memory is unavailable")
        try:
            rows = ctx.project_memory.list(ctx.workspace, path=args.path)
        except (ValueError, OSError) as exc:
            return ToolOutcome.failure(str(exc))
        return ToolOutcome(output=json.dumps([row.model_dump() for row in rows], ensure_ascii=False))
