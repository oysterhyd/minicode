"""Bounded sparse character -> TextIO seek-cookie indexes for immutable logs.

Cookies, not raw byte offsets, preserve UTF-8 and exact newline semantics.
A cold random read still scans its prefix; subsequent sequential pages reuse
one cursor, and repeated indexed reads skip at most one 64-Kcharacter interval.
"""
from __future__ import annotations

from bisect import bisect_right
from collections import OrderedDict
from dataclasses import dataclass, field
from pathlib import Path
from threading import Lock

_INTERVAL = 64 * 1024
_MAX_FILES = 32
_MAX_ANCHORS = 4096


@dataclass
class _Index:
    lock: Lock = field(default_factory=Lock)
    signature: tuple[int, int, int, int] | None = None
    offsets: list[int] = field(default_factory=lambda: [0])
    cookies: list[int] = field(default_factory=lambda: [0])
    cursor_offset: int = 0
    cursor_cookie: int = 0

    def remember(self, offset: int, cookie: int) -> None:
        if offset % _INTERVAL or len(self.offsets) >= _MAX_ANCHORS:
            return
        position = bisect_right(self.offsets, offset)
        if position and self.offsets[position - 1] == offset:
            return
        self.offsets.insert(position, offset)
        self.cookies.insert(position, cookie)


class TextPageReader:
    def __init__(self) -> None:
        self._indices: OrderedDict[Path, _Index] = OrderedDict()
        self._lock = Lock()

    def _index(self, path: Path) -> _Index:
        with self._lock:
            index = self._indices.get(path)
            if index is None:
                index = _Index()
                self._indices[path] = index
                if len(self._indices) > _MAX_FILES:
                    self._indices.popitem(last=False)
            else:
                self._indices.move_to_end(path)
            return index

    def read(self, path: Path, offset: int, limit: int) -> tuple[str, int | None, bool] | None:
        if offset < 0 or limit <= 0:
            raise ValueError("offset must be nonnegative and limit must be positive")
        index = self._index(path)
        try:
            with index.lock, path.open("r", encoding="utf-8", newline="") as handle:
                stat = path.stat()
                signature = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns)
                if signature != index.signature:
                    index.signature = signature
                    index.offsets[:] = [0]
                    index.cookies[:] = [0]
                    index.cursor_offset = index.cursor_cookie = 0
                anchor = bisect_right(index.offsets, offset) - 1
                skipped, cookie = index.offsets[anchor], index.cookies[anchor]
                if skipped < index.cursor_offset <= offset:
                    skipped, cookie = index.cursor_offset, index.cursor_cookie
                handle.seek(cookie)
                while skipped < offset:
                    chunk = handle.read(min(_INTERVAL - skipped % _INTERVAL, offset - skipped))
                    skipped += len(chunk)
                    if not chunk:
                        index.cursor_offset, index.cursor_cookie = skipped, handle.tell()
                        return "", skipped, False
                    index.remember(skipped, handle.tell())
                parts: list[str] = []
                end = offset
                while end < offset + limit:
                    chunk = handle.read(min(_INTERVAL - end % _INTERVAL, offset + limit - end))
                    if not chunk:
                        break
                    parts.append(chunk)
                    end += len(chunk)
                    index.remember(end, handle.tell())
                index.cursor_offset, index.cursor_cookie = end, handle.tell()
                has_more = bool(handle.read(1))
                return "".join(parts), None if has_more else end, has_more
        except (OSError, UnicodeDecodeError):
            return None
