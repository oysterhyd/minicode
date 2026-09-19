"""Deterministic scripted provider used by tests and no-key demos.

``FakeProvider`` replays a list of :class:`FakeTurn` objects in order,
streaming each turn's text in fixed-size chunks and returning fully formed
tool calls. When the script runs out it falls back to an "exhausted" turn,
so a consumer looping against it always receives a well-formed
``ResponseDone`` instead of an error.
"""

from __future__ import annotations

from typing import Any, AsyncIterator

from pydantic import BaseModel, Field

from minicode.core.models import (
    Message,
    ModelResponse,
    StopReason,
    TextBlock,
    ToolSpec,
    ToolUseBlock,
    Usage,
)

from .base import ResponseDone, StreamEvent, TextDelta

#: Maximum characters streamed per ``TextDelta``.
_CHUNK_SIZE = 40


class FakeToolCall(BaseModel):
    """One scripted tool invocation requested by the fake model."""

    name: str
    arguments: dict[str, Any] = Field(default_factory=dict)
    # When omitted, auto-generated as "fake_tool_<n>" with a per-provider
    # monotonically increasing counter.
    id: str | None = None


class FakeTurn(BaseModel):
    """One scripted assistant turn."""

    text: str | None = None  # streamed as deltas
    tool_calls: list[FakeToolCall] = Field(default_factory=list)
    input_tokens: int | None = None  # deterministic usage override
    output_tokens: int | None = None
    # Default: TOOL_USE if tool_calls else END_TURN.
    stop_reason: StopReason | None = None


class FakeProviderOptions(BaseModel):
    """Configuration for a :class:`FakeProvider` instance."""

    turns: list[FakeTurn] = Field(default_factory=list)
    exhausted_text: str = "(fake provider: script exhausted)"
    default_input_tokens: int = 100
    default_output_tokens: int = 20


def _chunks(text: str, size: int = _CHUNK_SIZE) -> list[str]:
    """Slice *text* into ordered pieces of at most *size* characters."""
    return [text[i : i + size] for i in range(0, len(text), size)]


class FakeProvider:
    """Concrete scripted provider satisfying the :class:`Provider` contract."""

    name = "fake"

    def __init__(self, options: FakeProviderOptions | None = None) -> None:
        self.options = options if options is not None else FakeProviderOptions()
        self._index = 0
        self._tool_call_counter = 0

    @property
    def turns_consumed(self) -> int:
        """Number of scripted turns popped so far (read-only)."""
        return self._index

    def reset(self) -> None:
        """Rewind the script index so the next ``stream`` replays turn 0."""
        self._index = 0

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[Message],
        tools: list[ToolSpec],
    ) -> AsyncIterator[StreamEvent]:
        """Yield the scripted turn: zero or more ``TextDelta`` then one
        ``ResponseDone`` (exhausted-text fallback when the script is done)."""
        del system, messages, tools  # the fake provider ignores the request

        options = self.options
        if self._index < len(options.turns):
            turn = options.turns[self._index]
        else:
            turn = FakeTurn(text=options.exhausted_text)
        self._index += 1

        # Stream text as deterministic ~40-character deltas.
        if turn.text:
            for piece in _chunks(turn.text):
                yield TextDelta(piece)

        # Assemble the final response blocks.
        blocks: list[TextBlock | ToolUseBlock] = []
        if turn.text:
            blocks.append(TextBlock(text=turn.text))
        for call in turn.tool_calls:
            if call.id is None:
                self._tool_call_counter += 1
                tool_id = f"fake_tool_{self._tool_call_counter}"
            else:
                tool_id = call.id
            blocks.append(
                ToolUseBlock(id=tool_id, name=call.name, input=dict(call.arguments))
            )

        stop_reason = (
            turn.stop_reason
            if turn.stop_reason is not None
            else (StopReason.TOOL_USE if turn.tool_calls else StopReason.END_TURN)
        )
        usage = Usage(
            input_tokens=(
                turn.input_tokens
                if turn.input_tokens is not None
                else options.default_input_tokens
            ),
            output_tokens=(
                turn.output_tokens
                if turn.output_tokens is not None
                else options.default_output_tokens
            ),
        )

        yield ResponseDone(
            response=ModelResponse(blocks=blocks, stop_reason=stop_reason, usage=usage)
        )
