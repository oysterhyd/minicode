"""Anthropic adapter implementing the :class:`Provider` contract on top of
the official ``anthropic`` SDK's streaming API.

The SDK is imported lazily (inside ``__init__``/``stream``) so this module
imports cleanly even when the ``anthropic`` package is not installed.
"""

from __future__ import annotations

from typing import Any, AsyncIterator

from minicode.core.models import (
    Block,
    Message,
    ModelResponse,
    StopReason,
    TextBlock,
    ToolResultBlock,
    ToolSpec,
    ToolUseBlock,
    Usage,
)

from .base import ResponseDone, StreamEvent, TextDelta
from .errors import ProviderAuthError, ProviderError, ProviderRequestError

# Anthropic stop_reason -> our normalized StopReason (anything else -> END_TURN).
_STOP_REASON_MAP = {
    "tool_use": StopReason.TOOL_USE,
    "max_tokens": StopReason.MAX_TOKENS,
}


class AnthropicProvider:
    """Streams one assistant turn from the Anthropic Messages API."""

    name = "anthropic"

    def __init__(
        self,
        *,
        model: str,
        api_key: str | None = None,
        client: Any | None = None,
        max_tokens: int = 4096,
    ) -> None:
        self.model = model
        self.api_key = api_key
        self.max_tokens = max_tokens
        if client is None:
            # Lazy import: keeps minicode usable without the SDK installed.
            # api_key=None lets the SDK resolve credentials from the environment.
            import anthropic

            self._client: Any = anthropic.AsyncAnthropic(api_key=api_key)
        else:
            self._client = client  # injected (tests / custom transports)

    # ------------------------------------------------------------------
    # Serialization: our block model -> Anthropic request dicts
    # ------------------------------------------------------------------

    @staticmethod
    def _serialize_block(block: Block) -> dict[str, Any]:
        if isinstance(block, ToolUseBlock):
            return {
                "type": "tool_use",
                "id": block.id,
                "name": block.name,
                "input": block.input,
            }
        if isinstance(block, ToolResultBlock):
            return {
                "type": "tool_result",
                "tool_use_id": block.tool_use_id,
                "content": block.content,
                "is_error": block.is_error,
            }
        return {"type": "text", "text": block.text}

    def _serialize_messages(self, messages: list[Message]) -> list[dict[str, Any]]:
        return [
            {
                "role": message.role,
                "content": [self._serialize_block(b) for b in message.content],
            }
            for message in messages
        ]

    @staticmethod
    def _serialize_tools(tools: list[ToolSpec]) -> list[dict[str, Any]]:
        return [
            {
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            }
            for tool in tools
        ]

    # ------------------------------------------------------------------
    # Deserialization: SDK response objects -> our ModelResponse
    # ------------------------------------------------------------------

    @staticmethod
    def _map_blocks(content: Any) -> list[Block]:
        blocks: list[Block] = []
        for raw in content or []:
            block_type = getattr(raw, "type", None)
            if block_type == "text":
                blocks.append(TextBlock(text=getattr(raw, "text", "")))
            elif block_type == "tool_use":
                raw_input = getattr(raw, "input", None)
                blocks.append(
                    ToolUseBlock(
                        id=getattr(raw, "id", ""),
                        name=getattr(raw, "name", ""),
                        input=dict(raw_input) if raw_input else {},
                    )
                )
            # Other block types (e.g. thinking) are ignored for P0.
        return blocks

    @staticmethod
    def _map_usage(response: Any) -> Usage:
        raw_usage = getattr(response, "usage", None)
        return Usage(
            input_tokens=int(getattr(raw_usage, "input_tokens", 0) or 0),
            output_tokens=int(getattr(raw_usage, "output_tokens", 0) or 0),
        )

    # ------------------------------------------------------------------
    # Streaming
    # ------------------------------------------------------------------

    async def stream(
        self,
        *,
        system: str | None,
        messages: list[Message],
        tools: list[ToolSpec],
    ) -> AsyncIterator[StreamEvent]:
        import anthropic  # lazy: needed here for the exception classes

        request_kwargs: dict[str, Any] = {
            "model": self.model,
            "messages": self._serialize_messages(messages),
            "tools": self._serialize_tools(tools),
            "max_tokens": self.max_tokens,
        }
        if system is not None:
            request_kwargs["system"] = system

        try:
            async with self._client.messages.stream(**request_kwargs) as stream:
                async for event in stream:
                    if (
                        event.type == "content_block_delta"
                        and event.delta.type == "text_delta"
                    ):
                        yield TextDelta(event.delta.text)
                response = await stream.get_final_message()
        except (anthropic.AuthenticationError, anthropic.PermissionDeniedError) as exc:
            raise ProviderAuthError(f"anthropic auth failed: {exc}") from exc
        except anthropic.BadRequestError as exc:
            raise ProviderRequestError(f"anthropic rejected the request: {exc}") from exc
        except anthropic.APIError as exc:
            raise ProviderError(f"anthropic API error: {exc}") from exc

        raw_stop = getattr(response, "stop_reason", None)
        yield ResponseDone(
            response=ModelResponse(
                blocks=self._map_blocks(getattr(response, "content", None)),
                stop_reason=_STOP_REASON_MAP.get(raw_stop, StopReason.END_TURN),
                usage=self._map_usage(response),
            )
        )
