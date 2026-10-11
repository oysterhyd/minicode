"""User-editable global AGENTS.md, shared by desktop and command-line hosts."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

from minicode.core.paths import is_link_or_junction

MAX_INSTRUCTIONS_BYTES = 64_000


class GlobalInstructions:
    def __init__(self, path: Path | None = None):
        self.path = path or Path.home() / ".minicode" / "AGENTS.md"

    def _check_path(self) -> None:
        if any(os.path.lexists(path) and is_link_or_junction(path) for path in (self.path, self.path.parent)):
            raise ValueError("global instructions path is a link or junction")

    def read(self) -> dict:
        self._check_path()
        if not self.path.exists():
            return {"path": str(self.path), "content": "", "exists": False}
        with self.path.open("rb") as stream:
            data = stream.read(MAX_INSTRUCTIONS_BYTES + 1)
        if len(data) > MAX_INSTRUCTIONS_BYTES:
            raise ValueError("global instructions exceed 64 KB")
        return {"path": str(self.path), "content": data.decode("utf-8-sig"), "exists": True}

    def save(self, content: str) -> dict:
        if not isinstance(content, str):
            raise ValueError("instructions content must be a string")
        data = content.encode("utf-8")
        if len(data) > MAX_INSTRUCTIONS_BYTES:
            raise ValueError("global instructions exceed 64 KB")
        self._check_path()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=self.path.parent, prefix=".agents-", suffix=".tmp", delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            self._check_path()
            os.replace(temporary, self.path)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        return self.read()
