"""Tests for minicode.reports.html — offline single-file HTML execution report.

Plain sync pytest; events/messages/summaries are hand-constructed (no
runtime needed) plus one integration test through a real SqliteStore.
"""

from __future__ import annotations

from html.parser import HTMLParser
from typing import Any

import pytest

from minicode.core.models import (
    Event,
    EventType,
    Message,
    TextBlock,
    ToolResultBlock,
)
from minicode.reports import render_session_html
from minicode.storage import SessionSummary, SqliteStore

# ---------------------------------------------------------------------------
# Fixtures / helpers
# ---------------------------------------------------------------------------

APPLY_PATCH_DIFF = (
    "--- a/app.py\n"
    "+++ b/app.py\n"
    "@@ -1,3 +1,4 @@\n"
    "+print('added line')\n"
    "-print('removed line')\n"
    " context line"
)


def make_summary(**overrides: Any) -> SessionSummary:
    defaults: dict[str, Any] = dict(
        session_id="sess-abc123",
        created_at="2026-09-20T08:30:00.123456+00:00",
        workspace="D:/work/demo",
        provider="fake",
        model="fake-mini",
        status="completed",
        exit_reason="completed",
        rounds=3,
        input_tokens=1200,
        output_tokens=340,
    )
    defaults.update(overrides)
    return SessionSummary(**defaults)


def make_event(
    seq: int,
    type_: EventType,
    data: dict[str, Any] | None = None,
    ts: str = "2026-09-20T08:31:05.987654+00:00",
) -> Event:
    return Event(seq=seq, type=type_, timestamp=ts, data=data or {})


class _ParseChecker(HTMLParser):
    """Records start tags; feeding must never raise."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.tags: list[str] = []

    def handle_starttag(self, tag: str, attrs) -> None:  # noqa: ANN001
        self.tags.append(tag)


def parse_ok(html_out: str) -> _ParseChecker:
    checker = _ParseChecker()
    checker.feed(html_out)
    checker.close()
    return checker


# Realistic payload per event type, used by the badge-coverage tests.
SAMPLE_DATA: dict[EventType, dict[str, Any]] = {
    EventType.SESSION_START: {
        "workspace": "D:/work/demo",
        "provider": "fake",
        "model": "fake-mini",
    },
    EventType.ROUND_START: {"round": 1},
    EventType.ASSISTANT_MESSAGE: {
        "text": "Let me run the tests.",
        "tool_calls": ["bash"],
        "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15},
        "stop_reason": "tool_use",
    },
    EventType.TOOL_CALL_START: {
        "call_id": "c1",
        "name": "bash",
        "arguments": {"command": "pytest -q"},
    },
    EventType.TOOL_CALL_RESULT: {
        "call_id": "c1",
        "name": "bash",
        "success": True,
        "exit_code": 0,
        "output_preview": "1 passed",
    },
    EventType.TOOL_OUTPUT: {"call_id": "c1", "name": "bash", "output_preview": "collecting 12 tests"},
    EventType.APPROVAL_REQUEST: {
        "call_id": "c1",
        "tool_name": "bash",
        "summary": "pytest -q",
    },
    EventType.APPROVAL_DECISION: {
        "call_id": "c1",
        "tool_name": "bash",
        "granted": True,
        "reason": None,
    },
    EventType.ROUND_END: {"round": 1},
    EventType.SESSION_END: {
        "exit_reason": "completed",
        "rounds": 1,
        "total_usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15},
    },
    EventType.CONTEXT_COMPACTED: {"before_tokens": 90000, "after_tokens": 30000},
    EventType.GOAL_CHECK: {
        "items": [{"item": "tests pass", "kind": "command", "passed": True, "exit_code": 0}]
    },
    EventType.SIDE_EFFECT_UNKNOWN: {
        "tool": "bash",
        "detail": "process crashed after writing the file",
    },
    EventType.BACKGROUND_JOB_STARTED: {"job_id": "j1", "command": "sleep 10"},
    EventType.BACKGROUND_JOB_COMPLETED: {"job_id": "j1", "exit_code": 0},
    EventType.BACKGROUND_JOB_LOST: {"job_id": "j1"},
    EventType.PROJECT_INSTRUCTIONS: {"path": "AGENTS.md", "sha256": "abc"},
    EventType.SKILL_ACTIVATED: {"name": "review", "sha256": "def"},
    EventType.SKILL_DEACTIVATED: {"name": "review"},
    EventType.SUBAGENT_START: {"kind": "review", "child_session_id": "child-1"},
    EventType.SUBAGENT_RESULT: {"kind": "review", "child_session_id": "child-1", "exit_reason": "completed"},
    EventType.MCP_DISCOVERY: {"server": "docs", "plugin": "docs", "protocol": "2026-07-28"},
    EventType.BUDGET_CHANGED: {"budget": {"max_total_tokens": 1000}},
    EventType.PROVIDER_RETRY: {"attempt": 1, "next_attempt": 2, "delay_s": 0.25, "error": "temporary outage"},
}

# ---------------------------------------------------------------------------
# Header / meta
# ---------------------------------------------------------------------------


def test_header_renders_summary_fields_and_timestamp():
    html_out = render_session_html(make_summary(), [], [])

    assert "sess-abc123" in html_out
    assert "D:/work/demo" in html_out
    assert "fake-mini" in html_out
    # ISO timestamp truncated to YYYY-MM-DD HH:MM:SS
    assert "2026-09-20 08:30:00" in html_out
    assert "T08:30:00" not in html_out
    # Token totals (1200 + 340 = 1540), rendered in header and footer
    assert ">1200<" in html_out and ">340<" in html_out and ">1540<" in html_out
    assert ">3<" in html_out  # rounds


@pytest.mark.parametrize(
    ("reason", "label"),
    [
        ("completed", "已完成"),
        ("max_rounds", "达到最大轮数"),
        ("token_budget", "Token 预算耗尽"),
        ("time_budget", "时长预算耗尽"),
        ("cancelled", "已取消"),
        ("goal_not_met", "验收未通过"),
        ("provider_error", "模型调用失败"),
        ("internal_error", "内部错误"),
    ],
)
def test_exit_reason_chinese_labels(reason: str, label: str):
    html_out = render_session_html(
        make_summary(status=reason, exit_reason=reason), [], []
    )
    assert label in html_out


def test_running_status_label():
    html_out = render_session_html(
        make_summary(status="running", exit_reason=None), [], []
    )
    assert "运行中" in html_out


# ---------------------------------------------------------------------------
# Security: escaping of untrusted content
# ---------------------------------------------------------------------------


def test_tool_output_script_is_escaped():
    ev = make_event(
        0,
        EventType.TOOL_CALL_RESULT,
        {
            "call_id": "c1",
            "name": "bash",
            "success": True,
            "exit_code": 0,
            "output_preview": "<script>alert(1)</script>",
        },
    )
    html_out = render_session_html(make_summary(), [ev], [])

    assert "<script>" not in html_out
    assert "&lt;script&gt;alert(1)&lt;/script&gt;" in html_out


def test_tool_arguments_with_markup_are_escaped():
    ev = make_event(
        0,
        EventType.TOOL_CALL_START,
        {
            "call_id": "c2",
            "name": "bash",
            "arguments": {"command": 'echo "><img src=x onerror=alert(1)>'},
        },
    )
    html_out = render_session_html(make_summary(), [ev], [])

    assert "<img" not in html_out
    assert "&lt;img" in html_out
    # attribute breakout via quotes must not survive either
    assert 'onerror=alert(1)">' not in html_out


def test_diff_and_full_output_are_escaped():
    ev_start = make_event(
        0, EventType.TOOL_CALL_START, {"call_id": "c9", "name": "edit", "arguments": {}}
    )
    ev_result = make_event(
        1,
        EventType.TOOL_CALL_RESULT,
        {"call_id": "c9", "name": "edit", "success": True, "output_preview": "Done!"},
    )
    full = APPLY_PATCH_DIFF + "\n+<script>alert(1)</script>"
    html_out = render_session_html(
        make_summary(), [ev_start, ev_result], [], full_outputs={"c9": full}
    )

    assert "<script>" not in html_out
    assert "&lt;script&gt;" in html_out


# ---------------------------------------------------------------------------
# edit/write diff and full_outputs expansion
# ---------------------------------------------------------------------------


def _edit_events() -> list[Event]:
    return [
        make_event(
            0,
            EventType.TOOL_CALL_START,
            {"call_id": "c9", "name": "edit", "arguments": {"path": "a.py"}},
        ),
        make_event(
            1,
            EventType.TOOL_CALL_RESULT,
            {
                "call_id": "c9",
                "name": "edit",
                "success": True,
                "exit_code": 0,
                "output_preview": "Done!",
            },
        ),
    ]


def test_edit_diff_from_full_outputs_is_colored():
    html_out = render_session_html(
        make_summary(), _edit_events(), [], full_outputs={"c9": APPLY_PATCH_DIFF}
    )

    assert "<details" in html_out and "<summary>" in html_out
    # per-line coloring: + green, - red, @@ hunk
    assert '<span class="d-add">+print(&#x27;added line&#x27;)</span>' in html_out
    assert '<span class="d-del">-print(&#x27;removed line&#x27;)</span>' in html_out
    assert '<span class="d-hunk">@@ -1,3 +1,4 @@</span>' in html_out
    assert '<span class="d-meta">--- a/app.py</span>' in html_out


def test_edit_diff_falls_back_to_messages_tool_result():
    message = Message(
        role="user",
        content=[
            TextBlock(text="tool results"),
            ToolResultBlock(tool_use_id="c9", content=APPLY_PATCH_DIFF, is_error=False),
        ],
    )
    html_out = render_session_html(make_summary(), _edit_events(), [message])

    assert "<details" in html_out
    assert '<span class="d-add">+print(&#x27;added line&#x27;)</span>' in html_out
    assert '<span class="d-del">-print(&#x27;removed line&#x27;)</span>' in html_out


def test_non_edit_full_output_expands_without_diff_colors():
    ev_start = make_event(
        0,
        EventType.TOOL_CALL_START,
        {"call_id": "c3", "name": "read", "arguments": {"path": "a.txt"}},
    )
    ev_result = make_event(
        1,
        EventType.TOOL_CALL_RESULT,
        {
            "call_id": "c3",
            "name": "read",
            "success": True,
            "output_preview": "hello",
        },
    )
    html_out = render_session_html(
        make_summary(), [ev_start, ev_result], [], full_outputs={"c3": "line1\nline2"}
    )

    assert "<details" in html_out
    assert "line1" in html_out
    assert 'class="d-add"' not in html_out


def test_failed_tool_result_styled_differently():
    ev = make_event(
        0,
        EventType.TOOL_CALL_RESULT,
        {
            "call_id": "c4",
            "name": "bash",
            "success": False,
            "exit_code": 2,
            "error": "command not found",
        },
    )
    html_out = render_session_html(make_summary(), [ev], [])

    assert "fail" in html_out
    assert "command not found" in html_out
    assert ">2<" in html_out  # exit code rendered


# ---------------------------------------------------------------------------
# Goal checks
# ---------------------------------------------------------------------------


def test_goal_check_items_rendered_in_timeline_and_table():
    ev = make_event(
        0,
        EventType.GOAL_CHECK,
        {
            "items": [
                {"item": "tests pass", "kind": "command", "passed": True, "exit_code": 0},
                {"item": "lint clean", "kind": "command", "passed": False, "exit_code": 1},
            ]
        },
    )
    html_out = render_session_html(make_summary(), [ev], [])

    # acceptance section is always present
    assert "Goal" in html_out
    assert "goal_check" in html_out
    # every item appears with its pass/fail marker
    assert "tests pass" in html_out and "lint clean" in html_out
    assert "✓ 通过" in html_out and "✗ 失败" in html_out
    # evidence table columns
    assert "<table" in html_out
    assert "kind" in html_out and "退出码" in html_out


def test_goal_section_present_even_without_goal_events():
    html_out = render_session_html(make_summary(), [], [])
    assert "Goal" in html_out


# ---------------------------------------------------------------------------
# Badge coverage over every EventType + parser sanity
# ---------------------------------------------------------------------------


def test_all_event_types_render_and_parse():
    events = [
        make_event(seq, type_, SAMPLE_DATA[type_])
        for seq, type_ in enumerate(EventType)
    ]
    html_out = render_session_html(make_summary(), events, [])

    checker = parse_ok(html_out)  # must not raise
    assert "div" in checker.tags
    for type_ in EventType:
        assert type_.value in html_out


def test_all_event_types_with_empty_data_do_not_crash():
    events = [make_event(seq, type_, {}) for seq, type_ in enumerate(EventType)]
    html_out = render_session_html(make_summary(), events, [])

    parse_ok(html_out)
    for type_ in EventType:
        assert type_.value in html_out


# ---------------------------------------------------------------------------
# Empty session
# ---------------------------------------------------------------------------


def test_empty_session_renders_without_crash():
    html_out = render_session_html(make_summary(), [], [])

    checker = parse_ok(html_out)
    assert "sess-abc123" in checker.tags or "sess-abc123" in html_out
    assert "时间线" in html_out
    assert "用量" in html_out
    assert "（无事件记录）" in html_out


# ---------------------------------------------------------------------------
# Zero external dependencies
# ---------------------------------------------------------------------------


def test_no_external_resources():
    events = [
        make_event(seq, type_, SAMPLE_DATA[type_]) for seq, type_ in enumerate(EventType)
    ]
    html_out = render_session_html(make_summary(), events, [], full_outputs={"c1": "x"})

    lowered = html_out.lower()
    assert "<link" not in lowered
    assert "<script src" not in lowered
    assert "@import" not in lowered


# ---------------------------------------------------------------------------
# Integration: real SqliteStore round trip
# ---------------------------------------------------------------------------


def test_render_from_sqlite_store(tmp_path):
    store = SqliteStore(tmp_path / "sessions.db")
    try:
        session_id = store.create_session(
            workspace="D:/work/demo", provider="fake", model="fake-mini"
        )
        store.append_event(
            session_id,
            EventType.SESSION_START,
            {"workspace": "D:/work/demo", "provider": "fake", "model": "fake-mini"},
        )
        store.append_event(session_id, EventType.ROUND_START, {"round": 1})
        store.append_event(
            session_id,
            EventType.TOOL_CALL_START,
            {"call_id": "c1", "name": "bash", "arguments": {"command": "pytest -q"}},
        )
        store.append_event(
            session_id,
            EventType.TOOL_CALL_RESULT,
            {
                "call_id": "c1",
                "name": "bash",
                "success": True,
                "exit_code": 0,
                "output_preview": "3 passed",
            },
        )
        store.append_message(
            session_id,
            Message(
                role="user",
                content=[ToolResultBlock(tool_use_id="c1", content="3 passed", is_error=False)],
            ),
        )
        store.append_event(
            session_id,
            EventType.GOAL_CHECK,
            {"items": [{"item": "tests pass", "kind": "command", "passed": True, "exit_code": 0}]},
        )
        store.append_event(
            session_id,
            EventType.SESSION_END,
            {
                "exit_reason": "completed",
                "rounds": 1,
                "total_usage": {"input_tokens": 100, "output_tokens": 40, "total_tokens": 140},
            },
        )
        store.update_session(
            session_id,
            status="completed",
            exit_reason="completed",
            rounds=1,
            input_tokens=100,
            output_tokens=40,
        )

        summary = store.get_session(session_id)
        events = store.get_events(session_id)
        messages = store.get_messages(session_id)
    finally:
        store.close()

    assert summary is not None
    assert len(events) == 6
    html_out = render_session_html(summary, events, messages, full_outputs={"c1": "3 passed"})

    assert session_id in html_out
    assert "pytest -q" in html_out
    assert "3 passed" in html_out
    assert "tests pass" in html_out
    assert "session_end" in html_out
    assert "已完成" in html_out
    assert "140" in html_out  # total tokens in footer
    parse_ok(html_out)
