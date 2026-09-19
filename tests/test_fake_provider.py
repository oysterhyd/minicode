"""Tests for the deterministic scripted FakeProvider (plain pytest + asyncio.run)."""

from __future__ import annotations

import asyncio
from typing import Any

from minicode.core.models import StopReason, TextBlock
from minicode.providers import (
    FakeProvider,
    FakeProviderOptions,
    FakeToolCall,
    FakeTurn,
    ResponseDone,
    StreamEvent,
    TextDelta,
)


async def collect(provider: Any, **kw: Any) -> list[StreamEvent]:
    """Drain one provider.stream call with empty request arguments."""
    return [e async for e in provider.stream(system=None, messages=[], tools=[])]


def dones(events: list[StreamEvent]) -> list[ResponseDone]:
    return [e for e in events if isinstance(e, ResponseDone)]


def deltas(events: list[StreamEvent]) -> list[TextDelta]:
    return [e for e in events if isinstance(e, TextDelta)]


# ---------------------------------------------------------------------------
# Text turns
# ---------------------------------------------------------------------------


def test_text_turn_streams_full_text_and_single_done() -> None:
    text = "abcdefghij" * 12  # 120 chars -> several ~40-char chunks
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[FakeTurn(text=text, input_tokens=7, output_tokens=3)]
        )
    )

    events = asyncio.run(collect(provider))

    ds = deltas(events)
    dns = dones(events)
    assert "".join(d.text for d in ds) == text
    assert all(len(d.text) <= 40 for d in ds)
    assert len(ds) >= 2  # text was actually split into multiple deltas
    assert len(dns) == 1
    # Exactly one ResponseDone, as the terminal event, nothing after it.
    assert events[-1] is dns[0]

    response = dns[0].response
    assert response.blocks == [TextBlock(text=text)]
    assert response.stop_reason == StopReason.END_TURN
    assert response.usage.input_tokens == 7
    assert response.usage.output_tokens == 3


def test_empty_text_yields_no_deltas_and_empty_response() -> None:
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="")]))

    events = asyncio.run(collect(provider))

    assert deltas(events) == []
    assert len(dones(events)) == 1
    assert dones(events)[0].response.blocks == []


# ---------------------------------------------------------------------------
# Tool-call turns
# ---------------------------------------------------------------------------


def test_tool_calls_get_auto_ids_monotonic_across_turns() -> None:
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(tool_calls=[FakeToolCall(name="read_file", arguments={"path": "a.py"})]),
                FakeTurn(
                    tool_calls=[
                        FakeToolCall(name="write_file"),
                        FakeToolCall(name="list_dir", id="explicit-id"),
                    ]
                ),
            ]
        )
    )

    events_first = asyncio.run(collect(provider))
    events_second = asyncio.run(collect(provider))

    dns = dones(events_first) + dones(events_second)
    assert len(dns) == 2
    first, second = (d.response for d in dns)

    assert [c.id for c in first.tool_calls] == ["fake_tool_1"]
    assert first.tool_calls[0].input == {"path": "a.py"}
    assert first.stop_reason == StopReason.TOOL_USE

    # Counter keeps increasing across turns; explicit ids pass through.
    assert [c.id for c in second.tool_calls] == ["fake_tool_2", "explicit-id"]
    assert second.stop_reason == StopReason.TOOL_USE
    # No text in these turns -> no deltas at all.
    assert deltas(events_first) == [] and deltas(events_second) == []


# ---------------------------------------------------------------------------
# Determinism
# ---------------------------------------------------------------------------


def test_identical_options_produce_identical_sequences() -> None:
    options = FakeProviderOptions(
        turns=[
            FakeTurn(
                text="hello world " * 10,
                tool_calls=[FakeToolCall(name="t", arguments={"k": "v"})],
            ),
            FakeTurn(text="done"),
        ]
    )
    provider_a = FakeProvider(options)
    provider_b = FakeProvider(options)
    # One stream() call consumes one scripted turn; drain both turns.
    events_a = asyncio.run(collect(provider_a)) + asyncio.run(collect(provider_a))
    events_b = asyncio.run(collect(provider_b)) + asyncio.run(collect(provider_b))

    assert [d.text for d in deltas(events_a)] == [d.text for d in deltas(events_b)]
    assert [d.response.model_dump() for d in dones(events_a)] == [
        d.response.model_dump() for d in dones(events_b)
    ]


# ---------------------------------------------------------------------------
# Script exhaustion, usage defaults, reset
# ---------------------------------------------------------------------------


def test_script_exhaustion_yields_fallback_text_and_end_turn() -> None:
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="only turn")]))

    first = asyncio.run(collect(provider))
    assert "".join(d.text for d in deltas(first)) == "only turn"
    assert provider.turns_consumed == 1

    second = asyncio.run(collect(provider))
    response = dones(second)[0].response
    assert response.text == "(fake provider: script exhausted)"
    assert response.stop_reason == StopReason.END_TURN
    assert response.usage.input_tokens == 100  # default_input_tokens
    assert response.usage.output_tokens == 20  # default_output_tokens
    assert provider.turns_consumed == 2


def test_per_turn_usage_override_respected() -> None:
    provider = FakeProvider(
        FakeProviderOptions(
            turns=[
                FakeTurn(text="a", input_tokens=11, output_tokens=5),
                FakeTurn(text="b"),  # no overrides -> provider defaults
            ],
            default_input_tokens=100,
            default_output_tokens=20,
        )
    )

    events_first = asyncio.run(collect(provider))
    events_second = asyncio.run(collect(provider))

    first, second = (d.response for d in dones(events_first) + dones(events_second))
    assert (first.usage.input_tokens, first.usage.output_tokens) == (11, 5)
    assert (second.usage.input_tokens, second.usage.output_tokens) == (100, 20)


def test_reset_rewinds_script_index() -> None:
    provider = FakeProvider(FakeProviderOptions(turns=[FakeTurn(text="only turn")]))

    asyncio.run(collect(provider))
    assert provider.turns_consumed == 1

    provider.reset()
    assert provider.turns_consumed == 0

    replay = asyncio.run(collect(provider))
    assert "".join(d.text for d in deltas(replay)) == "only turn"
    assert provider.turns_consumed == 1
