"""Small read-only stdio MCP server for a workspace's docs directory."""

from __future__ import annotations

import sys
from pathlib import Path

from mcp.server import MCPServer


ROOT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path("docs").resolve()
mcp = MCPServer("minicode-docs")


@mcp.tool()
def list_documents() -> list[str]:
    """List Markdown documents available in the configured docs directory."""
    if not ROOT.is_dir():
        return []
    return sorted(str(path.relative_to(ROOT)) for path in ROOT.rglob("*.md")
                  if path.is_file() and path.resolve().is_relative_to(ROOT))[:500]


@mcp.tool()
def read_document(path: str, offset: int = 0, limit: int = 16000) -> str:
    """Read a bounded UTF-8 slice of one Markdown document in the docs directory."""
    target = (ROOT / path).resolve()
    if not target.is_relative_to(ROOT) or not target.is_file() or target.suffix != ".md":
        raise ValueError("document is outside the configured docs directory")
    if offset < 0 or not 1 <= limit <= 64000:
        raise ValueError("offset or limit is outside allowed bounds")
    with target.open("rb") as stream:
        stream.seek(offset)
        data = stream.read(limit)
        has_more = bool(stream.read(1))
    return f"{data.decode('utf-8', errors='replace')}\n[has_more={has_more}; next_offset={offset + len(data)}]"


if __name__ == "__main__":
    mcp.run()
