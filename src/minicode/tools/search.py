"""Text search tool: ripgrep-accelerated with a deterministic Python fallback."""

from __future__ import annotations

import asyncio
import fnmatch
import os
import re
import shutil
import time
from pathlib import Path

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, bounded_output, resolve_or_fail
from minicode.tools.files import SKIP_DIRS

_RG_TIMEOUT_S = 30.0
_RG_MAX_COUNT = 50
_MAX_LINE_CHARS = 200
_BINARY_SNIFF_BYTES = 8192

# rg output line: <path>:<line>:<text> (relative paths contain no colons).
_RG_LINE_RE = re.compile(r"^(?P<path>[^:]+):(?P<lineno>\d+):(?P<text>.*)$")


class GrepArgs(BaseModel):
    pattern: str
    path: str = "."
    glob: str | None = None
    max_results: int | None = None
    case_sensitive: bool = False


class GrepTool(BaseTool):
    name = "grep"
    description = (
        "Search file contents in the workspace with a regular expression and "
        "return 'path:line: text' match lines. Optional filename glob filter; "
        "case-insensitive by default."
    )
    requires_approval = False
    args_model = GrepArgs

    async def execute(self, args: GrepArgs, ctx: ToolContext) -> ToolOutcome:
        base, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not base.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.path}")

        effective_max = (
            ctx.limits.max_search_results if args.max_results is None else args.max_results
        )
        effective_max = max(1, min(effective_max, ctx.limits.max_search_results))

        # Validate up front so both strategies fail identically on bad patterns.
        flags = 0 if args.case_sensitive else re.IGNORECASE
        try:
            regex = re.compile(args.pattern, flags)
        except re.error as exc:
            return ToolOutcome.failure(f"invalid regex: {exc}")

        try:
            matches = await self._search_with_ripgrep(args, base, ctx, effective_max)
            if matches is None:
                matches = await asyncio.to_thread(
                    self._python_search, regex, base, args.glob, effective_max, ctx
                )
        except TimeoutError:
            return ToolOutcome.failure("search timed out; narrow path or glob and retry")
        if not matches:
            return ToolOutcome(output="(no matches)")
        return bounded_output("\n".join(matches), ctx.limits.max_output_chars)

    # ------------------------------------------------------------------
    # ripgrep strategy: returns match lines, [] for no matches, or None
    # when ripgrep is unavailable / unusable and the fallback should run.
    # ------------------------------------------------------------------

    async def _search_with_ripgrep(
        self,
        args: GrepArgs,
        base: Path,
        ctx: ToolContext,
        effective_max: int,
    ) -> list[str] | None:
        if shutil.which("rg") is None:
            return None
        try:
            rel = base.relative_to(Path(ctx.workspace).resolve()).as_posix()
        except ValueError:  # pragma: no cover - containment guaranteed upstream
            return None
        rel_base = "." if rel in ("", ".") else rel

        argv = [
            "rg", "--no-heading", "--line-number", "--color", "never",
            "--max-columns", str(_MAX_LINE_CHARS), "--max-columns-preview",
        ]
        if not args.case_sensitive:
            argv.append("--ignore-case")
        if args.glob:
            argv += ["--glob", args.glob]
        argv += ["--max-count", str(_RG_MAX_COUNT), args.pattern, rel_base]

        try:
            proc = await asyncio.create_subprocess_exec(
                *argv,
                cwd=str(ctx.workspace),
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
            )
        except OSError:
            return None
        async def collect() -> tuple[list[str], bool]:
            assert proc.stdout is not None
            lines: list[str] = []
            while line := await proc.stdout.readline():
                if len(lines) >= effective_max:
                    proc.kill()
                    await proc.wait()
                    return lines, True
                lines.append(line.decode("utf-8", errors="replace").rstrip("\r\n"))
            await proc.wait()
            return lines, False

        try:
            lines, capped = await asyncio.wait_for(collect(), timeout=_RG_TIMEOUT_S)
        except asyncio.TimeoutError:
            try:
                proc.kill()
            except ProcessLookupError:  # pragma: no cover - race on exit
                pass
            await proc.wait()
            raise TimeoutError("ripgrep timed out")
        except asyncio.CancelledError:
            try:
                proc.kill()
            except ProcessLookupError:
                pass
            await proc.wait()
            raise

        if proc.returncode == 1 and not capped:  # ripgrep: no matches
            return []
        if proc.returncode != 0 and not capped:
            return None  # unusable run (bad pattern for rg, etc.) -> fallback
        prefix = "" if rel_base == "." else rel_base + "/"
        normalized: list[str] = []
        for line in lines:
            match = _RG_LINE_RE.match(line)
            if match is None:
                normalized.append(line)
                continue
            path = match.group("path").replace("\\", "/")
            if path.startswith("./"):
                path = path[2:]
            if prefix and path.startswith(prefix):
                path = path[len(prefix) :]
            normalized.append(f"{path}:{match.group('lineno')}: {match.group('text')}")
        if capped:
            normalized.append("...[达到结果上限；请缩小 path 或 glob 继续搜索]")
        return normalized

    # ------------------------------------------------------------------
    # Pure-Python strategy: deterministic, no external dependencies.
    # ------------------------------------------------------------------

    def _python_search(
        self,
        regex: re.Pattern[str],
        base: Path,
        glob: str | None,
        max_results: int,
        ctx: ToolContext,
    ) -> list[str]:
        matches: list[str] = []
        skipped_large = 0
        deadline = time.monotonic() + _RG_TIMEOUT_S
        for dirpath, dirnames, filenames in os.walk(base):
            if time.monotonic() >= deadline:
                raise TimeoutError("python search timed out")
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for filename in sorted(filenames):
                if time.monotonic() >= deadline:
                    raise TimeoutError("python search timed out")
                if glob is not None and not fnmatch.fnmatch(filename, glob):
                    continue
                fpath = Path(dirpath) / filename
                try:
                    if fpath.stat().st_size > ctx.limits.search_max_file_bytes:
                        skipped_large += 1
                        continue
                except OSError:
                    continue
                text = self._read_text(fpath, ctx)
                if text is None:
                    continue
                try:
                    rel = fpath.relative_to(base).as_posix()
                except ValueError:  # pragma: no cover - walk stays below base
                    rel = fpath.as_posix()
                for lineno, line in enumerate(text.splitlines(), start=1):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("python search timed out")
                    if regex.search(line) is None:
                        continue
                    matches.append(f"{rel}:{lineno}: {line[:_MAX_LINE_CHARS]}")
                    if len(matches) >= max_results:
                        matches.append("...[达到结果上限；请缩小 path 或 glob 继续搜索]")
                        return matches
        if skipped_large:
            matches.append(
                f"...[跳过 {skipped_large} 个超出搜索页预算的文件；可用 read 分页查看]"
            )
        return matches

    @staticmethod
    def _read_text(fpath: Path, ctx: ToolContext) -> str | None:
        """Read *fpath* for searching; None when skipped (binary/too large/unreadable)."""
        try:
            if fpath.stat().st_size > ctx.limits.search_max_file_bytes:
                return None
            with open(fpath, "rb") as fh:
                head = fh.read(_BINARY_SNIFF_BYTES)
                if b"\x00" in head:
                    return None
                body = head + fh.read()
        except OSError:
            return None
        return body.decode("utf-8", errors="replace")
