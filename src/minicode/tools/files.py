"""File tools aligned with the Pi agent's core tool set.

``read`` / ``ls`` are read-only; ``edit`` / ``write`` mutate files and are
gated by the permission policy. All paths are resolved strictly inside the
workspace.
"""

from __future__ import annotations

import difflib
import os
from pathlib import Path

from pydantic import BaseModel, Field

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, resolve_or_fail, truncate_output

# Directory names never descended into when walking the workspace
# (listing / searching tools and the goals workspace fingerprint).
SKIP_DIRS = {
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    "node_modules",
    ".minicode",
    ".pytest_cache",
    ".ruff_cache",
}


def _relpath(target: Path, ctx: ToolContext) -> str:
    """Workspace-relative posix path for a resolved *target*."""
    try:
        return target.relative_to(Path(ctx.workspace).resolve()).as_posix()
    except ValueError:  # pragma: no cover - containment guaranteed by resolve_in_workspace
        return target.name


def _read_text(target: Path, limit_bytes: int) -> tuple[str | None, ToolOutcome | None]:
    """Read *target* as UTF-8 capped at *limit_bytes*.

    Returns ``(text, None)`` on success or ``(None, failure_outcome)`` for
    binary content / unreadable files, so callers fail uniformly.
    """
    try:
        size = target.stat().st_size
    except OSError as exc:
        return None, ToolOutcome.failure(f"failed to stat file: {exc}")
    if size > limit_bytes:
        return None, ToolOutcome.failure(
            f"file too large: {size} bytes (limit {limit_bytes})"
        )
    try:
        return target.read_text(encoding="utf-8"), None
    except UnicodeDecodeError:
        return None, ToolOutcome.failure("binary or non-UTF-8 file")
    except OSError as exc:
        return None, ToolOutcome.failure(f"failed to read file: {exc}")


# ---------------------------------------------------------------------------
# read
# ---------------------------------------------------------------------------


class ReadArgs(BaseModel):
    path: str
    offset: int = Field(default=1, ge=1, description="1-based line number of the first line to return")
    limit: int | None = Field(default=None, gt=0, description="maximum number of lines to return")


class ReadTool(BaseTool):
    name = "read"
    description = (
        "Read a UTF-8 text file from the workspace and return it with "
        "cat -n style line numbers. Supports 1-based offset and line limit."
    )
    requires_approval = False
    args_model = ReadArgs

    async def execute(self, args: ReadArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not target.is_file():
            return ToolOutcome.failure(f"path is not a file: {args.path}")

        text, failure = _read_text(target, ctx.limits.max_read_bytes)
        if failure is not None:
            return failure

        lines = text.splitlines()
        if args.offset > len(lines):
            return ToolOutcome.failure(
                f"offset {args.offset} beyond end of file ({len(lines)} lines)"
            )
        start = args.offset - 1
        end = len(lines) if args.limit is None else start + args.limit
        selected = lines[start:end]
        numbered = "\n".join(
            f"{lineno:>6}\t{line}" for lineno, line in enumerate(selected, start=args.offset)
        )
        return ToolOutcome(output=truncate_output(numbered, ctx.limits.max_output_chars))


# ---------------------------------------------------------------------------
# ls
# ---------------------------------------------------------------------------


class LsArgs(BaseModel):
    path: str = "."


class LsTool(BaseTool):
    name = "ls"
    description = (
        "List files and directories under a workspace directory, sorted, with "
        "trailing '/' on directories. Common junk directories are skipped."
    )
    requires_approval = False
    args_model = LsArgs

    _MAX_ENTRIES = 500

    async def execute(self, args: LsArgs, ctx: ToolContext) -> ToolOutcome:
        base, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not base.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.path}")

        entries: list[str] = []
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            rel_dir = os.path.relpath(dirpath, base)
            prefix = "" if rel_dir == "." else Path(rel_dir).as_posix() + "/"
            for dirname in dirnames:
                entries.append(f"{prefix}{dirname}/")
            for filename in sorted(filenames):
                entries.append(f"{prefix}{filename}")

        entries.sort()
        if len(entries) > self._MAX_ENTRIES:
            omitted = len(entries) - self._MAX_ENTRIES
            entries = entries[: self._MAX_ENTRIES]
            entries.append(f"...[{omitted} more entries]")
        return ToolOutcome(output=truncate_output("\n".join(entries), ctx.limits.max_output_chars))


# ---------------------------------------------------------------------------
# edit
# ---------------------------------------------------------------------------


class EditArgs(BaseModel):
    path: str
    old_text: str = Field(
        min_length=1, description="exact text to replace; must match the file exactly"
    )
    new_text: str = Field(description="replacement text")
    replace_all: bool = Field(
        default=False,
        description="replace every occurrence instead of failing on ambiguous matches",
    )


class EditTool(BaseTool):
    name = "edit"
    description = (
        "Replace exact text in an existing file and return a unified diff. "
        "Refuses missing matches and (unless replace_all) ambiguous matches "
        "instead of guessing."
    )
    requires_approval = True
    args_model = EditArgs

    async def execute(self, args: EditArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        relpath = _relpath(target, ctx)
        if not target.is_file():
            return ToolOutcome.failure(
                f"path is not a file: {relpath} (use write to create a new file)"
            )
        text, failure = _read_text(target, ctx.limits.max_read_bytes)
        if failure is not None:
            return failure

        count = text.count(args.old_text)
        if count == 0:
            return ToolOutcome.failure(
                f"old_text not found in {relpath}: file may have changed; re-read the file first"
            )
        if count > 1 and not args.replace_all:
            return ToolOutcome.failure(
                f"old_text matches {count} locations in {relpath}; "
                "include more surrounding context to make it unique, "
                "or pass replace_all=true"
            )

        occurrences = count if args.replace_all else 1
        updated = text.replace(args.old_text, args.new_text, occurrences)
        try:
            target.write_text(updated, encoding="utf-8")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")
        return ToolOutcome(output=_diff_output(relpath, text, updated, ctx))


# ---------------------------------------------------------------------------
# write
# ---------------------------------------------------------------------------


class WriteArgs(BaseModel):
    path: str
    content: str = Field(description="full file content; overwrites existing files")


class WriteTool(BaseTool):
    name = "write"
    description = (
        "Create a new file or overwrite an existing one with the given "
        "content, returning a unified diff against the previous state. "
        "Prefer edit for changing existing files."
    )
    requires_approval = True
    args_model = WriteArgs

    async def execute(self, args: WriteArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        relpath = _relpath(target, ctx)

        previous = ""
        if target.exists():
            if not target.is_file():
                return ToolOutcome.failure(f"path exists and is not a file: {relpath}")
            text, failure = _read_text(target, ctx.limits.max_read_bytes)
            if failure is not None:
                return failure
            previous = text

        try:
            os.makedirs(target.parent, exist_ok=True)
            target.write_text(args.content, encoding="utf-8")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")
        return ToolOutcome(output=_diff_output(relpath, previous, args.content, ctx))


def _diff_output(relpath: str, before: str, after: str, ctx: ToolContext) -> str:
    """Unified diff header line ('Applied ... (+n -m)') plus the diff body."""
    diff_lines = list(
        difflib.unified_diff(
            before.splitlines(),
            after.splitlines(),
            fromfile=f"a/{relpath}",
            tofile=f"b/{relpath}",
            lineterm="",
        )
    )
    adds = sum(
        1 for line in diff_lines if line.startswith("+") and not line.startswith("+++")
    )
    dels = sum(
        1 for line in diff_lines if line.startswith("-") and not line.startswith("---")
    )
    header = f"Applied change to {relpath} (+{adds} -{dels})"
    return truncate_output(header + "\n" + "\n".join(diff_lines), ctx.limits.max_output_chars)
