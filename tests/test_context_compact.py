"""Tests for minicode.context (estimate + layered compaction) — plain sync
pytest, no pytest-asyncio. Messages are constructed directly; the artifact
store is faked with a small callable-based double."""

from __future__ import annotations

import json

from minicode.context import (
    CompactConfig,
    CompactStats,
    ContextCompactor,
    estimate_messages_tokens,
    estimate_tokens,
)
from minicode.context.compact import SHRINK_MARKER
from minicode.core.models import (
    Message,
    TextBlock,
    ToolResultBlock,
    ToolSpec,
    ToolUseBlock,
)

# ---------------------------------------------------------------------------
# Fixtures: interaction units (user -> assistant(tool call) -> user(result))
# ---------------------------------------------------------------------------


def make_unit(
    index: int,
    *,
    tool: str = "read_file",
    tool_input: dict | None = None,
    assistant_text: str | None = None,
    result_chars: int = 0,
    result_text: str | None = None,
    error: bool = False,
) -> list[Message]:
    """One complete interaction unit for tool call ``call_<index>``."""
    call_id = f"call_{index}"
    blocks: list = []
    if assistant_text:
        blocks.append(TextBlock(text=assistant_text))
    blocks.append(
        ToolUseBlock(id=call_id, name=tool, input=tool_input or {"path": f"file_{index}.py"})
    )
    content = result_text if result_text is not None else ("x" * result_chars or "ok")
    return [
        Message(role="user", content=[TextBlock(text=f"任务 {index}")]),
        Message(role="assistant", content=blocks),
        Message(role="user", content=[ToolResultBlock(tool_use_id=call_id, content=content, is_error=error)]),
    ]


def build_history(count: int, **unit_kwargs) -> list[Message]:
    return [message for index in range(1, count + 1) for message in make_unit(index, **unit_kwargs)]


def assert_pairing(messages: list[Message]) -> None:
    """Every tool_use has exactly its own tool_result in the same list."""
    use_ids = {b.id for m in messages for b in m.content if isinstance(b, ToolUseBlock)}
    result_ids = {b.tool_use_id for m in messages for b in m.content if isinstance(b, ToolResultBlock)}
    assert use_ids == result_ids


class FakeSpill:
    """spill_fn double recording (kind, content, artifact_id) calls."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str]] = []

    def __call__(self, kind: str, content: str) -> str:
        artifact_id = f"{kind}_{len(self.calls):04x}"
        self.calls.append((kind, content, artifact_id))
        return artifact_id


# ---------------------------------------------------------------------------
# 1. estimate_tokens
# ---------------------------------------------------------------------------


def test_estimate_tokens_is_conservative_and_monotonic():
    assert estimate_tokens("") == 0
    assert estimate_tokens("ab") == 1  # short text still counts as 1
    assert estimate_tokens("abcdef") == 2  # 6 chars // 3
    assert estimate_tokens("a" * 30) == 10
    # Monotonic in length.
    assert estimate_tokens("a" * 100) < estimate_tokens("a" * 1000)


# ---------------------------------------------------------------------------
# 2. estimate_messages_tokens components
# ---------------------------------------------------------------------------


def test_estimate_messages_tokens_counts_all_components():
    # Empty prompt = output reserve only.
    assert estimate_messages_tokens(None, [], None) == 2000
    assert estimate_messages_tokens(None, [], None, reserve_output_tokens=0) == 0

    # System prompt is counted.
    assert estimate_messages_tokens("abcdef", [], None) == 2000 + 2

    # Plain text blocks.
    msgs = [Message(role="user", content=[TextBlock(text="a" * 30)])]
    assert estimate_messages_tokens(None, msgs, None) == 2000 + 10

    # tool_result content is counted.
    msgs = [Message(role="user", content=[ToolResultBlock(tool_use_id="t", content="a" * 30)])]
    assert estimate_messages_tokens(None, msgs, None) == 2000 + 10

    # tool_use contributes call name + serialized input JSON.
    tool_input = {"path": "a" * 30}
    msgs = [Message(role="assistant", content=[ToolUseBlock(id="t", name="read_file", input=tool_input)])]
    expected = estimate_tokens("read_file") + estimate_tokens(json.dumps(tool_input, ensure_ascii=False))
    assert estimate_messages_tokens(None, msgs, None) == 2000 + expected

    # Tool schemas add on top of the reserve.
    spec = ToolSpec(name="read_file", description="d" * 300, input_schema={"type": "object"})
    without = estimate_messages_tokens(None, [], None)
    with_specs = estimate_messages_tokens(None, [], [spec])
    assert with_specs - without >= estimate_tokens(json.dumps(spec.model_dump(), ensure_ascii=False))


# ---------------------------------------------------------------------------
# 3. needs_compaction threshold behaviour
# ---------------------------------------------------------------------------


def test_needs_compaction_threshold():
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000))  # trigger at 8000
    small = build_history(1, result_chars=50)
    big = build_history(20, result_chars=1000)

    assert compactor.needs_compaction("system", small) is False
    assert compactor.needs_compaction("system", big) is True

    # Exactly consistent with the estimator and the trigger fraction.
    estimate = estimate_messages_tokens("system", big)
    assert compactor.needs_compaction("system", big) == (estimate > 10_000 * 0.8)

    # Tool schemas count towards the estimate.
    spec = ToolSpec(name="t", description="d" * 5000, input_schema={"type": "object"})
    assert compactor.needs_compaction(None, [], [spec]) is False  # ~1666 + 2000 < 8000
    heavy = ContextCompactor(CompactConfig(max_context_tokens=3000))
    assert heavy.needs_compaction(None, [], [spec]) is True


# ---------------------------------------------------------------------------
# 4. No-op paths: empty / tiny / all-tail histories come back untouched
# ---------------------------------------------------------------------------


def test_compact_noop_on_empty_and_tiny_histories():
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000))

    empty = compactor.compact(None, [])  # no system prompt: reserve only
    assert empty.changed is False
    assert empty.messages == []
    assert empty.stats == CompactStats(tokens_before=2000, tokens_after=2000)

    # A single unit is the tail: nothing may be archived.
    single = build_history(1)
    result = compactor.compact("system", single)
    assert result.changed is False
    assert result.messages is single  # original list returned as-is
    assert result.stats.tokens_after == result.stats.tokens_before

    # All four units are inside the tail window and every result is below
    # the shrink preview size -> nothing archived, nothing shrunk.
    # (An all-tail history with LARGE old results is deliberately still
    # shrunk by step B; that case is covered in test_compact_shrinks_old... )
    all_tail = build_history(4, result_chars=100)
    result = compactor.compact("system", all_tail)
    assert result.changed is False
    assert result.messages is all_tail
    assert result.stats.archived_units == 0


# ---------------------------------------------------------------------------
# 5. Step A: early units archived behind an [artifact:...] reference
# ---------------------------------------------------------------------------


def test_compact_archives_early_units_with_spill_reference():
    messages = build_history(6, result_chars=1000)
    spill = FakeSpill()
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=10_000, tail_keep_rounds=2), spill
    )

    result = compactor.compact("system", messages)

    assert result.changed is True
    assert result.stats.archived_units == 4
    assert result.stats.shrunk_results == 0  # tail results are the most recent two
    assert result.stats.tokens_after < result.stats.tokens_before

    # Exactly one archive spill, holding the full JSON of 12 archived messages.
    assert len(spill.calls) == 1
    kind, payload, artifact_id = spill.calls[0]
    assert kind == "archive"
    stored = json.loads(payload)
    assert len(stored) == 12
    use_ids = [b["id"] for m in stored for b in m["content"] if b["type"] == "tool_use"]
    assert use_ids == [f"call_{i}" for i in range(1, 5)]

    # Replacement message carries the artifact reference marker.
    summary = result.messages[0]
    assert summary.role == "user"
    text = summary.content[0].text
    assert f"[artifact:{artifact_id}]" in text
    assert "可用 read_artifact 工具读取完整内容" in text
    assert "read_file" in text and "file_1.py" in text  # tool + arg digest

    # Pairing invariant and complete removal of archived units.
    assert_pairing(result.messages)
    remaining_uses = {b.id for m in result.messages for b in m.content if isinstance(b, ToolUseBlock)}
    assert remaining_uses == {"call_5", "call_6"}


# ---------------------------------------------------------------------------
# 6. Step A without a spill_fn: pure truncation-style replacement
# ---------------------------------------------------------------------------


def test_compact_without_spill_fn_replaces_without_reference():
    messages = build_history(6, result_chars=1000)
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000, tail_keep_rounds=2))

    result = compactor.compact("system", messages)

    assert result.changed is True
    assert result.stats.archived_units == 4
    text = result.messages[0].content[0].text
    assert "[artifact:" not in text
    assert "未配置 artifact 存储" in text
    assert_pairing(result.messages)


# ---------------------------------------------------------------------------
# 7. Step B: old tool results shrink to preview + marker
# ---------------------------------------------------------------------------


def test_compact_shrinks_old_tool_results():
    messages = build_history(3, result_chars=1200)
    # tail_keep_rounds=4 covers all three units -> no archiving; step B still
    # protects only the two most recent results.
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000, tail_keep_rounds=4))

    result = compactor.compact("system", messages)

    assert result.changed is True
    assert result.stats.archived_units == 0
    assert result.stats.shrunk_results == 1

    oldest = result.messages[2].content[0]
    assert oldest.content == "x" * 200 + SHRINK_MARKER
    # The two most recent results stay full.
    assert result.messages[5].content[0].content == "x" * 1200
    assert result.messages[8].content[0].content == "x" * 1200
    assert_pairing(result.messages)

    # Input objects were never mutated.
    assert messages[2].content[0].content == "x" * 1200


def test_compact_shrink_leaves_small_results_alone():
    messages = build_history(3, result_chars=100)  # below shrink_preview_chars
    compactor = ContextCompactor(CompactConfig(max_context_tokens=10_000, tail_keep_rounds=4))

    result = compactor.compact("system", messages)

    assert result.changed is False
    assert result.stats.shrunk_results == 0
    assert result.messages is messages


# ---------------------------------------------------------------------------
# 8. Tail units are never touched (identity-preserving)
# ---------------------------------------------------------------------------


def test_tail_units_never_touched():
    messages = build_history(6, result_chars=1000)
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=10_000, tail_keep_rounds=2), FakeSpill()
    )

    result = compactor.compact("system", messages)

    # The retained tail messages are the very same objects, in order.
    tail = result.messages[1:]
    assert tail == messages[-6:]
    for original, kept in zip(messages[-6:], tail):
        assert kept is original
    # Newest result content stays verbatim.
    assert result.messages[-1].content[0].content == "x" * 1000
    # Structure: summary + two full user/assistant/user rounds.
    assert [m.role for m in result.messages] == [
        "user", "user", "assistant", "user", "user", "assistant", "user",
    ]


# ---------------------------------------------------------------------------
# 9. Step C: structured summary when still over budget
# ---------------------------------------------------------------------------


def test_structured_summary_merges_remaining_early_units():
    messages = (
        make_unit(
            1,
            tool="edit",
            tool_input={"path": "src/auth.py", "new_text": "fix"},
            assistant_text="先看认证代码",
            result_chars=2500,
        )
        + make_unit(2, result_text="boom: file not found\n" + "x" * 2479, error=True)
        + make_unit(3, assistant_text="已修复主要问题\nTODO: 重试写入配置", result_chars=2500)
        + make_unit(4, result_chars=2500)
    )
    # min_archive_units=5 blocks step A (only 3 early units); the estimate
    # stays over budget after shrinking, so step C must merge units 1-3.
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=2500, tail_keep_rounds=1, min_archive_units=5),
        FakeSpill(),
    )

    result = compactor.compact("system", messages)

    assert result.changed is True
    assert result.stats.archived_units == 0
    assert result.stats.shrunk_results == 2  # units 1-2 results; units 3-4 are most recent
    assert result.stats.summarized_units == 3

    assert len(result.messages) == 4  # summary + last unit (3 messages)
    text = result.messages[0].content[0].text
    assert text.startswith("[context summary]")
    assert "- 目标: 任务 1" in text
    assert "- 关键文件: src/auth.py" in text
    assert "- 失败原因: boom: file not found" in text
    assert "- 剩余 todo: TODO: 重试写入配置" in text
    assert result.stats.tokens_after < result.stats.tokens_before

    # The last unit is preserved as the same objects; pairs stay intact.
    for original, kept in zip(messages[-3:], result.messages[1:]):
        assert kept is original
    assert_pairing(result.messages)


def test_structured_summary_skipped_under_threshold():
    messages = build_history(4, result_chars=50)
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=10_000, tail_keep_rounds=1, min_archive_units=5)
    )
    result = compactor.compact("system", messages)
    # Early units exist (3) but the estimate is far below the trigger, and
    # step A is blocked by min_archive_units -> nothing happens.
    assert result.changed is False
    assert result.stats.summarized_units == 0


# ---------------------------------------------------------------------------
# 10. Pairing safety net: a use/result pair split across the boundary must
#     block archiving instead of orphaning a tool_result
# ---------------------------------------------------------------------------


def test_pairing_safety_blocks_unsafe_archive():
    # Pathological ordering: the result arrives after a new human input, so
    # unit 1 holds the tool_use while unit 2 holds its tool_result.
    messages = [
        Message(role="user", content=[TextBlock(text="第一个请求")]),
        Message(role="assistant", content=[ToolUseBlock(id="call_a", name="read_file", input={"path": "x"})]),
        Message(role="user", content=[TextBlock(text="等一下")]),
        Message(role="user", content=[ToolResultBlock(tool_use_id="call_a", content="result text")]),
    ]
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=10_000, tail_keep_rounds=1, min_archive_units=1),
        FakeSpill(),
    )

    result = compactor.compact("system", messages)

    # Archiving unit 1 alone would orphan the retained result -> refused.
    assert result.changed is False
    assert result.stats.archived_units == 0
    assert result.messages is messages


def test_pairing_safety_allows_archive_when_span_is_complete():
    messages = [
        Message(role="user", content=[TextBlock(text="第一个请求")]),
        Message(role="assistant", content=[ToolUseBlock(id="call_a", name="read_file", input={"path": "x"})]),
        Message(role="user", content=[TextBlock(text="等一下")]),
        Message(role="user", content=[ToolResultBlock(tool_use_id="call_a", content="result text")]),
    ]
    spill = FakeSpill()
    compactor = ContextCompactor(
        CompactConfig(max_context_tokens=10_000, tail_keep_rounds=0, min_archive_units=1),
        spill,
    )

    result = compactor.compact("system", messages)

    # The whole history is one archive span: the pair moves together.
    assert result.changed is True
    assert result.stats.archived_units == 2
    assert len(result.messages) == 1
    assert_pairing(result.messages)
    assert len(spill.calls) == 1
    assert "call_a" in spill.calls[0][1]
    assert "result text" in spill.calls[0][1]
