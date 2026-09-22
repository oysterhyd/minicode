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
        content = ctx.artifact_store.read(ctx.session_id, args.artifact_id)
        if content is None:
            return ToolOutcome.failure(
                f"unknown artifact for this session: {args.artifact_id}"
            )
        total = len(content)
        if args.offset > total:
            return ToolOutcome.failure(
                f"offset {args.offset} beyond end of artifact ({total} characters)"
            )
        end = min(total, args.offset + args.limit)
        page = content[args.offset:end]
        status = f"artifact {args.artifact_id} · characters {args.offset}-{end} of {total}"
        if end < total:
            status += f" · next_offset={end}"
        else:
            status += " · end"
        return ToolOutcome(output=f"{status}\n{page}")
