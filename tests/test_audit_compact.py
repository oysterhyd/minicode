"""Pairing safety remains equivalent while single-task compaction stays linear."""
from __future__ import annotations

import random

from minicode.context import compact
from minicode.core.models import Message, TextBlock, ToolUseBlock, ToolResultBlock


def reference_pairing(messages, lo, hi):
    inside, outside = set(), set()
    for index, message in enumerate(messages):
        for block in message.content:
            if isinstance(block, ToolUseBlock):
                (inside if lo <= index < hi else outside).add(block.id)
    for index, message in enumerate(messages):
        for block in message.content:
            if isinstance(block, ToolResultBlock) and block.tool_use_id not in (inside if lo <= index < hi else outside):
                return False
    return True


def test_pairing_counter_matches_reference_with_duplicate_and_orphan_ids():
    rng = random.Random(2026)
    for _ in range(80):
        messages = []
        for _ in range(10):
            blocks = []
            for _ in range(rng.randrange(4)):
                call_id = str(rng.randrange(5))
                blocks.append(ToolUseBlock(id=call_id, name="read", input={}) if rng.randrange(2)
                              else ToolResultBlock(tool_use_id=call_id, content="result"))
            messages.append(Message(role="user", content=blocks))
        for lo in range(len(messages)):
            for hi in range(lo, len(messages) + 1):
                assert compact._pairing_ok(messages, lo, hi) == reference_pairing(messages, lo, hi)


def test_closed_exchange_compaction_scans_history_only_once(monkeypatch):
    messages = [Message(role="user", content=[TextBlock(text="task")])]
    for index in range(1000):
        messages.extend([
            Message(role="assistant", content=[ToolUseBlock(id=str(index), name="read", input={})]),
            Message(role="user", content=[ToolResultBlock(tool_use_id=str(index), content="x" * 200)]),
        ])
    visited = 0
    original = compact._pairing_counts
    def counted(values):
        nonlocal visited
        visited += len(values)
        return original(values)
    monkeypatch.setattr(compact, "_pairing_counts", counted)
    compactor = compact.ContextCompactor(compact.CompactConfig(max_context_tokens=10_000, tail_keep_rounds=2))
    result = compactor.compact(None, messages)
    assert result.stats.archived_units == 998
    assert visited <= 2 * len(messages) + 50
    assert reference_pairing(result.messages, 0, len(result.messages))
