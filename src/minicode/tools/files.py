"""File tools: read, list and patch files strictly inside the workspace."""

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


# ---------------------------------------------------------------------------
# read_file
# ---------------------------------------------------------------------------


class ReadFileArgs(BaseModel):
    path: str
    offset: int = Field(default=1, ge=1, description="1-based line number of the first line to return")
    limit: int | None = Field(default=None, gt=0, description="maximum number of lines to return")


class ReadFileTool(BaseTool):
    name = "read_file"
    description = (
        "Read a UTF-8 text file from the workspace and return it with "
        "cat -n style line numbers. Supports 1-based offset and line limit."
    )
    requires_approval = False
    args_model = ReadFileArgs

    async def execute(self, args: ReadFileArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not target.is_file():
            return ToolOutcome.failure(f"path is not a file: {args.path}")

        size = target.stat().st_size
        if size > ctx.limits.max_read_bytes:
            return ToolOutcome.failure(
                f"file too large: {size} bytes (limit {ctx.limits.max_read_bytes})"
            )
        try:
            text = target.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            return ToolOutcome.failure("binary or non-UTF-8 file")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to read file: {exc}")

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
# list_files
# ---------------------------------------------------------------------------


class ListFilesArgs(BaseModel):
    path: str = "."


class ListFilesTool(BaseTool):
    name = "list_files"
    description = (
        "List files and directories under a workspace directory, sorted, with "
        "trailing '/' on directories. Common junk directories are skipped."
    )
    requires_approval = False
    args_model = ListFilesArgs

    _MAX_ENTRIES = 500

    async def execute(self, args: ListFilesArgs, ctx: ToolContext) -> ToolOutcome:
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
# apply_patch
# ---------------------------------------------------------------------------


class ApplyPatchArgs(BaseModel):
    path: str
    old_text: str | None = Field(
        default=None,
        description="exact text to replace; omit or pass empty to create a new file",
    )
    new_text: str


class ApplyPatchTool(BaseTool):
    name = "apply_patch"
    description = (
        "Create a new file (no old_text) or replace exactly one occurrence of "
        "old_text in an existing file. Returns a unified diff; refuses "
        "ambiguous or missing matches instead of guessing."
    )
    requires_approval = True
    args_model = ApplyPatchArgs

    async def execute(self, args: ApplyPatchArgs, ctx: ToolContext) -> ToolOutcome:
        target, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        relpath = _relpath(target, ctx)

        old_text = args.old_text or ""
        if not old_text:
            return self._create(target, relpath, args.new_text, ctx)
        return self._edit(target, relpath, old_text, args.new_text, ctx)

    def _create(self, target: Path, relpath: str, new_text: str, ctx: ToolContext) -> ToolOutcome:
        if target.exists():
            return ToolOutcome.failure("file already exists; provide old_text to edit it")
        try:
            os.makedirs(target.parent, exist_ok=True)
            target.write_text(new_text, encoding="utf-8")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")
        diff = difflib.unified_diff(
            [],
            new_text.splitlines(),
            fromfile=f"a/{relpath}",
            tofile=f"b/{relpath}",
            lineterm="",
        )
        return ToolOutcome(output=truncate_output("\n".join(diff), ctx.limits.max_output_chars))

    def _edit(
        self,
        target: Path,
        relpath: str,
        old_text: str,
        new_text: str,
        ctx: ToolContext,
    ) -> ToolOutcome:
        if not target.is_file():
            return ToolOutcome.failure(f"path is not a file: {relpath}")
        try:
            text = target.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            return ToolOutcome.failure("binary or non-UTF-8 file")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to read file: {exc}")

        count = text.count(old_text)
        if count == 0:
            return ToolOutcome.failure(
                f"old_text not found in {relpath}: file may have changed; re-read the file first"
            )
        if count > 1:
            return ToolOutcome.failure(
                f"old_text matches {count} locations in {relpath}; "
                "include more surrounding context to make it unique"
            )

        updated = text.replace(old_text, new_text, 1)
        try:
            target.write_text(updated, encoding="utf-8")
        except OSError as exc:
            return ToolOutcome.failure(f"failed to write file: {exc}")

        diff_lines = list(
            difflib.unified_diff(
                text.splitlines(),
                updated.splitlines(),
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
        header = f"Applied patch to {relpath} (+{adds} -{dels})"
        output = header + "\n" + "\n".join(diff_lines)
        return ToolOutcome(output=truncate_output(output, ctx.limits.max_output_chars))
