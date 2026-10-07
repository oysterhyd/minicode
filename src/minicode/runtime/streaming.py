"""Provider request boundaries, protocol validation and observable retries."""

from __future__ import annotations

import asyncio
import json
import time
from contextlib import aclosing
from dataclasses import dataclass
from typing import Awaitable, Callable

from minicode.core.limits import MAX_RESPONSE_BYTES, MAX_TOOL_CALLS
from minicode.core.models import EventType, ExitReason, Message, ModelResponse, ToolResultBlock, ToolSpec
from minicode.providers.base import Provider, ResponseDone, TextDelta
from minicode.providers.errors import ProviderError, ProviderProtocolError


@dataclass(slots=True)
class StreamResult:
    response: ModelResponse | None = None
    duration_s: float = 0.0
    failure: ExitReason | None = None
    error: Exception | None = None
    usage_uncertain: bool = False


def validate_response(response: ModelResponse, known_ids: set[str],
                      settled: Callable[[str], bool]) -> None:
    if not isinstance(response, ModelResponse):
        raise ProviderProtocolError("provider returned an invalid response")
    if any(isinstance(block, ToolResultBlock) for block in response.blocks):
        raise ProviderProtocolError("provider emitted a tool result as an assistant block")
    calls = response.tool_calls
    if len(calls) > MAX_TOOL_CALLS:
        raise ProviderProtocolError("provider returned too many tool calls")
    ids: set[str] = set()
    for call in calls:
        if (not call.id.strip() or len(call.id) > 256 or not call.name.strip()
                or call.id in ids or call.id in known_ids or settled(call.id)):
            raise ProviderProtocolError("provider returned a missing or reused tool call id/name")
        ids.add(call.id)
    if len(response.model_dump_json().encode("utf-8")) > MAX_RESPONSE_BYTES:
        raise ProviderProtocolError("provider response exceeds the capture limit")


async def stream_response(
    provider: Provider, *, system: str, messages: list[Message], tools: list[ToolSpec],
    on_delta: Callable[[str], Awaitable[None]] | None,
    on_retry: Callable[[EventType, dict], Awaitable[object]],
    settled: Callable[[str], bool], attempts: int = 3,
) -> StreamResult:
    # Adapters and observers never receive the runtime's mutable conversation.
    messages = [message.model_copy(deep=True) for message in messages]
    tools = [tool.model_copy(deep=True) for tool in tools]
    known_ids = {call.id for message in messages for call in message.content
                 if getattr(call, "type", None) == "tool_use"}
    uncertain = False
    for attempt in range(attempts):
        started = time.monotonic()
        saw_text = False
        captured = 0
        try:
            async with asyncio.timeout(getattr(provider, "timeout_s", 180.0)):
                async with aclosing(provider.stream(
                    system=system, messages=[message.model_copy(deep=True) for message in messages],
                    tools=[tool.model_copy(deep=True) for tool in tools],
                )) as stream:
                    async for event in stream:
                        if isinstance(event, TextDelta):
                            saw_text = saw_text or bool(event.text)
                            captured += len(event.text.encode("utf-8"))
                            if captured > MAX_RESPONSE_BYTES:
                                raise ProviderProtocolError("provider text exceeds the capture limit")
                            if on_delta is not None:
                                try:
                                    await on_delta(event.text)
                                except Exception:
                                    # UI observer failure cannot invalidate a response.
                                    pass
                        elif isinstance(event, ResponseDone):
                            validate_response(event.response, known_ids, settled)
                            if not event.response.blocks:
                                raise ProviderProtocolError("provider returned an empty assistant response")
                            return StreamResult(response=event.response.model_copy(deep=True), duration_s=time.monotonic() - started,
                                                usage_uncertain=uncertain)
                        else:
                            raise ProviderProtocolError("provider emitted an unknown stream event")
            raise ProviderProtocolError("provider stream ended without a final response")
        except asyncio.CancelledError:
            raise
        except (ProviderError, TimeoutError) as error:
            uncertain = True
            if isinstance(error, TimeoutError):
                error = ProviderError("provider request timed out")
            if not error.retryable or saw_text or attempt + 1 == attempts:
                return StreamResult(failure=ExitReason.PROVIDER_ERROR, error=error, usage_uncertain=True)
            delay = min(30.0, max(0.0, error.retry_after if error.retry_after is not None
                                   else 0.25 * 2 ** attempt))
            await on_retry(EventType.PROVIDER_RETRY, {"attempt": attempt + 1,
                           "next_attempt": attempt + 2, "delay_s": delay,
                           "error": str(error)})
            await asyncio.sleep(delay)
        except Exception as error:
            return StreamResult(failure=ExitReason.INTERNAL_ERROR, error=error, usage_uncertain=True)
    raise AssertionError("stream attempts exhausted")
