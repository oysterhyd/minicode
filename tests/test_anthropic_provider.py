"""Tests for AnthropicProvider using a stub SDK client (no network, no key).

The stub mimics the official SDK surface the provider relies on:
``client.messages.stream(**kwargs)`` returns an async context manager that
is also an async iterator of stream events and offers ``get_final_message()``.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace
from typing import Any

import anthropic
import pytest

from minicode.core.models import (
    Message,
    StopReason,
    TextBlock,
    ToolResultBlock,
    ToolSpec,
    ToolUseBlock,
    Usage,
)
from minicode.providers import (
    AnthropicProvider,
    ProviderAuthError,
    ProviderError,
    ProviderRequestError,
    ResponseDone,
    StreamEvent,
    TextDelta,
)


# ---------------------------------------------------------------------------
# Stub SDK client
# ---------------------------------------------------------------------------


class StubStream:
    """Async context manager + async iterator standing in for a MessageStream."""

    def __init__(
        self,
        events: list[Any],
        final_message: Any,
        enter_error: Exception | None = None,
    ) -> None:
        self._events = list(events)
        self._final = final_message
        self._enter_error = enter_error

    async def __aenter__(self) -> "StubStream":
        if self._enter_error is not None:
            raise self._enter_error
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        return False

    def __aiter__(self) -> Any:
        async def _gen() -> Any:
            for event in self._events:
                yield event

        return _gen()

    async def get_final_message(self) -> Any:
        return self._final


class StubMessages:
    """Records the kwargs of every ``stream(...)`` call."""

    def __init__(self, client: "StubClient") -> None:
        self._client = client
        self.last_kwargs: dict[str, Any] | None = None

    def stream(self, **kwargs: Any) -> StubStream:
        self.last_kwargs = kwargs
        return StubStream(
            events=self._client.events,
            final_message=self._client.final_message,
            enter_error=self._client.enter_error,
        )


class StubClient:
    def __init__(
        self,
        events: list[Any],
        final_message: Any,
        enter_error: Exception | None = None,
    ) -> None:
        self.events = events
        self.final_message = final_message
        self.enter_error = enter_error
        self.messages = StubMessages(self)


def make_provider(
    events: list[Any], final_message: Any, enter_error: Exception | None = None
) -> tuple[AnthropicProvider, StubClient]:
    stub = StubClient(events=events, final_message=final_message, enter_error=enter_error)
    provider = AnthropicProvider(model="claude-test-model", client=stub, max_tokens=512)
    return provider, stub


async def collect(provider: AnthropicProvider, **kwargs: Any) -> list[StreamEvent]:
    return [e async for e in provider.stream(**kwargs)]


def run_stream(provider: AnthropicProvider, **kwargs: Any) -> list[StreamEvent]:
    return asyncio.run(collect(provider, **kwargs))


# ---------------------------------------------------------------------------
# Test 1: happy path — deltas, final mapping, and request serialization
# ---------------------------------------------------------------------------


def test_stream_maps_deltas_response_and_request_kwargs() -> None:
    events = [
        SimpleNamespace(
            type="content_block_delta",
            delta=SimpleNamespace(type="text_delta", text="Hello "),
        ),
        SimpleNamespace(
            type="content_block_delta",
            delta=SimpleNamespace(type="text_delta", text="world"),
        ),
        SimpleNamespace(type="message_start"),  # non-delta event -> ignored
    ]
    final_message = SimpleNamespace(
        content=[
            SimpleNamespace(type="text", text="Hello world"),
            SimpleNamespace(
                type="tool_use", id="toolu_1", name="read_file", input={"path": "a.py"}
            ),
        ],
        stop_reason="tool_use",
        usage=SimpleNamespace(input_tokens=12, output_tokens=34),
    )
    provider, stub = make_provider(events, final_message)

    messages = [
        Message(role="user", content=[TextBlock(text="read it")]),
        Message(
            role="assistant",
            content=[ToolUseBlock(id="toolu_1", name="read_file", input={"path": "a.py"})],
        ),
        Message(
            role="user",
            content=[ToolResultBlock(tool_use_id="toolu_1", content="file contents")],
        ),
    ]
    tools = [
        ToolSpec(name="read_file", description="Read a file", input_schema={"type": "object"})
    ]

    collected = run_stream(provider, system="You are helpful.", messages=messages, tools=tools)

    # Streamed deltas come first, exactly one ResponseDone at the end.
    ds = [e for e in collected if isinstance(e, TextDelta)]
    dns = [e for e in collected if isinstance(e, ResponseDone)]
    assert [d.text for d in ds] == ["Hello ", "world"]
    assert len(dns) == 1
    assert collected[-1] is dns[0]

    response = dns[0].response
    assert response.blocks == [
        TextBlock(text="Hello world"),
        ToolUseBlock(id="toolu_1", name="read_file", input={"path": "a.py"}),
    ]
    assert response.stop_reason == StopReason.TOOL_USE
    assert response.usage == Usage(input_tokens=12, output_tokens=34)

    # The request reached the SDK with our arguments serialized correctly.
    kwargs = stub.messages.last_kwargs
    assert kwargs is not None
    assert kwargs["model"] == "claude-test-model"
    assert kwargs["system"] == "You are helpful."
    assert kwargs["max_tokens"] == 512
    assert kwargs["tools"] == [
        {
            "name": "read_file",
            "description": "Read a file",
            "input_schema": {"type": "object"},
        }
    ]
    assert kwargs["messages"][0]["content"] == [{"type": "text", "text": "read it"}]
    assert kwargs["messages"][1]["content"] == [
        {"type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {"path": "a.py"}}
    ]
    # ToolResultBlock -> Anthropic tool_result dict.
    assert kwargs["messages"][2]["content"] == [
        {
            "type": "tool_result",
            "tool_use_id": "toolu_1",
            "content": "file contents",
            "is_error": False,
        }
    ]


def test_tool_result_is_error_flag_serialized() -> None:
    provider, stub = make_provider(events=[], final_message=SimpleNamespace(content=[]))
    messages = [
        Message(
            role="user",
            content=[
                ToolResultBlock(tool_use_id="t9", content="boom", is_error=True),
                TextBlock(text="continue"),
            ],
        )
    ]

    run_stream(provider, system=None, messages=messages, tools=[])

    kwargs = stub.messages.last_kwargs
    assert kwargs is not None
    assert kwargs["messages"][0]["content"] == [
        {"type": "tool_result", "tool_use_id": "t9", "content": "boom", "is_error": True},
        {"type": "text", "text": "continue"},
    ]


# ---------------------------------------------------------------------------
# Test 2: SDK errors map onto the provider error hierarchy
# ---------------------------------------------------------------------------


def _sdk_error(error_cls: Any, status_code: int, message: str) -> Any:
    """Build an SDK status error without a real HTTP exchange."""
    response = SimpleNamespace(
        status_code=status_code, headers={}, request=SimpleNamespace()
    )
    return error_cls(message, response=response, body=None)


def test_authentication_error_maps_to_provider_auth_error() -> None:
    error = _sdk_error(anthropic.AuthenticationError, 401, "invalid x-api-key")
    provider, _ = make_provider(
        events=[], final_message=SimpleNamespace(content=[]), enter_error=error
    )

    with pytest.raises(ProviderAuthError):
        run_stream(provider, system=None, messages=[], tools=[])


def test_permission_denied_error_maps_to_provider_auth_error() -> None:
    error = _sdk_error(anthropic.PermissionDeniedError, 403, "not allowed")
    provider, _ = make_provider(
        events=[], final_message=SimpleNamespace(content=[]), enter_error=error
    )

    with pytest.raises(ProviderAuthError):
        run_stream(provider, system=None, messages=[], tools=[])


def test_bad_request_error_maps_to_provider_request_error() -> None:
    error = _sdk_error(anthropic.BadRequestError, 400, "max_tokens too large")
    provider, _ = make_provider(
        events=[], final_message=SimpleNamespace(content=[]), enter_error=error
    )

    with pytest.raises(ProviderRequestError):
        run_stream(provider, system=None, messages=[], tools=[])


def test_generic_api_error_maps_to_provider_error() -> None:
    error = anthropic.APIError("connection exploded", SimpleNamespace(), body=None)
    provider, _ = make_provider(
        events=[], final_message=SimpleNamespace(content=[]), enter_error=error
    )

    with pytest.raises(ProviderError) as excinfo:
        run_stream(provider, system=None, messages=[], tools=[])
    # ProviderError base itself (not accidentally a subclass like Auth/Request).
    assert type(excinfo.value) is ProviderError


def test_unexpected_non_sdk_exception_propagates() -> None:
    provider, _ = make_provider(
        events=[], final_message=SimpleNamespace(content=[]), enter_error=RuntimeError("boom")
    )

    with pytest.raises(RuntimeError):
        run_stream(provider, system=None, messages=[], tools=[])


# ---------------------------------------------------------------------------
# Test 3: stop_reason and usage fallbacks
# ---------------------------------------------------------------------------


def test_end_turn_and_missing_usage_is_unknown() -> None:
    # Final message has no `usage` attribute at all -> getattr defaults kick in.
    final_message = SimpleNamespace(
        content=[SimpleNamespace(type="text", text="all done")],
        stop_reason="end_turn",
    )
    provider, _ = make_provider(events=[], final_message=final_message)

    collected = run_stream(provider, system=None, messages=[], tools=[])

    response = [e for e in collected if isinstance(e, ResponseDone)][0].response
    assert response.stop_reason == StopReason.END_TURN
    assert response.usage == Usage(input_tokens=0, output_tokens=0, available=False)


def test_max_tokens_stop_reason_maps() -> None:
    final_message = SimpleNamespace(
        content=[SimpleNamespace(type="text", text="cut off")],
        stop_reason="max_tokens",
        usage=SimpleNamespace(input_tokens=1, output_tokens=2),
    )
    provider, _ = make_provider(events=[], final_message=final_message)

    collected = run_stream(provider, system=None, messages=[], tools=[])

    response = [e for e in collected if isinstance(e, ResponseDone)][0].response
    assert response.stop_reason == StopReason.MAX_TOKENS
    assert response.usage == Usage(input_tokens=1, output_tokens=2)


def test_stop_sequence_maps_to_end_turn() -> None:
    final_message = SimpleNamespace(
        content=[], stop_reason="stop_sequence", usage=SimpleNamespace()
    )
    provider, _ = make_provider(events=[], final_message=final_message)

    collected = run_stream(provider, system=None, messages=[], tools=[])

    response = [e for e in collected if isinstance(e, ResponseDone)][0].response
    assert response.stop_reason == StopReason.END_TURN
    assert response.usage == Usage()
