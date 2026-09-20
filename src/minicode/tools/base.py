"""Tool base classes: shared limits, execution context and argument validation."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, ClassVar, Protocol, runtime_checkable

from pydantic import BaseModel, ValidationError

from minicode.core.models import ToolOutcome, ToolSpec
from minicode.core.paths import PathOutsideWorkspaceError, resolve_in_workspace


class ToolLimits(BaseModel):
    """Resource caps applied by every built-in tool."""

    max_read_bytes: int = 256_000           # read cap
    max_output_chars: int = 20_000          # ls / grep output cap
    max_command_output_chars: int = 10_000  # bash output cap
    default_command_timeout_s: float = 60.0
    max_command_timeout_s: float = 300.0
    max_search_results: int = 100
    search_max_file_bytes: int = 1_000_000
    # Tool outputs longer than this are spilled to an artifact; the model
    # sees a preview plus a reference it can read back with read_artifact.
    spill_threshold_chars: int = 4_000
    spill_preview_chars: int = 1_000


@runtime_checkable
class ArtifactStoreLike(Protocol):
    """What tools need from the session artifact store (see storage.artifacts)."""

    def spill(self, session_id: str, kind: str, content: str) -> Any: ...

    def read(self, session_id: str, artifact_id: str) -> str | None: ...


@runtime_checkable
class BackgroundManagerLike(Protocol):
    """What tools need from the background job manager (see tasks.background)."""

    def start(self, command: str, cwd: Path, timeout_s: float) -> Any: ...


@dataclass(slots=True)
class ToolContext:
    """Per-execution context handed to every tool invocation."""

    workspace: Path                          # absolute resolved workspace root
    limits: ToolLimits = field(default_factory=ToolLimits)
    # Optional services; None keeps tools self-contained in tests.
    artifact_store: ArtifactStoreLike | None = None
    background_manager: BackgroundManagerLike | None = None
    session_id: str | None = None


def truncate_output(text: str, limit: int) -> str:
    """Cap *text* at *limit* characters, appending a truncation marker."""
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n...[output truncated: {len(text)} chars total]"


def resolve_or_fail(ctx: ToolContext, user_path: str) -> tuple[Path | None, ToolOutcome | None]:
    """Resolve *user_path* inside the workspace.

    Returns ``(path, None)`` on success or ``(None, failure_outcome)`` when the
    path escapes the workspace (or cannot be resolved at all), so traversal and
    symlink escapes become ordinary tool failures instead of exceptions.
    """
    try:
        return resolve_in_workspace(ctx.workspace, user_path), None
    except PathOutsideWorkspaceError as exc:
        return None, ToolOutcome.failure(f"path outside workspace: {exc}")
    except OSError as exc:
        return None, ToolOutcome.failure(f"invalid path: {exc}")


class BaseTool(ABC):
    """Common plumbing for all tools: schema generation and arg validation.

    Subclasses declare ``name``, ``description``, ``args_model`` (a pydantic
    model that doubles as the JSON-schema source and the validator) and
    implement :meth:`execute`.
    """

    name: ClassVar[str]
    description: ClassVar[str]
    requires_approval: ClassVar[bool] = False
    args_model: ClassVar[type[BaseModel]]

    def spec(self) -> ToolSpec:
        """Provider-facing tool description derived from the args model."""
        schema = self.args_model.model_json_schema()
        schema.pop("title", None)
        return ToolSpec(
            name=self.name,
            description=self.description,
            input_schema=schema,
            requires_approval=self.requires_approval,
        )

    async def run(self, raw_args: dict[str, Any], ctx: ToolContext) -> ToolOutcome:
        """Validate *raw_args* and dispatch to :meth:`execute`.

        Invalid arguments produce a failed outcome without ever executing the
        tool body.
        """
        try:
            args = self.args_model(**raw_args)
        except ValidationError as exc:
            return ToolOutcome.failure(f"invalid arguments for {self.name}: {exc}")
        return await self.execute(args, ctx)

    @abstractmethod
    async def execute(self, args: BaseModel, ctx: ToolContext) -> ToolOutcome:
        """Perform the tool's work with already-validated arguments."""
