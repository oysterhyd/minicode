"""Tool base classes: shared limits, execution context and argument validation."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, ClassVar, Protocol, runtime_checkable

from pydantic import BaseModel, ValidationError

from minicode.core.models import ToolOutcome, ToolSpec
from minicode.core.paths import PathOutsideWorkspaceError, resolve_in_workspace


@dataclass(frozen=True, slots=True)
class ToolExecution:
    """Trusted local metadata, never inferred from a remote tool's name."""

    replay_safe: bool = False
    parallel_group: str | None = None
    attempts: int = 1
    timeout_s: float | None = 300.0


READ_EXECUTION = ToolExecution(replay_safe=True, parallel_group="read", attempts=2)
STATE_EXECUTION = ToolExecution(replay_safe=True)
DELEGATE_EXECUTION = ToolExecution(replay_safe=True, parallel_group="delegate", timeout_s=None)


class ToolLimits(BaseModel):
    """Resource caps applied by every built-in tool."""

    max_read_bytes: int = 256_000           # read cap
    max_output_chars: int = 50 * 1024       # UTF-8 page budget for read/ls/grep
    max_command_output_chars: int = 50 * 1024  # UTF-8 tail budget for bash
    max_command_capture_bytes: int = 100_000_000  # disk-backed log quota
    #: Fallback when a call site does not set an explicit default. The desktop
    #: bridge relies on this: an unbounded ``await`` on a wedged shell (a
    #: descendant holding the output pipe, a command that never exits) would
    #: otherwise freeze the whole turn with no way back except cancel.
    default_command_timeout_s: float | None = 300.0
    max_command_timeout_s: float | None = 3600.0
    max_search_results: int = 100
    search_max_file_bytes: int = 1_000_000


def utf8_prefix(text: str, max_bytes: int) -> str:
    """Return the longest UTF-8 prefix that fits without splitting a character."""
    if max_bytes <= 0:
        return ""
    raw = text.encode("utf-8")
    return raw[:max_bytes].decode("utf-8", errors="ignore")


def tail_output(text: str, max_bytes: int = 50 * 1024, max_lines: int = 2000) -> str:
    """Keep the end of command output within both line and UTF-8 byte limits."""
    lines = text.splitlines(keepends=True)
    tail = "".join(lines[-max_lines:])
    raw = tail.encode("utf-8")
    if len(raw) > max_bytes:
        tail = raw[-max_bytes:].decode("utf-8", errors="ignore")
    return tail


@runtime_checkable
class ArtifactStoreLike(Protocol):
    """What tools need from the session artifact store (see storage.artifacts)."""

    def spill(self, session_id: str, kind: str, content: str) -> Any: ...

    def read(self, session_id: str, artifact_id: str) -> str | None: ...

    def read_page(
        self, session_id: str, artifact_id: str, offset: int, limit: int
    ) -> tuple[str, int | None, bool] | None: ...

    async def spill_binary_stream(self, session_id: str, kind: str, source: Any) -> Any: ...


@runtime_checkable
class BackgroundManagerLike(Protocol):
    """What tools need from the background job manager (see tasks.background)."""

    def start(self, command: str, cwd: Path, timeout_s: float | None) -> Any: ...


@dataclass(slots=True)
class ToolContext:
    """Per-execution context handed to every tool invocation."""

    workspace: Path                          # absolute resolved workspace root
    limits: ToolLimits = field(default_factory=ToolLimits)
    # Optional services; None keeps tools self-contained in tests.
    artifact_store: ArtifactStoreLike | None = None
    background_manager: BackgroundManagerLike | None = None
    session_id: str | None = None
    activate_skill: Callable[[str], str] | None = None
    deactivate_skill: Callable[[str], str] | None = None
    delegate: Callable[[str, str], Awaitable[ToolOutcome]] | None = None
    task_store: Any | None = None
    project_memory: Any | None = None
    on_output: Callable[[str], Awaitable[None]] | None = None


def truncate_output(text: str, limit: int) -> str:
    """Cap *text* at *limit* characters, appending a truncation marker."""
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n...[output truncated: {len(text)} chars total]"


def bounded_output(text: str, limit: int, **updates: Any) -> ToolOutcome:
    """Build an outcome with a bounded preview and recoverable original.

    Tool implementations use this instead of throwing the tail away.  The
    runtime archives ``full_output`` before returning a smaller preview and
    artifact reference to the model.
    """
    preview = truncate_output(text, limit)
    full_output = text if preview != text else None
    return ToolOutcome(output=preview, full_output=full_output, **updates)


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
    execution: ClassVar[ToolExecution] = ToolExecution()
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

    def validate_args(self, raw_args: dict[str, Any]) -> dict[str, Any]:
        """Return the canonical arguments used for both approval and execution."""
        accepted = set(self.args_model.model_fields)
        accepted.update(field.alias for field in self.args_model.model_fields.values()
                        if isinstance(field.alias, str))
        unknown = set(raw_args) - accepted
        if unknown:
            raise ValueError(f"unknown argument fields: {', '.join(sorted(unknown))}")
        return self.args_model.model_validate(raw_args).model_dump(exclude_none=True, by_alias=True)

    async def run(self, raw_args: dict[str, Any], ctx: ToolContext) -> ToolOutcome:
        """Validate *raw_args* and dispatch to :meth:`execute`.

        Invalid arguments produce a failed outcome without ever executing the
        tool body.
        """
        try:
            args = self.args_model.model_validate(self.validate_args(raw_args))
        except (ValidationError, ValueError) as exc:
            return ToolOutcome.failure(f"invalid arguments for {self.name}: {exc}")
        return await self.execute(args, ctx)

    @abstractmethod
    async def execute(self, args: BaseModel, ctx: ToolContext) -> ToolOutcome:
        """Perform the tool's work with already-validated arguments."""
