"""read_artifact: read back the full original output behind a reference.

Compaction and output spilling replace large content with a short preview
plus an ``[artifact:<id>]`` reference; the complete text lives in the
session's artifact store (see ``minicode.storage.artifacts``). This tool is
the model's on-demand way to pull the original bytes back into context.
"""

from __future__ import annotations

from pydantic import BaseModel, Field

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, truncate_output


class ReadArtifactArgs(BaseModel):
    """Arguments for :class:`ReadArtifactTool`."""

    artifact_id: str = Field(
        pattern=r"^[a-z_]+[0-9a-f]*$",
        description=(
            "identifier taken from an [artifact:<id>] reference, "
            "e.g. archive_1a2b3c4d5e6f"
        ),
    )


class ReadArtifactTool(BaseTool):
    name = "read_artifact"
    description = (
        "Read the full original content of a spilled or archived output by "
        "reference. Large tool outputs and compacted history appear in "
        "context only as a preview plus an [artifact:<id>] marker; pass that "
        "id here to retrieve the complete original text."
    )
    requires_approval = False
    args_model = ReadArtifactArgs

    async def execute(self, args: ReadArtifactArgs, ctx: ToolContext) -> ToolOutcome:
        if ctx.artifact_store is None or ctx.session_id is None:
            return ToolOutcome.failure("no artifact store configured for this session")
        content = ctx.artifact_store.read(ctx.session_id, args.artifact_id)
        if content is None:
            return ToolOutcome.failure(f"unknown artifact: {args.artifact_id}")
        return ToolOutcome(output=truncate_output(content, ctx.limits.max_output_chars))
