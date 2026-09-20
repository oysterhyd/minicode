"""Shared UTC timestamp helper (the storage convention across minicode)."""

from __future__ import annotations

from datetime import datetime, timezone

__all__ = ["utc_now"]


def utc_now() -> str:
    """UTC ISO-8601 timestamp with timezone offset (storage convention)."""
    return datetime.now(timezone.utc).isoformat()
