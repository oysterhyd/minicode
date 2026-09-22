"""Tests for CommandCodeProvider (OpenAI chat-completions SSE over httpx).

Every network interaction is mocked with ``httpx.MockTransport``; no test
performs real I/O and none touches the real ``~/.zcode/v2/provider_config.json``
(an autouse fixture pins the environment variables and redirects ``Path.home``
into a temporary directory).
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any, AsyncIterator, Callable

import httpx
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
from minicode.providers.base import ResponseDone, StreamEvent, TextDelta
from minicode.providers.commandcode import (
    DEFAULT_MAX_TOKENS,
    CommandCodeProvider,
    _build_payload,
)
from minicode.providers.errors import (
    ProviderAuthError,
    ProviderError,
    ProviderRequestError,
)
from minicode.providers.zcode_config import DEFAULT_MODEL

GW_BASE = "https://gw.test/provider/v1"


# ---------------------------------------------------------------------------
# Isolation: never read the real ZCode config or real env credentials
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def _isolated_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("COMMANDCODE_API_KEY", "env-test-key")
    monkeypatch.setenv("COMMANDCODE_BASE_URL", "https://env-gw.test/v1")
    monkeypatch.setattr(Path, "home", lambda: tmp_path)


# ---------------------------------------------------------------------------
# Helpers: SSE response construction and stream driving
# ---------------------------------------------------------------------------


def sse_body(chunks: list[dict[str, Any]], *, done: bool = True) -> bytes:
    """Encode OpenAI-style SSE chunks into a response body."""
    lines: list[str] = []
    for chunk in chunks:
        lines.append("data: " + json.dumps(chunk))
        lines.append("")
    if done:
        lines.append("data: [DONE]")
        lines.append("")
    return "\n".join(lines).encode("utf-8")


def text_delta(piece: str) -> dict[str, Any]:
    return {"choices": [{"index": 0, "delta": {"content": piece}, "finish_reason": None}]}


def sse_response(chunks: list[dict[str, Any]], *, done: bool = True) -> httpx.Response:
    return httpx.Response(
        200,
        content=sse_body(chunks, done=done),
        headers={"content-type": "text/event-stream"},
    )


def make_provider(
    responder: Callable[[httpx.Request], httpx.Response], **kwargs: Any
) -> tuple[CommandCodeProvider, list[httpx.Request]]:
    """Build a provider backed by a MockTransport, recording every request."""
    captured: list[httpx.Request] = []

    def handler(request: httpx.Request) -> httpx.Response:
        captured.append(request)
        return responder(request)

    provider = CommandCodeProvider(
        base_url=GW_BASE,
        api_key="test-key",
        transport=httpx.MockTransport(handler),
        **kwargs,
    )
    return provider, captured


def run_stream(
    provider: CommandCodeProvider,
    *,
    system: str | None = "You are helpful.",
    messages: list[Message] | None = None,
    tools: list[ToolSpec] | None = None,
) -> list[StreamEvent]:
    async def collect() -> list[StreamEvent]:
        return [
            event
            async for event in provider.stream(
                system=system,
                messages=messages if messages is not None else [],
                tools=tools if tools is not None else [],
            )
        ]

    return asyncio.run(collect())


def base_user_message() -> list[Message]:
    return [Message(role="user", content=[TextBlock(text="hi")])]


# ---------------------------------------------------------------------------
# 1. Happy path: text deltas, usage, request shape
# ---------------------------------------------------------------------------


def test_stream_text_deltas_usage_and_request_shape() -> None:
    chunks = [
        # Reasoning models may lead with reasoning-only chunks (content=None).
        {"choices": [{"index": 0, "delta": {"content": None, "reasoning_content": "hmm"}}]},
        text_delta("Hello "),
        text_delta("world"),
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        # Final usage-only chunk (stream_options.include_usage).
        {"choices": [], "usage": {"prompt_tokens": 111, "completion_tokens": 22}},
    ]
    provider, captured = make_provider(lambda request: sse_response(chunks))
    tools = [ToolSpec(name="read_file", description="Read a file", input_schema={"type": "object"})]

    collected = run_stream(
        provider,
        messages=base_user_message(),
        tools=tools,
    )

    deltas = [e for e in collected if isinstance(e, TextDelta)]
    dones = [e for e in collected if isinstance(e, ResponseDone)]
    # Reasoning-only chunk must not surface as a TextDelta.
    assert [d.text for d in deltas] == ["Hello ", "world"]
    assert len(dones) == 1
    assert collected[-1] is dones[0]

    response = dones[0].response
    assert response.blocks == [TextBlock(text="Hello world")]
    assert response.stop_reason == StopReason.END_TURN
    assert response.usage == Usage(input_tokens=111, output_tokens=22)

    # The HTTP request itself: URL join, auth header, and body shape.
    assert len(captured) == 1
    request = captured[0]
    assert request.url.path == "/provider/v1/chat/completions"
    assert request.headers["authorization"] == "Bearer test-key"

    body = json.loads(request.content)
    assert body["model"] == DEFAULT_MODEL == "deepseek/deepseek-v4.1-flash"
    assert body["stream"] is True
    assert body["stream_options"] == {"include_usage": True}
    assert body["max_tokens"] == DEFAULT_MAX_TOKENS == 8192
    assert body["messages"] == [
        {"role": "system", "content": "You are helpful."},
        {"role": "user", "content": "hi"},
    ]
    assert body["tools"] == [
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object"},
            },
        }
    ]


def test_stream_without_system_or_tools_omits_them() -> None:
    provider, captured = make_provider(lambda request: sse_response([]))
    run_stream(provider, system=None, messages=base_user_message(), tools=[])

    body = json.loads(captured[0].content)
    assert body["messages"][0] == {"role": "user", "content": "hi"}
    assert "tools" not in body


def test_usage_is_unknown_when_gateway_omits_it() -> None:
    provider, _ = make_provider(lambda request: sse_response([text_delta("hi")]))
    collected = run_stream(provider, messages=base_user_message())

    done = [e for e in collected if isinstance(e, ResponseDone)][0]
    assert done.response.usage == Usage(available=False)


def test_stream_without_finish_reason_still_yields_done() -> None:
    # Gateway ends after [DONE] without ever setting finish_reason.
    chunks = [text_delta("tolerated")]
    provider, _ = make_provider(lambda request: sse_response(chunks))
    collected = run_stream(provider, messages=base_user_message())

    dones = [e for e in collected if isinstance(e, ResponseDone)]
    assert len(dones) == 1
    assert dones[0].response.blocks == [TextBlock(text="tolerated")]
    assert dones[0].response.stop_reason == StopReason.END_TURN


# ---------------------------------------------------------------------------
# 2. Tool calls: incremental aggregation
# ---------------------------------------------------------------------------


def test_tool_call_arguments_aggregated_from_incremental_chunks() -> None:
    chunks = [
        {"choices": [{"index": 0, "delta": {"content": None}}]},
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": "read_file", "arguments": '{"pa'},
                            }
                        ]
                    },
                }
            ]
        },
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {"index": 0, "function": {"arguments": 'th": "a.py"}'}}
                        ]
                    },
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
        {"choices": [], "usage": {"prompt_tokens": 7, "completion_tokens": 9}},
    ]
    provider, _ = make_provider(lambda request: sse_response(chunks))

    collected = run_stream(provider, messages=base_user_message())

    # No text -> zero TextDelta events (contract: zero or more).
    assert [e for e in collected if isinstance(e, TextDelta)] == []
    done = [e for e in collected if isinstance(e, ResponseDone)][0]
    response = done.response
    assert response.blocks == [
        ToolUseBlock(id="call_1", name="read_file", input={"path": "a.py"})
    ]
    assert response.stop_reason == StopReason.TOOL_USE
    assert response.usage == Usage(input_tokens=7, output_tokens=9)


def test_multiple_tool_calls_keep_index_order() -> None:
    chunks = [
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {"index": 1, "id": "call_b", "type": "function", "function": {"name": "bash", "arguments": "{}"}},
                            {"index": 0, "id": "call_a", "type": "function", "function": {"name": "read_file", "arguments": "{}"}},
                        ]
                    },
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]
    provider, _ = make_provider(lambda request: sse_response(chunks))
    collected = run_stream(provider, messages=base_user_message())

    done = [e for e in collected if isinstance(e, ResponseDone)][0]
    # tool_order follows the arrival order of each new index (0 then 1).
    assert [b.name for b in done.response.blocks] == ["bash", "read_file"]
    assert done.response.stop_reason == StopReason.TOOL_USE


def test_mixed_text_and_tool_calls() -> None:
    chunks = [
        text_delta("Let me check "),
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_9",
                                "type": "function",
                                "function": {"name": "grep", "arguments": '{"pattern": "x"}'},
                            }
                        ]
                    },
                }
            ]
        },
        text_delta("first."),
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]
    provider, _ = make_provider(lambda request: sse_response(chunks))
    collected = run_stream(provider, messages=base_user_message())

    assert [e.text for e in collected if isinstance(e, TextDelta)] == [
        "Let me check ",
        "first.",
    ]
    done = [e for e in collected if isinstance(e, ResponseDone)][0]
    assert done.response.blocks == [
        TextBlock(text="Let me check first."),
        ToolUseBlock(id="call_9", name="grep", input={"pattern": "x"}),
    ]
    assert done.response.stop_reason == StopReason.TOOL_USE


def test_malformed_tool_arguments_raise_request_error() -> None:
    chunks = [
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_x",
                                "type": "function",
                                "function": {"name": "f", "arguments": '{"broken": '},
                            }
                        ]
                    },
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]
    provider, _ = make_provider(lambda request: sse_response(chunks))

    with pytest.raises(ProviderRequestError, match="malformed tool call arguments"):
        run_stream(provider, messages=base_user_message())


def test_tool_arguments_decoding_to_non_object_raise_request_error() -> None:
    chunks = [
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_y",
                                "type": "function",
                                "function": {"name": "f", "arguments": "[1, 2]"},
                            }
                        ]
                    },
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]
    provider, _ = make_provider(lambda request: sse_response(chunks))

    with pytest.raises(ProviderRequestError, match="must decode to an object"):
        run_stream(provider, messages=base_user_message())


def test_malformed_sse_chunk_raises_provider_error() -> None:
    body = b"data: {not json at all}\n\ndata: [DONE]\n\n"
    provider, _ = make_provider(
        lambda request: httpx.Response(200, content=body, headers={"content-type": "text/event-stream"})
    )

    with pytest.raises(ProviderError, match="malformed SSE chunk"):
        run_stream(provider, messages=base_user_message())


# ---------------------------------------------------------------------------
# 3. HTTP status -> error hierarchy mapping
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("status", [401, 403])
def test_auth_statuses_map_to_provider_auth_error(status: int) -> None:
    provider, _ = make_provider(
        lambda request: httpx.Response(status, text='{"error": "invalid key"}')
    )

    with pytest.raises(ProviderAuthError, match=str(status)):
        run_stream(provider, messages=base_user_message())


@pytest.mark.parametrize("status", [400, 422])
def test_bad_request_statuses_map_to_provider_request_error(status: int) -> None:
    provider, _ = make_provider(
        lambda request: httpx.Response(status, text='{"error": "bad schema"}')
    )

    with pytest.raises(ProviderRequestError, match=str(status)):
        run_stream(provider, messages=base_user_message())


@pytest.mark.parametrize("status", [429, 500, 503])
def test_server_statuses_map_to_plain_provider_error(status: int) -> None:
    provider, _ = make_provider(
        lambda request: httpx.Response(status, text="gateway overloaded")
    )

    with pytest.raises(ProviderError) as excinfo:
        run_stream(provider, messages=base_user_message())
    # Exactly ProviderError (not a subclass): retryable transport-ish failure.
    assert type(excinfo.value) is ProviderError
    message = str(excinfo.value)
    assert str(status) in message
    assert "gateway overloaded" in message


def test_error_body_is_truncated_to_300_chars() -> None:
    provider, _ = make_provider(
        lambda request: httpx.Response(429, text="x" * 1000)
    )

    with pytest.raises(ProviderError) as excinfo:
        run_stream(provider, messages=base_user_message())
    assert "x" * 300 in str(excinfo.value)
    assert "x" * 301 not in str(excinfo.value)


def test_the_api_key_is_never_echoed_in_errors() -> None:
    provider, _ = make_provider(
        lambda request: httpx.Response(401, text="unauthorized")
    )

    with pytest.raises(ProviderAuthError) as excinfo:
        run_stream(provider, messages=base_user_message())
    assert "test-key" not in str(excinfo.value)
    assert "env-test-key" not in str(excinfo.value)


# ---------------------------------------------------------------------------
# 4. Transport failures
# ---------------------------------------------------------------------------


def test_handler_connect_error_maps_to_provider_error() -> None:
    def responder(request: httpx.Request) -> httpx.Response:
        raise httpx.ConnectError("connection refused", request=request)

    provider, _ = make_provider(responder)

    with pytest.raises(ProviderError, match="commandcode stream failed"):
        run_stream(provider, messages=base_user_message())


def test_mid_stream_read_error_maps_to_provider_error() -> None:
    async def stream_bytes() -> AsyncIterator[bytes]:
        yield sse_body([text_delta("partial")], done=False)
        raise httpx.ReadError("connection reset mid-stream")

    provider, _ = make_provider(
        lambda request: httpx.Response(
            200, content=stream_bytes(), headers={"content-type": "text/event-stream"}
        )
    )

    with pytest.raises(ProviderError) as excinfo:
        run_stream(provider, messages=base_user_message())
    assert "commandcode stream failed" in str(excinfo.value)
    assert "connection reset" in str(excinfo.value)


# ---------------------------------------------------------------------------
# 5. Message serialization: _build_payload unit tests
# ---------------------------------------------------------------------------


def test_build_payload_maps_conversation_in_order() -> None:
    messages = [
        Message(role="user", content=[TextBlock(text="hi")]),
        Message(
            role="assistant",
            content=[
                TextBlock(text="checking"),
                ToolUseBlock(id="call_1", name="read_file", input={"path": "a.py"}),
                ToolUseBlock(id="call_2", name="bash", input={"cmd": "ls"}),
            ],
        ),
        Message(
            role="user",
            content=[
                ToolResultBlock(tool_use_id="call_1", content="file body"),
                ToolResultBlock(tool_use_id="call_2", content="ls output", is_error=True),
                TextBlock(text="continue please"),
            ],
        ),
    ]
    tools = [ToolSpec(name="read_file", description="Read a file", input_schema={"type": "object"})]

    payload = _build_payload(model="m", system="sys prompt", messages=messages, tools=tools)

    assert payload["model"] == "m"
    assert payload["max_tokens"] == DEFAULT_MAX_TOKENS == 8192
    assert payload["stream"] is True
    assert payload["stream_options"] == {"include_usage": True}
    assert payload["tools"] == [
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file",
                "parameters": {"type": "object"},
            },
        }
    ]

    # Exact sequence: system, user, assistant(text+tool_calls merged),
    # then the two tool answers, then the trailing user text.
    assert payload["messages"] == [
        {"role": "system", "content": "sys prompt"},
        {"role": "user", "content": "hi"},
        {
            "role": "assistant",
            "content": "checking",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "read_file", "arguments": '{"path": "a.py"}'},
                },
                {
                    "id": "call_2",
                    "type": "function",
                    "function": {"name": "bash", "arguments": '{"cmd": "ls"}'},
                },
            ],
        },
        {"role": "tool", "tool_call_id": "call_1", "content": "file body"},
        {"role": "tool", "tool_call_id": "call_2", "content": "ls output"},
        {"role": "user", "content": "continue please"},
    ]


def test_build_payload_edge_cases() -> None:
    messages = [
        # Assistant with only tool calls -> content=None.
        Message(role="assistant", content=[ToolUseBlock(id="c1", name="t", input={})]),
        # Tool results without text -> no trailing user message.
        Message(role="user", content=[ToolResultBlock(tool_use_id="c1", content="ok")]),
    ]

    payload = _build_payload(model="m", system=None, messages=messages, tools=[])

    assert payload["messages"] == [
        {
            "role": "assistant",
            "content": None,
            "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "t", "arguments": "{}"}}
            ],
        },
        {"role": "tool", "tool_call_id": "c1", "content": "ok"},
    ]
    assert "tools" not in payload
    # system=None must not produce a system message.
    assert payload["messages"][0]["role"] == "assistant"


def test_build_payload_uses_ascii_safe_json_for_arguments() -> None:
    messages = [
        Message(
            role="assistant",
            content=[ToolUseBlock(id="c1", name="t", input={"路径": "文件.py"})],
        ),
    ]
    payload = _build_payload(model="m", system=None, messages=messages, tools=[])
    arguments = payload["messages"][0]["tool_calls"][0]["function"]["arguments"]
    assert json.loads(arguments) == {"路径": "文件.py"}
    # ensure_ascii=False: multibyte characters survive verbatim.
    assert "路径" in arguments


# ---------------------------------------------------------------------------
# 6. Construction: credential resolution and defaults
# ---------------------------------------------------------------------------


def test_provider_defaults_to_gateway_model_and_strips_base_url_slash() -> None:
    provider = CommandCodeProvider(base_url=GW_BASE + "/", api_key="k")
    assert provider.model == DEFAULT_MODEL
    assert provider.name == "commandcode"
    assert provider.base_url == GW_BASE
    assert provider.api_key == "k"


def test_provider_without_credentials_raises_request_error(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Isolation fixture already points Path.home at tmp_path; drop the env
    # credentials so neither discovery channel can succeed.
    monkeypatch.delenv("COMMANDCODE_API_KEY", raising=False)
    monkeypatch.delenv("COMMANDCODE_BASE_URL", raising=False)

    with pytest.raises(
        ProviderRequestError, match="COMMANDCODE_API_KEY 或本机 ZCode 配置"
    ):
        CommandCodeProvider()


# ---------------------------------------------------------------------------
# Reasoning effort and cache-token accounting
# ---------------------------------------------------------------------------


def usage_chunk(prompt: int, completion: int, cached: int | None = None) -> dict:
    usage: dict = {"prompt_tokens": prompt, "completion_tokens": completion}
    if cached is not None:
        usage["prompt_tokens_details"] = {"cached_tokens": cached}
    return {"choices": [], "usage": usage}


def test_reasoning_effort_included_in_payload_when_set():
    provider, captured = make_provider(
        lambda request: sse_response([text_delta("hi")]), reasoning_effort="low"
    )
    events = run_stream(provider, messages=base_user_message())
    assert events[-1].response.text == "hi"
    payload = json.loads(captured[0].content.decode("utf-8"))
    assert payload["reasoning_effort"] == "low"


@pytest.mark.parametrize("effort", [None, "off"])
def test_reasoning_effort_omitted_for_none_and_off(effort):
    provider, captured = make_provider(
        lambda request: sse_response([text_delta("hi")]), reasoning_effort=effort
    )
    run_stream(provider, messages=base_user_message())
    payload = json.loads(captured[0].content.decode("utf-8"))
    assert "reasoning_effort" not in payload


def test_default_effort_is_gateway_default():
    provider, captured = make_provider(lambda request: sse_response([text_delta("hi")]))
    run_stream(provider, messages=base_user_message())
    payload = json.loads(captured[0].content.decode("utf-8"))
    assert "reasoning_effort" not in payload


def test_cached_tokens_parsed_into_usage():
    provider, _ = make_provider(
        lambda request: sse_response([text_delta("hi"), usage_chunk(500, 20, cached=320)])
    )
    events = run_stream(provider, messages=base_user_message())
    usage = events[-1].response.usage
    assert usage.input_tokens == 500
    assert usage.output_tokens == 20
    assert usage.cache_read_tokens == 320
    assert usage.cache_hit_rate == 320 / 500


def test_missing_cache_details_count_as_zero():
    provider, _ = make_provider(
        lambda request: sse_response([text_delta("hi"), usage_chunk(100, 5)])
    )
    events = run_stream(provider, messages=base_user_message())
    assert events[-1].response.usage.cache_read_tokens == 0


def test_payload_max_tokens_overrides_the_default():
    """The request budget is the model's real output length, not a constant."""
    payload = _build_payload(model="m", system=None, messages=[], tools=[], max_tokens=384_000)
    assert payload["max_tokens"] == 384_000


def test_provider_threads_its_max_tokens_into_the_request(monkeypatch):
    """A catalog output cap reaches the wire instead of DEFAULT_MAX_TOKENS."""
    import httpx

    monkeypatch.setenv("COMMANDCODE_API_KEY", "k")
    monkeypatch.setenv("COMMANDCODE_BASE_URL", "https://gw.test/v1")
    seen: dict = {}

    def handler(request: httpx.Request) -> httpx.Response:
        seen.update(json.loads(request.content))
        return httpx.Response(200, content=b"data: [DONE]\n\n")

    provider = CommandCodeProvider(
        model="deepseek/deepseek-v4.1-flash",
        max_tokens=384_000,
        transport=httpx.MockTransport(handler),
    )

    async def drain() -> None:
        async for _event in provider.stream(system=None, messages=[], tools=[]):
            pass

    asyncio.run(drain())
    assert seen["max_tokens"] == 384_000
