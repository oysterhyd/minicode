"""Local skill discovery and one-level read-only delegation tools."""

from __future__ import annotations

from pydantic import BaseModel, Field

from minicode.context.extensions import SkillCatalog
from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext


class NoArgs(BaseModel):
    pass


class SkillNameArgs(BaseModel):
    name: str = Field(min_length=1)


class SkillResourceArgs(SkillNameArgs):
    path: str = Field(min_length=1)


class SkillsListTool(BaseTool):
    name = "skills_list"
    description = "List available local skills by name and description. Skill bodies are not loaded."
    args_model = NoArgs

    def __init__(self, catalog: SkillCatalog):
        self.catalog = catalog

    async def execute(self, args: NoArgs, ctx: ToolContext) -> ToolOutcome:
        return ToolOutcome(output=self.catalog.listing())


class SkillLoadTool(BaseTool):
    name = "skill_load"
    description = "Activate a named skill and read its instructions. Activation does not grant tool permissions."
    args_model = SkillNameArgs

    async def execute(self, args: SkillNameArgs, ctx: ToolContext) -> ToolOutcome:
        if ctx.activate_skill is None:
            return ToolOutcome.failure("skill activation is unavailable")
        try:
            return ToolOutcome(output=ctx.activate_skill(args.name))
        except (ValueError, OSError, UnicodeError) as exc:
            return ToolOutcome.failure(str(exc))


class SkillUnloadTool(BaseTool):
    name = "skill_unload"
    description = "Deactivate a previously loaded skill for this session."
    args_model = SkillNameArgs

    async def execute(self, args: SkillNameArgs, ctx: ToolContext) -> ToolOutcome:
        if ctx.deactivate_skill is None:
            return ToolOutcome.failure("skill deactivation is unavailable")
        try:
            return ToolOutcome(output=ctx.deactivate_skill(args.name))
        except ValueError as exc:
            return ToolOutcome.failure(str(exc))


class SkillResourceTool(BaseTool):
    name = "skill_resource"
    description = "Read a file inside a registered skill directory; scripts still require normal bash approval."
    args_model = SkillResourceArgs

    def __init__(self, catalog: SkillCatalog):
        self.catalog = catalog

    async def execute(self, args: SkillResourceArgs, ctx: ToolContext) -> ToolOutcome:
        try:
            source = self.catalog.resource(args.name, args.path)
        except (ValueError, OSError, UnicodeError) as exc:
            return ToolOutcome.failure(str(exc))
        return ToolOutcome(output=f"来源：{source.label()}\n{source.content}")


class DelegateArgs(BaseModel):
    kind: str = Field(min_length=1)
    task: str = Field(min_length=1)


class DelegateTool(BaseTool):
    name = "delegate"
    description = (
        "Delegate bounded repository exploration or review to a read-only assistant. "
        "It may use only read, ls, grep and its own session artifacts. "
        "The parent receives a concise result and a child session reference."
    )
    args_model = DelegateArgs

    def __init__(self, agent_kinds: list[str] | None = None, *, include_builtins: bool = True, descriptions: dict[str, str] | None = None):
        self.allowed_kinds = set(agent_kinds or []) | ({"explore", "review"} if include_builtins else set())
        self.description = "Delegate a bounded task to an assistant with its configured tools and the parent permission policy. Available kinds: " + ", ".join(sorted(self.allowed_kinds))
        if descriptions:
            self.description += "\n" + "\n".join(f"{name}: {description}" for name, description in descriptions.items())

    async def execute(self, args: DelegateArgs, ctx: ToolContext) -> ToolOutcome:
        if args.kind not in self.allowed_kinds:
            return ToolOutcome.failure(f"unknown read-only subagent kind: {args.kind}")
        if ctx.delegate is None:
            return ToolOutcome.failure("delegation is unavailable")
        return await ctx.delegate(args.kind, args.task)
