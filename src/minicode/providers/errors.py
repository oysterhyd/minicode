"""Provider error hierarchy surfaced to the runtime.

Any failure inside a provider (transport, auth, malformed request, ...) is
raised as a :class:`ProviderError` subclass instead of leaking SDK-specific
exceptions. The runtime maps these onto ``ExitReason.PROVIDER_ERROR``.
"""

from __future__ import annotations


class ProviderError(Exception):
    """Any provider failure surfaced to the runtime."""


class ProviderAuthError(ProviderError):
    """Auth/permission failure; not retryable without new credentials."""


class ProviderRequestError(ProviderError):
    """Invalid request (malformed messages/tools); not retryable as-is."""
