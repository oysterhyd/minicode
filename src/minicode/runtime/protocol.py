"""Bounded concurrent NDJSON dispatch for local hosts (protocol version 2)."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Awaitable, Callable


class RequestServer:
    def __init__(self, handle: Callable[[str, dict], Awaitable[object]],
                 emit: Callable[[dict], None], *, limit: int = 64):
        self.handle = handle
        self.emit = emit
        self.limit = limit
        self._pending: dict[str | int, asyncio.Task] = {}

    def submit(self, line: str) -> None:
        request_id = None
        try:
            if len(line.encode("utf-8")) > 4_000_000:
                raise ValueError("request exceeds the size limit")
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError("request must be an object")
            request_id = request.get("id")
            if isinstance(request_id, bool) or not isinstance(request_id, (str, int)):
                raise ValueError("request id must be a string or integer")
            method, params = request.get("method"), request.get("params", {})
            if not isinstance(method, str) or not isinstance(params, dict):
                raise ValueError("method must be a string and params must be an object")
            if request_id in self._pending:
                # A second response with this id would resolve the wrong promise.
                self.emit({"event": "bridge_error", "error": "duplicate in-flight request id"})
                return
            reserved = 8 if method in {"cancelTurn", "resolveApproval"} else 0
            if len(self._pending) >= self.limit + reserved:
                raise ValueError("request queue is full")
        except (ValueError, TypeError) as error:
            self.emit({"id": request_id, "error": str(error)} if isinstance(request_id, (str, int))
                      else {"event": "bridge_error", "error": str(error)})
            return
        task = asyncio.create_task(self._dispatch(request_id, method, params), name=f"rpc:{method}")
        self._pending[request_id] = task
        task.add_done_callback(lambda finished: self._pending.pop(request_id, None))

    async def _dispatch(self, request_id, method: str, params: dict) -> None:
        try:
            result = await self.handle(method, params)
        except asyncio.CancelledError:
            self.emit({"id": request_id, "error": "request cancelled"})
            raise
        except Exception as error:
            self.emit({"id": request_id, "error": str(error)})
        else:
            self.emit({"id": request_id, "result": result})

    async def aclose(self) -> None:
        pending = list(self._pending.values())
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        self._pending.clear()
