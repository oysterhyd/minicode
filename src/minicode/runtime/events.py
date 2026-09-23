"""Event recording: persistence plus optional live UI mirroring."""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from minicode.core.models import Event, EventType
from minicode.storage import SessionStore

#: Async hook mirrored every persisted event (UI / test observers).
EventCallback = Callable[[Event], Awaitable[None]]


class EventRecorder:
    """Persists events via the store and mirrors them to an optional async
    UI callback.

    Every event first receives its per-session sequence number from the
    store (persistence is the source of truth); the callback, when set, is
    awaited afterwards with the persisted :class:`Event`.
    """

    def __init__(
        self,
        store: SessionStore,
        session_id: str,
        on_event: EventCallback | None = None,
    ) -> None:
        self._store = store
        self._session_id = session_id
        self._on_event = on_event

    async def emit(self, type: EventType, data: dict[str, Any] | None = None) -> Event:
        """Persist one event, mirror it to the callback (if any), return it."""
        event = self._store.append_event(self._session_id, type, data)
        if self._on_event is not None:
            try:
                await self._on_event(event)
            except Exception:
                # Presentation callbacks are observers. Their failure must not
                # interrupt an already-persisted agent transition.
                pass
        return event
