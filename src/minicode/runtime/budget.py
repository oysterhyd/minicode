"""Budget guard for the agent loop."""

from __future__ import annotations

import time

from minicode.core.models import Budget, Usage


class BudgetChecker:
    """Per-turn budget guard over session-cumulative counters.

    Wall-clock deadline is per turn; rounds/tokens are session-cumulative.
    Construct one checker per ``run_turn`` call, seeding it with the
    session's cumulative usage and round count so the caps apply across
    turns, while the wall-clock deadline restarts each turn.
    """

    def __init__(self, budget: Budget, *, start_usage: Usage, start_rounds: int) -> None:
        self.budget = budget
        self._base_usage = start_usage
        self._base_rounds = start_rounds
        self.deadline = time.monotonic() + budget.max_seconds

    def time_exceeded(self) -> bool:
        """True once this turn's wall-clock deadline has passed."""
        return time.monotonic() >= self.deadline

    def rounds_exceeded(self, rounds: int) -> bool:
        """True when *rounds* (session-cumulative) has reached the cap."""
        return rounds >= self.budget.max_rounds

    def tokens_exceeded(self, usage: Usage) -> bool:
        """True when *usage* (session-cumulative) exceeds the token cap."""
        return usage.total_tokens > self.budget.max_total_tokens
