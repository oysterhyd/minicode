"""Provider error hierarchy surfaced to the runtime.

Any failure inside a provider (transport, auth, malformed request, ...) is
raised as a :class:`ProviderError` subclass instead of leaking SDK-specific
exceptions. The runtime maps these onto ``ExitReason.PROVIDER_ERROR``.
"""

from __future__ import annotations


class ProviderError(Exception):
    """Any provider failure surfaced to the runtime."""

    retryable = True

    def __init__(self, message: str, *, retry_after: float | None = None):
        super().__init__(message)
        self.retry_after = retry_after


class ProviderAuthError(ProviderError):
    """Auth/permission failure; not retryable without new credentials."""
    retryable = False


class ProviderRequestError(ProviderError):
    """Invalid request (malformed messages/tools); not retryable as-is."""
    retryable = False


class ProviderProtocolError(ProviderError):
    """A malformed or incomplete response must never authorize tools."""
    retryable = False
