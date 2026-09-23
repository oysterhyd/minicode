"""File tools aligned with the Pi agent's core tool set.

``read`` / ``ls`` are read-only; ``edit`` / ``write`` mutate files and are
gated by the permission policy. All paths are resolved strictly inside the
workspace.
"""

from __future__ import annotations

import asyncio
import codecs
import contextlib
import difflib
from itertools import chain
import mmap
import os
from pathlib import Path
import shutil
import tempfile

from pydantic import BaseModel, Field

from minicode.core.models import ToolOutcome
from minicode.tools.base import (
    BaseTool,
    ToolContext,
    bounded_output,
    resolve_or_fail,
)

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


def _atomic_write_text(target: Path, content: str) -> None:
    """Replace a file only after its new content is fully written on disk."""
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=f".{target.name}.", dir=target.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        if target.exists():
            os.chmod(name, target.stat().st_mode)
        os.replace(name, target)
    finally:
        with contextlib.suppress(OSError):
            Path(name).unlink(missing_ok=True)


def _edit_large_file(target: Path, args: "EditArgs", relpath: str) -> ToolOutcome:
    """Replace exact UTF-8 bytes in a large file using bounded copying."""
    old = args.old_text.encode("utf-8")
    new = args.new_text.encode("utf-8")
    original_stat = target.stat()
    with target.open("rb") as source:
        decoder = codecs.getincrementaldecoder("utf-8")(errors="strict")
        try:
            for chunk in iter(lambda: source.read(64 * 1024), b""):
                decoder.decode(chunk)
            decoder.decode(b"", final=True)
        except UnicodeDecodeError:
            return ToolOutcome.failure("binary or non-UTF-8 file")
        source.seek(0)
        with mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as mapped:
            count = 0
            position = 0
            while (found := mapped.find(old, position)) != -1:
                count += 1
                position = found + len(old)
            if count == 0:
                return ToolOutcome.failure(
                    f"old_text not found in {relpath}: file may have changed; re-read the file first"
                )
            if count > 1 and not args.replace_all:
                return ToolOutcome.failure(
                    f"old_text matches {count} locations in {relpath}; "
                    "include more surrounding context to make it unique, or pass replace_all=true"
                )
            fd, name = tempfile.mkstemp(prefix=f".{target.name}.", dir=target.parent)
            try:
                with os.fdopen(fd, "wb") as output:
                    position = 0
                    replacements = 0
                    while (found := mapped.find(old, position)) != -1:
                        source.seek(position)
                        remaining = found - position
                        while remaining:
                            chunk = source.read(min(64 * 1024, remaining))
                            output.write(chunk)
                            remaining -= len(chunk)
                        output.write(new)
                        position = found + len(old)
                        replacements += 1
                        if not args.replace_all:
                            break
                    source.seek(position)
                    shutil.copyfileobj(source, output, length=64 * 1024)
                    output.flush()
                    os.fsync(output.fileno())
                os.chmod(name, original_stat.st_mode)
            except BaseException:
                with contextlib.suppress(OSError):
                    Path(name).unlink(missing_ok=True)
                raise
    try:
        current_stat = target.stat()
        if (current_stat.st_size, current_stat.st_mtime_ns) != (
            original_stat.st_size, original_stat.st_mtime_ns
        ):
            return ToolOutcome.failure(f"file changed while editing {relpath}; re-read and retry")
        os.replace(name, target)
    finally:
        with contextlib.suppress(OSError):
            Path(name).unlink(missing_ok=True)
    return ToolOutcome(output=(
        f"Applied change to large file {relpath} ({replacements} replacement(s)); "
        f"old={original_stat.st_size} bytes, "
        f"new={original_stat.st_size + replacements * (len(new) - len(old))} bytes."
    ))


# ---------------------------------------------------------------------------
# read
# ---------------------------------------------------------------------------


class ReadArgs(BaseModel):
    path: str
    offset: int = Field(default=1, ge=1, description="1-based line number of the first line to return")
    limit: int | None = Field(default=None, gt=0, description="maximum number of lines to return")
    cursor: int | None = Field(default=None, ge=0, description="opaque next_cursor from a previous page")


class ReadTool(BaseTool):
    name = "read"
    description = (
        "Read a bounded page of any-size UTF-8 file with line numbers. "
        "Use next_offset and next_cursor from the result to continue, "
        "including through a single very long line."
    )
    requires_approval = False
    args_model = ReadArgs

    async def execute(self, args: ReadArgs, ctx: ToolContext) -> ToolOutcome:
        return await asyncio.to_thread(self._read, args, ctx)

    @staticmethod
    def _read(args: ReadArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not target.is_file():
            return ToolOutcome.failure(f"path is not a file: {args.path}")

        page_chars = min(ctx.limits.max_output_chars - 200, ctx.limits.max_read_bytes // 4)
        if page_chars < 10:
            return ToolOutcome.failure("read page budget is too small (need at least 10 characters)")
        line_limit = args.limit if args.limit is not None else 200
        try:
            with target.open("r", encoding="utf-8", newline=None) as handle:
                if args.cursor is not None:
                    handle.seek(args.cursor)
                else:
                    # Skip by bounded chunks so a huge single line does not
                    # have to be materialized just to reach a later line.
                    skipped = 0
                    while skipped < args.offset - 1:
                        fragment = handle.readline(page_chars)
                        if not fragment:
                            return ToolOutcome.failure(
                                f"offset {args.offset} beyond end of file ({skipped} lines)"
                            )
                        if fragment.endswith("\n"):
                            skipped += 1

                lines: list[str] = []
                line_number = args.offset
                remaining = page_chars
                while len(lines) < line_limit and remaining > 8:
                    prefix = f"{line_number:>6}\t"
                    fragment = handle.readline(max(1, remaining - len(prefix)))
                    if not fragment:
                        break
                    complete_line = fragment.endswith("\n")
                    lines.append(prefix + fragment.rstrip("\r\n"))
                    remaining -= len(lines[-1]) + 1
                    if complete_line:
                        line_number += 1
                    else:
                        break
                if not lines:
                    return ToolOutcome.failure(
                        f"offset {args.offset} beyond end of file ({args.offset - 1} lines)"
                    )
                next_cursor = handle.tell()
                more = bool(handle.read(1))
                output = "\n".join(lines)
                if more:
                    output += f"\n...[更多内容：next_offset={line_number}, next_cursor={next_cursor}]"
                return ToolOutcome(output=output)
        except UnicodeDecodeError:
            return ToolOutcome.failure("binary or non-UTF-8 file")
        except (OSError, ValueError) as exc:
            return ToolOutcome.failure(f"failed to read file: {exc}")


# ---------------------------------------------------------------------------
# ls
# ---------------------------------------------------------------------------


class LsArgs(BaseModel):
    path: str = "."
    offset: int = Field(default=0, ge=0, description="zero-based entry offset for paging")
    limit: int = Field(default=500, ge=1, le=500, description="entries in one page")


class LsTool(BaseTool):
    name = "ls"
    description = (
        "List a bounded page of files and directories under a workspace directory. "
        "Use next_offset to continue; narrow path for large trees."
    )
    requires_approval = False
    args_model = LsArgs

    async def execute(self, args: LsArgs, ctx: ToolContext) -> ToolOutcome:
        return await asyncio.to_thread(self._list, args, ctx)

    def _list(self, args: LsArgs, ctx: ToolContext) -> ToolOutcome:
        base, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not base.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.path}")

        entries: list[str] = []
        seen = 0
        has_more = False
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            rel_dir = os.path.relpath(dirpath, base)
            prefix = "" if rel_dir == "." else Path(rel_dir).as_posix() + "/"
            for entry in chain(
                (f"{prefix}{name}/" for name in dirnames),
                (f"{prefix}{name}" for name in sorted(filenames)),
            ):
                if seen >= args.offset:
                    if len(entries) >= args.limit:
                        has_more = True
                        break
                    entries.append(entry)
                seen += 1
            if has_more:
                break
        entries.sort()
        if has_more:
            entries.append(f"...[达到列表页上限；next_offset={args.offset + len(entries)}]")
        return bounded_output("\n".join(entries), ctx.limits.max_output_chars)


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
        try:
            if target.stat().st_size > ctx.limits.max_read_bytes:
                return _edit_large_file(target, args, relpath)
        except OSError as exc:
            return ToolOutcome.failure(f"failed to edit file: {exc}")
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
            _atomic_write_text(target, updated)
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")
        return _diff_output(relpath, text, updated, ctx)


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
        previous_large_size: int | None = None
        if target.exists():
            if not target.is_file():
                return ToolOutcome.failure(f"path exists and is not a file: {relpath}")
            try:
                size = target.stat().st_size
            except OSError as exc:
                return ToolOutcome.failure(f"failed to stat file: {exc}")
            if size > ctx.limits.max_read_bytes:
                previous_large_size = size
            else:
                text, failure = _read_text(target, ctx.limits.max_read_bytes)
                if failure is not None:
                    return failure
                previous = text

        try:
            _atomic_write_text(target, args.content)
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")
        if previous_large_size is not None or len(args.content.encode("utf-8")) > ctx.limits.max_read_bytes:
            old_label = f"{previous_large_size} bytes" if previous_large_size is not None else "new file"
            return ToolOutcome(output=(
                f"Applied write to {relpath}; old={old_label}, "
                f"new={len(args.content.encode('utf-8'))} bytes. Large diff omitted; use read pages to inspect."
            ))
        return _diff_output(relpath, previous, args.content, ctx)


def _diff_output(relpath: str, before: str, after: str, ctx: ToolContext) -> ToolOutcome:
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
    text = header + "\n" + "\n".join(diff_lines)
    return bounded_output(text, ctx.limits.max_output_chars)
