"""Model-facing operations on the session's durable task board."""

from __future__ import annotations

import json

from pydantic import BaseModel, Field

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext


class TaskCreateArgs(BaseModel):
    task_id: str = Field(min_length=1, max_length=80, pattern=r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
    title: str = Field(min_length=1, max_length=300)
    depends_on: list[str] = Field(default_factory=list, max_length=50)


class TaskIdArgs(BaseModel):
    task_id: str = Field(min_length=1)


class TaskCompleteArgs(TaskIdArgs):
    ok: bool


class TaskListArgs(BaseModel):
    pass


def _board(ctx: ToolContext):
    if ctx.task_store is None or ctx.session_id is None:
        raise ValueError("task board is unavailable")
    return ctx.task_store, ctx.session_id


class TaskCreateTool(BaseTool):
    name = "task_create"
    description = "Add a durable session task; dependencies must already exist."
    args_model = TaskCreateArgs

    async def execute(self, args: TaskCreateArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            store, session = _board(ctx)
            store.add(session, args.task_id, args.title, args.depends_on)
            return ToolOutcome(output=f"created task {args.task_id}")
        except ValueError as exc:
            return ToolOutcome.failure(str(exc))


class TaskListTool(BaseTool):
    name = "task_list"
    description = "List durable tasks, dependencies, owners and statuses in this session."
    args_model = TaskListArgs

    async def execute(self, args: TaskListArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            store, session = _board(ctx)
            rows = [row.model_dump() for row in store.list_tasks(session)]
            return ToolOutcome(output=json.dumps(rows, ensure_ascii=False))
        except ValueError as exc:
            return ToolOutcome.failure(str(exc))


class TaskClaimTool(BaseTool):
    name = "task_claim"
    description = "Claim one ready task for the current agent; blocked dependencies cannot be claimed."
    args_model = TaskIdArgs

    async def execute(self, args: TaskIdArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            store, session = _board(ctx)
            store.claim(session, args.task_id, owner="agent")
            return ToolOutcome(output=f"claimed task {args.task_id}")
        except ValueError as exc:
            return ToolOutcome.failure(str(exc))


class TaskCompleteTool(BaseTool):
    name = "task_complete"
    description = "Finish a task claimed by this agent; success unblocks dependents."
    args_model = TaskCompleteArgs

    async def execute(self, args: TaskCompleteArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            store, session = _board(ctx)
            store.complete(session, args.task_id, args.ok, owner="agent")
            return ToolOutcome(output=f"task {args.task_id}: {'done' if args.ok else 'failed'}")
        except ValueError as exc:
            return ToolOutcome.failure(str(exc))
