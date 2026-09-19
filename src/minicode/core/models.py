"""Shared data contracts for minicode.

Every module (providers, tools, runtime, storage, cli) builds on these models.
Keep this file dependency-free except for pydantic: it is the stable interface
between independently developed components.
"""

from __future__ import annotations

import enum
from typing import Annotated, Any, Awaitable, Callable, Literal, Union

from pydantic import BaseModel, Field

# ---------------------------------------------------------------------------
# Conversation messages (provider-facing, Anthropic-style block model)
# ---------------------------------------------------------------------------

Role = Literal["user", "assistant"]


class TextBlock(BaseModel):
    type: Literal["text"] = "text"
    text: str


class ToolUseBlock(BaseModel):
    """A tool invocation requested by the model."""

    type: Literal["tool_use"] = "tool_use"
    id: str
    name: str
    input: dict[str, Any] = Field(default_factory=dict)


class ToolResultBlock(BaseModel):
    """The runtime's answer to one ToolUseBlock, fed back to the model."""

    type: Literal["tool_result"] = "tool_result"
    tool_use_id: str
    content: str
    is_error: bool = False


Block = Annotated[
    Union[TextBlock, ToolUseBlock, ToolResultBlock],
    Field(discriminator="type"),
]


class Message(BaseModel):
    role: Role
    content: list[Block]


# ---------------------------------------------------------------------------
# Model responses and usage
# ---------------------------------------------------------------------------


class Usage(BaseModel):
    input_tokens: int = 0
    output_tokens: int = 0

    @property
    def total_tokens(self) -> int:
        return self.input_tokens + self.output_tokens

    def __add__(self, other: "Usage") -> "Usage":
        return Usage(
            input_tokens=self.input_tokens + other.input_tokens,
            output_tokens=self.output_tokens + other.output_tokens,
        )


class StopReason(str, enum.Enum):
    END_TURN = "end_turn"
    TOOL_USE = "tool_use"
    MAX_TOKENS = "max_tokens"


class ModelResponse(BaseModel):
    """Normalized provider response: text and/or tool calls plus usage."""

    blocks: list[Block] = Field(default_factory=list)
    stop_reason: StopReason = StopReason.END_TURN
    usage: Usage = Field(default_factory=Usage)

    @property
    def text(self) -> str:
        return "".join(b.text for b in self.blocks if isinstance(b, TextBlock))

    @property
    def tool_calls(self) -> list[ToolUseBlock]:
        return [b for b in self.blocks if isinstance(b, ToolUseBlock)]


# ---------------------------------------------------------------------------
# Tools
# ---------------------------------------------------------------------------


class ToolSpec(BaseModel):
    """Provider-facing tool description (JSON Schema based)."""

    name: str
    description: str
    input_schema: dict[str, Any]
    requires_approval: bool = False


class ToolOutcome(BaseModel):
    """What a tool execution produced; the runtime wraps it into a
    ToolResultBlock keyed by the original call id."""

    success: bool = True
    output: str = ""
    error: str | None = None
    exit_code: int | None = None

    @classmethod
    def failure(cls, error: str, output: str = "", exit_code: int | None = None) -> "ToolOutcome":
        return cls(success=False, output=output, error=error, exit_code=exit_code)


# ---------------------------------------------------------------------------
# Permissions / approval
# ---------------------------------------------------------------------------


class ApprovalRequest(BaseModel):
    """Shown to the user before a sensitive tool runs."""

    tool_name: str
    arguments: dict[str, Any]
    summary: str


class ApprovalDecision(BaseModel):
    granted: bool
    reason: str | None = None


ApprovalHandler = Callable[[ApprovalRequest], Awaitable[ApprovalDecision]]


# ---------------------------------------------------------------------------
# Events (append-only execution trace, persisted per session)
# ---------------------------------------------------------------------------


class EventType(str, enum.Enum):
    SESSION_START = "session_start"
    ROUND_START = "round_start"
    ASSISTANT_MESSAGE = "assistant_message"
    TOOL_CALL_START = "tool_call_start"
    TOOL_CALL_RESULT = "tool_call_result"
    APPROVAL_REQUEST = "approval_request"
    APPROVAL_DECISION = "approval_decision"
    ROUND_END = "round_end"
    SESSION_END = "session_end"


class Event(BaseModel):
    seq: int
    type: EventType
    timestamp: str
    data: dict[str, Any] = Field(default_factory=dict)


# ---------------------------------------------------------------------------
# Session lifecycle: budgets, exit reasons, results
# ---------------------------------------------------------------------------


class ExitReason(str, enum.Enum):
    COMPLETED = "completed"          # model finished with a final answer
    MAX_ROUNDS = "max_rounds"        # round budget exhausted
    TOKEN_BUDGET = "token_budget"    # cumulative token budget exhausted
    TIME_BUDGET = "time_budget"      # wall-clock budget exhausted
    CANCELLED = "cancelled"          # user interrupted (Ctrl+C)
    PROVIDER_ERROR = "provider_error"
    INTERNAL_ERROR = "internal_error"


class Budget(BaseModel):
    max_rounds: int = 20
    max_total_tokens: int = 200_000
    max_seconds: float = 600.0


class RunResult(BaseModel):
    session_id: str
    exit_reason: ExitReason
    rounds: int = 0
    total_usage: Usage = Field(default_factory=Usage)
    duration_s: float = 0.0
