"""Model provider adapters for minicode.

Exports the provider contract (:class:`Provider` plus its stream events),
the provider error hierarchy, and the concrete adapters:
the deterministic :class:`FakeProvider` used by tests and no-key demos,
the real :class:`AnthropicProvider` (which imports the ``anthropic``
SDK lazily so this package imports fine without it installed), and the
OpenAI-compatible :class:`CommandCodeProvider` (the default adapter,
configured via ``COMMANDCODE_API_KEY`` or a local ZCode installation).
"""

from __future__ import annotations

from .anthropic_provider import AnthropicProvider
from .base import Provider, ResponseDone, StreamEvent, TextDelta
from .commandcode import CommandCodeProvider
from .errors import ProviderAuthError, ProviderError, ProviderRequestError
from .fake import FakeProvider, FakeProviderOptions, FakeToolCall, FakeTurn

__all__ = [
    "AnthropicProvider",
    "CommandCodeProvider",
    "FakeProvider",
    "FakeProviderOptions",
    "FakeToolCall",
    "FakeTurn",
    "Provider",
    "ProviderAuthError",
    "ProviderError",
    "ProviderRequestError",
    "ResponseDone",
    "StreamEvent",
    "TextDelta",
]
