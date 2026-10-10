"""CommandCode gateway adapter implementing the :class:`Provider` contract.

Talks the OpenAI ``chat/completions`` wire protocol — as served by the ZCode
gateway — over Server-Sent Events, using ``httpx`` directly (no SDK). The
gateway is OpenAI-compatible: reasoning-capable models may send leading
chunks that only carry reasoning content (``delta.content`` is ``None``);
such chunks are skipped.

Stream contract (see :mod:`minicode.providers.base`): zero or more
:class:`TextDelta` events, then exactly one :class:`ResponseDone` carrying a
fully assembled :class:`~minicode.core.models.ModelResponse`; nothing is
yielded afterwards, and failures raise a
:class:`~minicode.providers.errors.ProviderError` subclass instead of
returning a malformed response.

Credentials are resolved from explicit arguments, falling back to
:func:`minicode.providers.zcode_config.discover_commandcode` (environment
variables, then the machine-local ZCode config file). Keys are never
printed or logged.
"""

from __future__ import annotations

import asyncio
import json
import time
from typing import Any, AsyncIterator

import httpx

from minicode.core.limits import MAX_RESPONSE_BYTES, MAX_TOOL_CALLS, MAX_SSE_BYTES, MAX_SSE_LINE_BYTES
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
from minicode.context.estimate import estimate_messages_tokens
from minicode.core.catalog import lookup_model

from .base import ResponseDone, StreamEvent, TextDelta
from .errors import ProviderAuthError, ProviderError, ProviderRequestError, ProviderProtocolError
from .zcode_config import DEFAULT_MODEL, discover_commandcode

#: Per-request completion budget sent to the gateway.
DEFAULT_MAX_TOKENS = 8192

_MISSING_CREDENTIALS_MSG = "CommandCode provider 需要 COMMANDCODE_API_KEY 或本机 ZCode 配置"

_DATA_PREFIX = "data:"
_DONE_TOKEN = "[DONE]"

# OpenAI finish_reason -> our normalized StopReason (anything else -> END_TURN).
_FINISH_REASON_MAP = {
    "tool_calls": StopReason.TOOL_USE,
    "length": StopReason.MAX_TOKENS,
}


# ---------------------------------------------------------------------------
# Request serialization: our block model -> OpenAI chat-completions payload
# ---------------------------------------------------------------------------


def _assistant_text(message: Message) -> str:
    return "".join(b.text for b in message.content if isinstance(b, TextBlock))


def _build_payload(
    *,
    model: str,
    system: str | None,
    messages: list[Message],
    tools: list[ToolSpec],
    max_tokens: int = DEFAULT_MAX_TOKENS,
    reasoning_effort: str | None = None,
    prompt_scale: float = 1.0,
    context_window: int | None = None,
) -> dict[str, Any]:
    """Serialize the normalized conversation into an OpenAI-compatible
    request body.

    Mapping rules (order is preserved end to end, so ``tool`` messages stay
    adjacent to the ``assistant(tool_calls)`` message they answer — the
    runtime's message ordering already guarantees this):

    - ``system`` string -> a leading ``{"role": "system"}`` message (skipped
      when ``None``);
    - assistant ``TextBlock`` -> ``content``; ``ToolUseBlock`` -> entries in
      ``tool_calls``; both combined stay in one assistant message
      (``content=None`` when there are tool calls but no text);
    - user ``ToolResultBlock`` -> one ``{"role": "tool"}`` message each, and
      any ``TextBlock`` is emitted as a trailing ``user`` message after them.

    ``reasoning_effort`` (when set) is passed through as the OpenAI-style
    top-level field for effort-capable models; ``None`` / ``"off"`` omit it
    so the gateway applies its own default.
    """
    chat_messages: list[dict[str, Any]] = []
    if system:
        chat_messages.append({"role": "system", "content": system})

    for message in messages:
        if message.role == "assistant":
            text = _assistant_text(message)
            tool_calls = [
                {
                    "id": block.id,
                    "type": "function",
                    "function": {
                        "name": block.name,
                        "arguments": json.dumps(block.input, ensure_ascii=False),
                    },
                }
                for block in message.content
                if isinstance(block, ToolUseBlock)
            ]
            entry: dict[str, Any] = {"role": "assistant", "content": text if text else None}
            if tool_calls:
                entry["tool_calls"] = tool_calls
            chat_messages.append(entry)
        else:  # user
            tool_messages = [
                {
                    "role": "tool",
                    "tool_call_id": block.tool_use_id,
                    "content": block.content,
                }
                for block in message.content
                if isinstance(block, ToolResultBlock)
            ]
            chat_messages.extend(tool_messages)
            text = _assistant_text(message)
            if not tool_messages or text:
                chat_messages.append({"role": "user", "content": text})

    prompt_estimate = int(estimate_messages_tokens(
        system, messages, tools, reserve_output_tokens=0
    ) * prompt_scale)
    window = context_window or lookup_model(model).context_window
    available_output = max(1, window - prompt_estimate - 1024)
    payload: dict[str, Any] = {
        "model": model,
        "messages": chat_messages,
        "stream": True,
        "stream_options": {"include_usage": True},
        "max_tokens": min(max_tokens, available_output),
    }
    if reasoning_effort is not None and reasoning_effort != "off":
        payload["reasoning_effort"] = reasoning_effort
    if tools:
        payload["tools"] = [
            {
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                },
            }
            for tool in tools
        ]
    return payload


# ---------------------------------------------------------------------------
# Response deserialization helpers
# ---------------------------------------------------------------------------


def _to_int(value: Any) -> int:
    """Coerce a gateway usage field to ``int``; garbage counts as zero."""
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


async def _bounded_sse_lines(response: httpx.Response) -> AsyncIterator[tuple[str, float]]:
    """Bound raw transport and unterminated lines before JSON materialization."""
    pending = bytearray()
    captured = 0
    received = 0.0
    async for chunk in response.aiter_bytes():
        received = time.monotonic()
        captured += len(chunk)
        if captured > MAX_SSE_BYTES:
            raise ProviderProtocolError("SSE stream exceeds the capture limit")
        for start in range(0, len(chunk), 64 * 1024):
            # CRLF split across chunks may emit an extra blank line; SSE ignores it.
            parts = chunk[start:start + 64 * 1024].replace(b"\r", b"\n").split(b"\n")
            for index, part in enumerate(parts):
                if len(pending) + len(part) > MAX_SSE_LINE_BYTES:
                    raise ProviderProtocolError("SSE line exceeds the capture limit")
                pending.extend(part)
                if index < len(parts) - 1:
                    yield pending.decode("utf-8", errors="replace"), received
                    pending.clear()
    if pending:
        yield pending.decode("utf-8", errors="replace"), received


def _absorb_tool_call_delta(
    delta: dict[str, Any],
    buffers: dict[int, dict[str, Any]],
    order: list[int],
    remaining_bytes: int,
) -> int:
    """Fold one incremental ``delta.tool_calls`` entry into *buffers*.

    Entries are keyed by their ``index``; ``function.arguments`` arrives as
    incremental string fragments that must be concatenated.
    """
    index = delta.get("index")
    if not isinstance(index, int) or index < 0:
        index = len(order)
    if index not in buffers and len(buffers) >= MAX_TOOL_CALLS:
        raise ProviderProtocolError("provider returned too many tool calls")
    function = delta.get("function")
    function = function if isinstance(function, dict) else {}
    call_id, name, arguments = delta.get("id"), function.get("name"), function.get("arguments")
    size = sum(len(value.encode("utf-8")) for value in (call_id, name, arguments)
               if isinstance(value, str))
    if size > remaining_bytes:
        raise ProviderProtocolError("provider response exceeds the capture limit")
    buffer = buffers.get(index)
    if buffer is None:
        buffer = {"id": "", "name": "", "arguments": []}
        buffers[index] = buffer
        order.append(index)
    if isinstance(call_id, str) and call_id:
        buffer["id"] = call_id
    if isinstance(name, str) and name:
        buffer["name"] = name
    if isinstance(arguments, str) and arguments:
        buffer["arguments"].append(arguments)
    return size


# ---------------------------------------------------------------------------
# The provider
# ---------------------------------------------------------------------------


class CommandCodeProvider:
    """Streams one assistant turn from the ZCode (CommandCode) gateway.

    Satisfies the :class:`~minicode.providers.base.Provider` contract on top
    of the OpenAI ``chat/completions`` SSE protocol via ``httpx``.
    """

    name = "commandcode"

    def __init__(
        self,
        model: str = DEFAULT_MODEL,
        base_url: str | None = None,
        api_key: str | None = None,
        timeout_s: float = 180.0,
        transport: httpx.AsyncBaseTransport | None = None,
        reasoning_effort: str | None = None,
        max_tokens: int | None = None,
    ) -> None:
        """Create the provider.

        ``base_url``/``api_key`` fall back to
        :func:`~minicode.providers.zcode_config.discover_commandcode`
        (``COMMANDCODE_API_KEY``/``COMMANDCODE_BASE_URL`` environment
        variables, then the machine-local ZCode config) when omitted; if
        either credential is still missing, :class:`ProviderRequestError`
        is raised.

        ``reasoning_effort`` sets the reasoning budget for effort-capable
        models ("low" / "medium" / "high"; ``None`` or ``"off"`` means the
        gateway default) and can be changed live via the ``/effort``
        command. ``max_tokens`` is the response budget sent as the request's
        ``max_tokens``; ``None`` keeps :data:`DEFAULT_MAX_TOKENS`. Hosts pass
        the model's real output length from the catalog
        (:attr:`~minicode.core.catalog.ModelInfo.max_output_tokens`) so a long
        answer is not cut off by the harness; each request lowers it to fit
        the estimated remaining context window. ``transport`` is **test-only**:
        an optional ``httpx`` async transport (e.g. ``httpx.MockTransport``)
        injected so tests can run without any network access.
        """
        self.model = model
        self.timeout_s = timeout_s
        self.reasoning_effort = reasoning_effort
        self.max_tokens = DEFAULT_MAX_TOKENS if max_tokens is None else max_tokens
        self.prompt_scale = 1.0
        self._transport = transport
        self._client: httpx.AsyncClient | None = None
        self._client_loop: asyncio.AbstractEventLoop | None = None

        resolved_base = base_url
        resolved_key = api_key
        if resolved_base is None or resolved_key is None:
            discovered = discover_commandcode()
            if discovered is not None:
                if resolved_base is None:
                    resolved_base = discovered[0]
                if resolved_key is None:
                    resolved_key = discovered[1]
        if not resolved_base or not resolved_key:
            raise ProviderRequestError(_MISSING_CREDENTIALS_MSG)

        self.base_url = resolved_base.rstrip("/")
        self.api_key = resolved_key

    def _http_client(self) -> httpx.AsyncClient:
        """Reuse connections for requests made on the same event loop."""
        loop = asyncio.get_running_loop()
        if self._client is None or self._client.is_closed or self._client_loop is not loop:
            self._client = httpx.AsyncClient(
                base_url=self.base_url + "/",
                headers={"Authorization": f"Bearer {self.api_key}"},
                timeout=self.timeout_s,
                transport=self._transport,
            )
            self._client_loop = loop
        return self._client

    async def aclose(self) -> None:
        """Close the connection pool before its owning event loop exits."""
        if self._client is not None and not self._client.is_closed:
            await self._client.aclose()
        self._client = None
        self._client_loop = None

    # ------------------------------------------------------------------
    # Error mapping
    # ------------------------------------------------------------------

    @staticmethod
    def _map_http_status(status_code: int, body: str) -> ProviderError:
        """Turn an HTTP error response into the provider error hierarchy.

        The snippet only carries server-controlled body text (truncated);
        the API key is never echoed back.
        """
        snippet = body[:300]
        if status_code in (401, 403):
            return ProviderAuthError(f"commandcode auth failed ({status_code}): {snippet}")
        if 400 <= status_code < 500 and status_code not in (408, 429):
            return ProviderRequestError(
                f"commandcode rejected the request ({status_code}): {snippet}"
            )
        return ProviderError(f"commandcode HTTP {status_code}: {snippet}")

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
        """Stream one assistant turn: ``TextDelta`` events for streamed text,
        then exactly one terminal ``ResponseDone``.

        SSE chunks are parsed line by line (``data:`` prefixed lines,
        terminated by ``data: [DONE]``); tool calls arrive as incremental
        ``delta.tool_calls`` fragments aggregated by ``index``. A terminal
        marker or finish reason is required; a truncated transport is never
        mistaken for a complete response.
        """
        payload = _build_payload(
            model=self.model,
            system=system,
            messages=messages,
            tools=tools,
            max_tokens=self.max_tokens,
            reasoning_effort=self.reasoning_effort,
            prompt_scale=self.prompt_scale,
            context_window=getattr(self, "context_window", None),
        )
        estimated_input = estimate_messages_tokens(system, messages, tools,
                                                   reserve_output_tokens=0)

        text_parts: list[str] = []
        tool_buffers: dict[int, dict[str, Any]] = {}
        captured = 0
        tool_order: list[int] = []
        finish_reason: str | None = None
        usage = Usage(available=False)
        terminated = False
        started = time.monotonic()
        first_output = last_output = None

        try:
            # The host closes the provider at the end of its event-loop life.
            # Keeping this client alive lets successive model rounds reuse HTTP
            # connections (the CLI closes it after each asyncio.run turn).
            client = self._http_client()
            async with client.stream(
                    "POST", "/chat/completions", json=payload
                ) as response:
                    if response.status_code >= 400:
                        body = ""
                        async for part in response.aiter_bytes(chunk_size=4096):
                            body = part.decode("utf-8", errors="replace")
                            break
                        error = self._map_http_status(response.status_code, body.replace(self.api_key, "[redacted]"))
                        try:
                            retry_after = float(response.headers.get("retry-after", ""))
                            if 0 <= retry_after <= 300:
                                error.retry_after = retry_after
                        except ValueError:
                            pass
                        raise error

                    async for line, received in _bounded_sse_lines(response):
                        line = line.strip()
                        if not line.startswith(_DATA_PREFIX):
                            continue  # blank lines, SSE comments, "event:" lines
                        data = line[len(_DATA_PREFIX) :].strip()
                        if data == _DONE_TOKEN:
                            terminated = True
                            break
                        try:
                            chunk = json.loads(data)
                        except json.JSONDecodeError as exc:
                            raise ProviderProtocolError(
                                f"commandcode returned a malformed SSE chunk: {data[:200]!r}"
                            ) from exc
                        if not isinstance(chunk, dict):
                            continue

                        choices = chunk.get("choices")
                        if isinstance(choices, list) and choices:
                            choice = choices[0]
                            if isinstance(choice, dict):
                                delta = choice.get("delta")
                                if isinstance(delta, dict):
                                    if any(delta.get(key) for key in ("content", "reasoning_content", "reasoning", "tool_calls")):
                                        last_output = received
                                        if first_output is None:
                                            first_output = last_output
                                    # Reasoning-only chunks have content=None: skipped.
                                    content = delta.get("content")
                                    if isinstance(content, str) and content:
                                        captured += len(content.encode("utf-8"))
                                        if captured > MAX_RESPONSE_BYTES:
                                            raise ProviderProtocolError("provider response exceeds the capture limit")
                                        text_parts.append(content)
                                        yield TextDelta(content)
                                    tool_call_deltas = delta.get("tool_calls")
                                    if isinstance(tool_call_deltas, list):
                                        for tool_delta in tool_call_deltas:
                                            if isinstance(tool_delta, dict):
                                                captured += _absorb_tool_call_delta(
                                                    tool_delta, tool_buffers, tool_order,
                                                    MAX_RESPONSE_BYTES - captured,
                                                )
                                raw_finish = choice.get("finish_reason")
                                if isinstance(raw_finish, str) and raw_finish:
                                    finish_reason = raw_finish

                        raw_usage = chunk.get("usage")
                        if isinstance(raw_usage, dict) and raw_usage:
                            # The final chunk (with include_usage) carries token counts.
                            # prompt_tokens_details.cached_tokens reports the cache-served
                            # subset of the prompt; garbage counts as zero.
                            details = raw_usage.get("prompt_tokens_details")
                            cached = (
                                details.get("cached_tokens")
                                if isinstance(details, dict)
                                else None
                            )
                            cache_write = (
                                details.get("cache_creation_tokens")
                                if isinstance(details, dict)
                                else raw_usage.get("cache_creation_input_tokens")
                            )
                            usage = Usage(
                                input_tokens=_to_int(raw_usage.get("prompt_tokens")),
                                output_tokens=_to_int(raw_usage.get("completion_tokens")),
                                cache_read_tokens=max(0, _to_int(cached)),
                                cache_write_tokens=max(0, _to_int(cache_write)),
                                available=True,
                            )
        except httpx.HTTPError as exc:
            # Transport failures (connect/read errors, mid-stream breaks).
            # asyncio.CancelledError is a BaseException and propagates untouched.
            raise ProviderError(f"commandcode stream failed: {exc}") from exc

        if not terminated and finish_reason is None:
            raise ProviderProtocolError("commandcode stream ended before a completion marker")
        if finish_reason in {"content_filter", "refusal"}:
            raise ProviderRequestError(f"provider declined the response: {finish_reason}")

        if usage.available and usage.input_tokens > 0 and estimated_input > 0:
            observed = min(3.0, max(0.5, usage.input_tokens / estimated_input))
            self.prompt_scale = (self.prompt_scale + observed) / 2

        # Assemble the final response blocks.
        blocks: list[Block] = []
        text = "".join(text_parts)
        if text:
            blocks.append(TextBlock(text=text))
        for index in tool_order:
            buffer = tool_buffers[index]
            if not buffer["id"] or not buffer["name"]:
                raise ProviderProtocolError("tool call is missing its id or name")
            try:
                arguments = "".join(buffer["arguments"])
                parsed_input = json.loads(arguments) if arguments else {}
            except json.JSONDecodeError as exc:
                raise ProviderRequestError(
                    "commandcode returned malformed tool call arguments "
                    f"(index {index}, name {buffer['name']!r}): {exc}"
                ) from exc
            if not isinstance(parsed_input, dict):
                raise ProviderRequestError(
                    "commandcode tool call arguments must decode to an object "
                    f"(index {index}, got {type(parsed_input).__name__})"
                )
            blocks.append(
                ToolUseBlock(
                    id=buffer["id"],
                    name=buffer["name"],
                    input=parsed_input,
                )
            )

        stop_reason = (
            _FINISH_REASON_MAP.get(finish_reason, StopReason.END_TURN)
            if finish_reason is not None
            else StopReason.END_TURN
        )
        yield ResponseDone(
            response=ModelResponse(blocks=blocks, stop_reason=stop_reason, usage=usage),
            generation_seconds=last_output - first_output if first_output is not None and last_output > first_output else None,
            first_token_seconds=first_output - started if first_output is not None else None,
        )
