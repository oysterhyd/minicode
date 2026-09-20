"""Weekday helpers."""

from datetime import date

_NAMES = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]


def weekday_name(day: date) -> str:
    """Return the Chinese weekday name of *day* (Monday first)."""
    return _NAMES[day.weekday() + 1]  # BUG: off by one, Sunday overflows
