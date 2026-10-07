"""Shared protocol capture limits, independent of providers and runtime."""

MAX_RESPONSE_BYTES = 16_000_000
MAX_TOOL_CALLS = 128
# Bound transport overhead and reasoning-only streams as well as visible output.
MAX_SSE_BYTES = 64_000_000
MAX_SSE_LINE_BYTES = MAX_RESPONSE_BYTES
