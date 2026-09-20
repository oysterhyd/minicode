"""Rough token estimation for context management.

These are ESTIMATES, not exact tokenizer counts: the harness never has the
model's tokenizer available, so everything here uses a deliberately
conservative characters/3 heuristic (≈3 characters per token for mixed
code/English/Chinese content). The numbers are only used to decide *when* to
compact and to report before/after sizes — never as a billing figure or a
provider parameter. Expect real tokenizers to differ by ±30% or more.
"""

from __future__ import annotations

import json

from minicode.core.models import (
    Message,
    TextBlock,
    ToolResultBlock,
    ToolSpec,
    ToolUseBlock,
)

__all__ = ["estimate_tokens", "estimate_messages_tokens"]


def estimate_tokens(text: str) -> int:
    """Conservative token estimate for a piece of text: ``len(text) // 3``.

    Any non-empty text counts as at least one token; empty text counts as 0
    so summing over absent system prompts / empty blocks stays honest.
    """
    if not text:
        return 0
    return max(1, len(text) // 3)


def _block_tokens(block: TextBlock | ToolUseBlock | ToolResultBlock) -> int:
    """Estimated tokens carried by one conversation block."""
    if isinstance(block, TextBlock):
        return estimate_tokens(block.text)
    if isinstance(block, ToolResultBlock):
        return estimate_tokens(block.content)
    if isinstance(block, ToolUseBlock):
        # The provider sees the call name plus the serialized input JSON.
        try:
            payload = json.dumps(block.input, ensure_ascii=False)
        except (TypeError, ValueError):  # pragma: no cover - inputs are JSON-safe
            payload = str(block.input)
        return estimate_tokens(block.name) + estimate_tokens(payload)
    return 0  # pragma: no cover - exhaustive over the Block union


def estimate_messages_tokens(
    system: str | None,
    messages: list[Message],
    tool_specs: list[ToolSpec] | None = None,
    reserve_output_tokens: int = 2000,
) -> int:
    """Estimate the full prompt size for the next provider call.

    Counts the system prompt, every block of every message (tool_result
    content, tool_use input JSON, plain text), the serialized tool schemas
    and a fixed reserve for the response the model is about to produce.
    """
    total = estimate_tokens(system) if system else 0
    for message in messages:
        total += sum(_block_tokens(block) for block in message.content)
    if tool_specs:
        for spec in tool_specs:
            try:
                payload = json.dumps(spec.model_dump(), ensure_ascii=False)
            except (TypeError, ValueError):  # pragma: no cover - specs are JSON-safe
                payload = spec.name
            total += estimate_tokens(payload)
    return total + reserve_output_tokens
