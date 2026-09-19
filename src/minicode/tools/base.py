"""Tool base classes: shared limits, execution context and argument validation."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, ClassVar

from pydantic import BaseModel, ValidationError

from minicode.core.models import ToolOutcome, ToolSpec


class ToolLimits(BaseModel):
    """Resource caps applied by every built-in tool."""

    max_read_bytes: int = 256_000           # read_file cap
    max_output_chars: int = 20_000          # list_files / search output cap
    max_command_output_chars: int = 10_000  # run_command output cap
    default_command_timeout_s: float = 60.0
    max_command_timeout_s: float = 300.0
    max_search_results: int = 100
    search_max_file_bytes: int = 1_000_000


@dataclass(slots=True)
class ToolContext:
    """Per-execution context handed to every tool invocation."""

    workspace: Path                          # absolute resolved workspace root
    limits: ToolLimits = field(default_factory=ToolLimits)


def truncate_output(text: str, limit: int) -> str:
    """Cap *text* at *limit* characters, appending a truncation marker."""
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n...[output truncated: {len(text)} chars total]"


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
