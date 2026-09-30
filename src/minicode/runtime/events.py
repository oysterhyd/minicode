"""Event recording: persistence plus optional live UI mirroring."""

from __future__ import annotations

from typing import Any, Awaitable, Callable

from minicode.core.models import Event, EventType, Message, ToolResultBlock
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
        await self.publish(event)
        return event

    async def checkpoint(
        self, type: EventType, data: dict[str, Any], *, message: Message | None = None,
        result: ToolResultBlock | None = None, counters: dict[str, Any] | None = None,
    ) -> Event:
        event = self.commit(type, data, message=message, result=result, counters=counters)
        await self.publish(event)
        return event

    def commit(self, type: EventType, data: dict[str, Any], *, message: Message | None = None,
               result: ToolResultBlock | None = None, counters: dict[str, Any] | None = None) -> Event:
        commit = getattr(self._store, "checkpoint", None)
        if commit is None:
            # Third-party stores can implement the same atomic contract.
            if message is not None:
                self._store.append_message(self._session_id, message)
            if counters:
                self._store.update_session(self._session_id, **counters)
            event = self._store.append_event(self._session_id, type, data)
        else:
            event = commit(self._session_id, type, data, message=message,
                           result=result, counters=counters)
        return event

    async def publish(self, event: Event) -> None:
        """Mirror a committed event without changing the transition."""
        if self._on_event is not None:
            try:
                await self._on_event(event.model_copy(deep=True))
            except Exception:
                # Presentation callbacks are observers. Their failure must not
                # interrupt an already-persisted agent transition.
                pass
