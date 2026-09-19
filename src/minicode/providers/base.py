"""Provider abstraction: the contract every model adapter implements.

A *provider* turns a normalized conversation (system prompt, ``Message``
list, ``ToolSpec`` list) into a stream of events and a final
:class:`~minicode.core.models.ModelResponse`.

Stream contract
---------------
Every ``Provider.stream`` call returns an async generator that:

- yields **zero or more** :class:`TextDelta` events (incremental assistant
  text), then **exactly one** :class:`ResponseDone` event;
- yields **nothing after** ``ResponseDone`` — it is the terminal event;
- **raises** :class:`~minicode.providers.errors.ProviderError` (or a
  subclass) on failure instead of returning a malformed or partial
  ``ResponseDone``;
- carries *complete, valid* tool inputs: each ``ToolUseBlock`` inside
  ``ResponseDone.response`` must hold a fully-parsed ``dict`` for its
  ``input`` (no partial JSON, no raw strings).

Consumers may therefore drive the loop as::

    async for event in provider.stream(system=..., messages=..., tools=...):
        if isinstance(event, TextDelta):
            ...  # render incrementally
        else:  # ResponseDone
            ...  # persist blocks / dispatch tool calls
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import AsyncIterator, Protocol

from minicode.core.models import Message, ModelResponse, ToolSpec


@dataclass(slots=True)
class TextDelta:
    """An incremental piece of assistant text streamed by the provider."""

    text: str


@dataclass(slots=True)
class ResponseDone:
    """Terminal stream event carrying the fully assembled response."""

    response: ModelResponse


StreamEvent = TextDelta | ResponseDone


class Provider(Protocol):
    """Structural interface implemented by all model adapters.

    Concrete classes are *not* required to inherit from this Protocol;
    satisfying its shape (``name`` attribute plus a ``stream`` method
    honouring the module docstring's stream contract) is enough.
    """

    name: str

    def stream(
        self,
        *,
        system: str | None,
        messages: list[Message],
        tools: list[ToolSpec],
    ) -> AsyncIterator[StreamEvent]:
        """Stream one assistant turn for the given conversation."""
        ...
