"""Tests for minicode.runtime.budget (BudgetChecker) — plain sync pytest."""

from __future__ import annotations

import time

from minicode.core.models import Budget, Usage
from minicode.runtime.budget import BudgetChecker


def test_time_deadline_honored_with_zero_seconds():
    """max_seconds=0: the deadline is construction time, so it is already past."""
    checker = BudgetChecker(Budget(max_seconds=0), start_usage=Usage(), start_rounds=0)
    assert checker.time_exceeded() is True


def test_time_deadline_not_reached_with_headroom():
    checker = BudgetChecker(Budget(max_seconds=60.0), start_usage=Usage(), start_rounds=0)
    assert checker.time_exceeded() is False


def test_deadline_is_per_turn_measured_from_construction():
    """The wall clock counts from checker construction, not session start."""
    time.sleep(0.01)
    checker = BudgetChecker(Budget(max_seconds=0.05), start_usage=Usage(), start_rounds=9)
    assert checker.time_exceeded() is False
    time.sleep(0.08)
    assert checker.time_exceeded() is True


def test_rounds_threshold_is_inclusive():
    checker = BudgetChecker(Budget(max_rounds=3), start_usage=Usage(), start_rounds=0)
    assert checker.rounds_exceeded(2) is False
    assert checker.rounds_exceeded(3) is True   # exactly at the cap counts as exceeded
    assert checker.rounds_exceeded(4) is True


def test_tokens_threshold_strictly_greater():
    checker = BudgetChecker(Budget(max_total_tokens=1000), start_usage=Usage(), start_rounds=0)
    # exactly at the cap is still fine (strict > comparison)
    assert checker.tokens_exceeded(Usage(input_tokens=400, output_tokens=600)) is False
    assert checker.tokens_exceeded(Usage(input_tokens=400, output_tokens=601)) is True


def test_below_threshold_all_checks_false():
    checker = BudgetChecker(
        Budget(max_rounds=10, max_total_tokens=10_000, max_seconds=60.0),
        start_usage=Usage(input_tokens=5, output_tokens=5),
        start_rounds=4,
    )
    assert checker.time_exceeded() is False
    assert checker.rounds_exceeded(5) is False
    assert checker.tokens_exceeded(Usage(input_tokens=50, output_tokens=50)) is False
