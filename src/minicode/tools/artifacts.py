"""Session-scoped access to archived tool output and compacted history."""

from __future__ import annotations

from pydantic import BaseModel, Field

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext


class ReadArtifactArgs(BaseModel):
    artifact_id: str = Field(min_length=1, description="artifact id from [artifact:<id>]")
    offset: int = Field(default=0, ge=0, description="zero-based character offset")
    limit: int = Field(default=4000, ge=1, le=20_000, description="characters to return")


class ReadArtifactTool(BaseTool):
    """Read a bounded page from an artifact owned by the current session."""

    name = "read_artifact"
    description = (
        "Read a page from a session artifact referenced as [artifact:<id>]. "
        "Use offset from the previous page's next_offset to continue. Artifacts "
        "from other sessions are never accessible."
    )
    requires_approval = False
    args_model = ReadArtifactArgs

    async def execute(self, args: ReadArtifactArgs, ctx: ToolContext) -> ToolOutcome:
        if ctx.artifact_store is None or ctx.session_id is None:
            return ToolOutcome.failure("artifact storage is not available for this session")
        aread_page = getattr(ctx.artifact_store, "aread_page", None)
        read_page = getattr(ctx.artifact_store, "read_page", None)
        if callable(aread_page):
            result = await aread_page(ctx.session_id, args.artifact_id, args.offset, args.limit)
        elif callable(read_page):
            result = read_page(ctx.session_id, args.artifact_id, args.offset, args.limit)
        else:
            content = ctx.artifact_store.read(ctx.session_id, args.artifact_id)
            result = None if content is None else (
                content[args.offset:args.offset + args.limit], len(content),
                args.offset + args.limit < len(content),
            )
        if result is None:
            return ToolOutcome.failure(
                f"unknown artifact for this session: {args.artifact_id}"
            )
        page, total, has_more = result
        if total is not None and args.offset > total:
            return ToolOutcome.failure(
                f"offset {args.offset} beyond end of artifact ({total} characters)"
            )
        end = args.offset + len(page)
        total_label = str(total) if total is not None else "?"
        status = f"artifact {args.artifact_id} · characters {args.offset}-{end} of {total_label}"
        if has_more:
            status += f" · next_offset={end}"
        else:
            status += " · end"
        return ToolOutcome(output=f"{status}\n{page}")
