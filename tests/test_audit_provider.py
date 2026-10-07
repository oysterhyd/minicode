"""Ensure malformed/flooded SSE streams are rejected before full capture."""
from __future__ import annotations

import asyncio
import json

import httpx
import pytest

from minicode.providers.commandcode import CommandCodeProvider
from minicode.providers.errors import ProviderProtocolError


class CountingStream(httpx.AsyncByteStream):
    def __init__(self, chunks):
        self.chunks = chunks
        self.consumed = 0
        self.closed = False

    async def __aiter__(self):
        for chunk in self.chunks:
            self.consumed += 1
            yield chunk

    async def aclose(self):
        self.closed = True


def tool_chunk(index, arguments):
    data = {"choices": [{"delta": {"tool_calls": [{"index": index, "id": f"call-{index}",
             "function": {"name": "write", "arguments": arguments}}]}}]}
    return ("data: " + json.dumps(data) + "\n\n").encode()


def rejected(stream, match):
    async def scenario():
        provider = CommandCodeProvider(base_url="https://offline.test/v1", api_key="test-key",
                                       transport=httpx.MockTransport(lambda _: httpx.Response(200, stream=stream)))
        try:
            with pytest.raises(ProviderProtocolError, match=match):
                _ = [event async for event in provider.stream(system=None, messages=[], tools=[])]
        finally:
            await provider.aclose()
        assert stream.closed
    asyncio.run(scenario())


def test_argument_only_capture_is_bounded_before_terminal_marker(monkeypatch):
    monkeypatch.setattr("minicode.providers.commandcode.MAX_RESPONSE_BYTES", 4096, raising=False)
    stream = CountingStream([tool_chunk(0, "x" * 1024) for _ in range(12)] + [b"data: [DONE]\n"])
    rejected(stream, "capture limit")
    assert stream.consumed <= 4


def test_129th_tool_is_rejected_before_stream_finishes():
    stream = CountingStream([tool_chunk(i, "{}") for i in range(150)] + [b"data: [DONE]\n"])
    rejected(stream, "too many tool calls")
    assert stream.consumed == 129


def test_single_unterminated_sse_line_is_bounded_while_reading(monkeypatch):
    monkeypatch.setattr("minicode.providers.commandcode.MAX_SSE_LINE_BYTES", 4096, raising=False)
    stream = CountingStream([b"data: " + b"x" * 1024] + [b"x" * 1024 for _ in range(10)])
    rejected(stream, "SSE line.*limit")
    assert stream.consumed <= 4


def test_reasoning_only_wire_capture_has_a_limit(monkeypatch):
    monkeypatch.setattr("minicode.providers.commandcode.MAX_SSE_BYTES", 2048, raising=False)
    data = {"choices": [{"delta": {"reasoning_content": "x" * 800}}]}
    chunk = ("data: " + json.dumps(data) + "\n\n").encode()
    stream = CountingStream([chunk] * 20)
    rejected(stream, "SSE stream.*limit")
    assert stream.consumed <= 3
