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


def test_round_slice_is_relative_to_resume_point():
    checker = BudgetChecker(Budget(max_rounds=2), start_usage=Usage(), start_rounds=20)
    assert not checker.rounds_exceeded(21)
    assert checker.rounds_exceeded(22)


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


# ---------------------------------------------------------------------------
# Uncapped token budget (the default)
#
# Regression guard for session 39e5f16a: a 200_000-token *cumulative* cap killed
# an ordinary 20-round session. Tokens are now uncapped unless asked for;
# rounds and wall-clock remain the bounds.
# ---------------------------------------------------------------------------


def test_zero_token_cap_never_fires():
    for cap in (0, -1):
        checker = BudgetChecker(Budget(max_total_tokens=cap), start_usage=Usage(), start_rounds=0)
        assert checker.tokens_exceeded(Usage(input_tokens=50_000_000)) is False


def test_budget_defaults_to_no_token_cap():
    budget = Budget()
    assert budget.max_total_tokens == 0
    checker = BudgetChecker(budget, start_usage=Usage(), start_rounds=0)
    assert checker.tokens_exceeded(Usage(input_tokens=10_000_000, output_tokens=1)) is False
