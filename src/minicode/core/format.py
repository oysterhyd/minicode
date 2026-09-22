"""Compact display formatting for terminal output.

Presentation only: stored session data and the HTML report keep exact numbers;
anything a user reads in the CLI or the TUI goes through here so a 300 000
token budget reads as ``300k`` and a 1 048 576 window as ``1M``.
"""

from __future__ import annotations

__all__ = ["format_tokens"]


def _trim(number: float) -> str:
    """One decimal, without a trailing ``.0`` (``300.0`` -> ``300``)."""
    return f"{number:.1f}".rstrip("0").rstrip(".")


def format_tokens(value: int) -> str:
    """Compact token count: ``950`` / ``12.3k`` / ``300k`` / ``1M``.

    ``1_048_576`` prints as ``1M`` and ``999_999`` as ``1M`` rather than
    ``1000k``, so the largest value in a unit never overflows it.
    """
    sign = "-" if value < 0 else ""
    magnitude = abs(value)
    if magnitude < 1_000:
        return f"{sign}{magnitude}"
    # 999_950 is where one decimal of k rounds to 1000.0 -> promote to M.
    if magnitude < 999_950:
        return f"{sign}{_trim(magnitude / 1_000)}k"
    return f"{sign}{_trim(magnitude / 1_000_000)}M"
