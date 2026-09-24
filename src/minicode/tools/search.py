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
from minicode.core.paths import is_link_or_junction
from minicode.tools.base import BaseTool, ToolContext, resolve_or_fail
from minicode.tools.files import SKIP_DIRS

_RG_TIMEOUT_S = 30.0
_MAX_LINE_CHARS = 500
_BINARY_SNIFF_BYTES = 8192

# rg output line: <path>:<line>:<text> (relative paths contain no colons).
_RG_LINE_RE = re.compile(r"^(?P<path>[^:]+):(?P<lineno>\d+):(?P<text>.*)$")


class GrepArgs(BaseModel):
    pattern: str
    path: str = "."
    glob: str | None = None
    max_results: int | None = None
    case_sensitive: bool = True


class GrepTool(BaseTool):
    name = "grep"
    description = (
        "Search file contents in the workspace with a regular expression and "
        "return 'path:line: text' match lines (100 by default, 50 KiB maximum). "
        "Path may be a file or directory; case-sensitive by default. "
        "Set case_sensitive=false for case-insensitive search."
    )
    requires_approval = False
    args_model = GrepArgs

    async def execute(self, args: GrepArgs, ctx: ToolContext) -> ToolOutcome:
        base, failure = resolve_or_fail(ctx, args.path)
        if failure is not None:
            return failure
        if not base.is_dir() and not base.is_file():
            return ToolOutcome.failure(f"path is not a file or directory: {args.path}")

        effective_max = (
            ctx.limits.max_search_results if args.max_results is None else args.max_results
        )
        effective_max = max(1, effective_max)

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
        output: list[str] = []
        remaining = ctx.limits.max_output_chars - 120
        for match in matches:
            size = len(match.encode("utf-8")) + (1 if output else 0)
            if size > remaining:
                output.append("...[达到 50 KiB 输出上限；请缩小 path 或 glob]")
                break
            output.append(match)
            remaining -= size
        preview = "\n".join(output)
        full = "\n".join(matches)
        return ToolOutcome(output=preview, full_output=full if preview != full else None)

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
            "rg", "--no-ignore", "--hidden", "--no-heading", "--line-number", "--color", "never",
            "--max-columns", str(_MAX_LINE_CHARS), "--max-columns-preview",
        ]
        # The workspace can itself live below an ignored parent (for example
        # pytest's .pytest_cache). Host ignore files must not hide its files.
        for skipped in sorted(SKIP_DIRS):
            argv += ["--glob", f"!**/{skipped}/**"]
        if not args.case_sensitive:
            argv.append("--ignore-case")
        if args.glob:
            argv += ["--glob", args.glob]
        argv += ["--", args.pattern, rel_base]

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
            content = match.group("text")
            clipped = content[:_MAX_LINE_CHARS]
            suffix = "...[行已截断]" if len(content) > _MAX_LINE_CHARS or "[Omitted end" in content else ""
            normalized.append(f"{path}:{match.group('lineno')}: {clipped}{suffix}")
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
        roots = os.walk(base) if base.is_dir() else [(str(base.parent), [], [base.name])]
        for dirpath, dirnames, filenames in roots:
            if time.monotonic() >= deadline:
                raise TimeoutError("python search timed out")
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS
                                 and not is_link_or_junction(Path(dirpath) / d))
            for filename in sorted(filenames):
                if time.monotonic() >= deadline:
                    raise TimeoutError("python search timed out")
                if glob is not None and not fnmatch.fnmatch(filename, glob):
                    continue
                fpath = Path(dirpath) / filename
                # A fallback search must not dereference a workspace link to
                # a file outside the workspace. ripgrep skips links by default.
                if is_link_or_junction(fpath):
                    continue
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
                    rel = fpath.relative_to(base).as_posix() if base.is_dir() else base.name
                except ValueError:  # pragma: no cover - walk stays below base
                    rel = fpath.as_posix()
                for lineno, line in enumerate(text.splitlines(), start=1):
                    if time.monotonic() >= deadline:
                        raise TimeoutError("python search timed out")
                    if regex.search(line) is None:
                        continue
                    clipped = line[:_MAX_LINE_CHARS]
                    suffix = "...[行已截断]" if len(line) > _MAX_LINE_CHARS else ""
                    matches.append(f"{rel}:{lineno}: {clipped}{suffix}")
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
