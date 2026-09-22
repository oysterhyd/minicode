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

import json
from typing import Any, AsyncIterator

import httpx

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

    payload: dict[str, Any] = {
        "model": model,
        "messages": chat_messages,
        "stream": True,
        "stream_options": {"include_usage": True},
        "max_tokens": max_tokens,
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


def _absorb_tool_call_delta(
    delta: dict[str, Any],
    buffers: dict[int, dict[str, str]],
    order: list[int],
) -> None:
    """Fold one incremental ``delta.tool_calls`` entry into *buffers*.

    Entries are keyed by their ``index``; ``function.arguments`` arrives as
    incremental string fragments that must be concatenated.
    """
    index = delta.get("index")
    if not isinstance(index, int) or index < 0:
        index = len(order)
    buffer = buffers.get(index)
    if buffer is None:
        buffer = {"id": "", "name": "", "arguments": ""}
        buffers[index] = buffer
        order.append(index)

    call_id = delta.get("id")
    if isinstance(call_id, str) and call_id:
        buffer["id"] = call_id
    function = delta.get("function")
    if isinstance(function, dict):
        name = function.get("name")
        if isinstance(name, str) and name:
            buffer["name"] = name
        arguments = function.get("arguments")
        if isinstance(arguments, str) and arguments:
            buffer["arguments"] += arguments


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
        answer is not cut off by the harness. ``transport`` is **test-only**:
        an optional ``httpx`` async transport (e.g. ``httpx.MockTransport``)
        injected so tests can run without any network access.
        """
        self.model = model
        self.timeout_s = timeout_s
        self.reasoning_effort = reasoning_effort
        self.max_tokens = DEFAULT_MAX_TOKENS if max_tokens is None else max_tokens
        self._transport = transport

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
        if status_code in (400, 422):
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
        ``delta.tool_calls`` fragments aggregated by ``index``. If the
        gateway ends the stream without a ``finish_reason``, the aggregated
        response is still yielded (``stop_reason`` defaults to
        ``END_TURN``) to tolerate gateway differences.
        """
        payload = _build_payload(
            model=self.model,
            system=system,
            messages=messages,
            tools=tools,
            max_tokens=self.max_tokens,
            reasoning_effort=self.reasoning_effort,
        )

        text_parts: list[str] = []
        tool_buffers: dict[int, dict[str, str]] = {}
        tool_order: list[int] = []
        finish_reason: str | None = None
        usage = Usage(available=False)

        try:
            # NOTE: base_url gets a trailing "/" so httpx merges the relative
            # request path into "<base>/chat/completions" instead of
            # concatenating raw paths (".../v1" + "chat/completions").
            async with httpx.AsyncClient(
                base_url=self.base_url + "/",
                headers={"Authorization": f"Bearer {self.api_key}"},
                timeout=self.timeout_s,
                transport=self._transport,
            ) as client:
                async with client.stream(
                    "POST", "/chat/completions", json=payload
                ) as response:
                    if response.status_code >= 400:
                        body = (await response.aread()).decode("utf-8", errors="replace")
                        raise self._map_http_status(response.status_code, body)

                    async for line in response.aiter_lines():
                        line = line.strip()
                        if not line.startswith(_DATA_PREFIX):
                            continue  # blank lines, SSE comments, "event:" lines
                        data = line[len(_DATA_PREFIX) :].strip()
                        if data == _DONE_TOKEN:
                            break
                        try:
                            chunk = json.loads(data)
                        except json.JSONDecodeError as exc:
                            raise ProviderError(
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
                                    # Reasoning-only chunks have content=None: skipped.
                                    content = delta.get("content")
                                    if isinstance(content, str) and content:
                                        text_parts.append(content)
                                        yield TextDelta(content)
                                    tool_call_deltas = delta.get("tool_calls")
                                    if isinstance(tool_call_deltas, list):
                                        for tool_delta in tool_call_deltas:
                                            if isinstance(tool_delta, dict):
                                                _absorb_tool_call_delta(
                                                    tool_delta, tool_buffers, tool_order
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

        # Assemble the final response blocks.
        blocks: list[Block] = []
        text = "".join(text_parts)
        if text:
            blocks.append(TextBlock(text=text))
        for index in tool_order:
            buffer = tool_buffers[index]
            try:
                parsed_input = json.loads(buffer["arguments"]) if buffer["arguments"] else {}
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
                    id=buffer["id"] or f"tool_call_{index}",
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
            response=ModelResponse(blocks=blocks, stop_reason=stop_reason, usage=usage)
        )
