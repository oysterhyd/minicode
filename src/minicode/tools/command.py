"""Shell command execution tool with timeouts and tree-safe killing.

The platform-shell helpers (``shell_command`` / ``spawn_shell`` /
``kill_process_tree`` / ``decode_shell_output``) live at module level because
they are also reused by ``minicode.tasks.background`` and
``minicode.goals.checker``.
"""

from __future__ import annotations

import asyncio
import contextlib
import locale
import os
import re
import shutil
import signal
import subprocess
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, resolve_or_fail, truncate_output

#: Force the Windows shells to speak UTF-8 on the pipe. Without this,
#: PowerShell writes its output in the console code page (cp936 on a Chinese
#: Windows) and every non-ASCII character in a command's output — paths,
#: document excerpts, test names — reaches the model as mojibake.
_PS_UTF8_PREAMBLE = (
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;"
    "$OutputEncoding=[System.Text.Encoding]::UTF8;"
)

#: ``2>/dev/null`` is a POSIX idiom PowerShell reads as "redirect to the file
#: /dev/null" (i.e. a stray ``<drive>:\\dev\\null``), which fails or silently
#: creates files. The POSIX null device maps onto the PowerShell one.
_WIN_NULL_REDIRECT_RE = re.compile(r"(?P<redir>[12]?>>|[12]?>)\s*/dev/null\b")

#: SGR colour escapes; PowerShell colourises formatted tables even when its
#: output is a pipe. Pure noise (and token cost) for the model.
_ANSI_SGR_RE = re.compile(r"\x1b\[[0-9;]*m")


def _translate_posix_redirects(command: str) -> str:
    """Rewrite ``>/dev/null`` / ``2>/dev/null`` for PowerShell."""
    return _WIN_NULL_REDIRECT_RE.sub(lambda m: f"{m.group('redir')}$null", command)


def shell_command(command: str) -> list[str]:
    """Wrap *command* for the platform shell, passed as a fixed argv."""
    if os.name == "nt":
        shell = "pwsh" if shutil.which("pwsh") else "powershell"
        command = _translate_posix_redirects(command)
        return [
            shell,
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            _PS_UTF8_PREAMBLE + command,
        ]
    return ["bash", "-c", command]


def decode_shell_output(raw: bytes) -> str:
    """Decode a shell's stdout, normalising line endings and stripping SGR.

    The harness asks its shells to emit UTF-8 (see :data:`_PS_UTF8_PREAMBLE`),
    but output from tools the shell itself invokes can still arrive in the
    system code page, so a clean decode is attempted in both encodings before
    falling back to replacement characters. Guessing wrong here is cheap
    (garbled text) while refusing to guess is not (unreadable output).
    """
    candidates = ["utf-8"]
    preferred = locale.getpreferredencoding(False)
    if preferred and preferred.lower().replace("-", "").replace("_", "") != "utf8":
        candidates.append(preferred)
    text: str | None = None
    for encoding in candidates:
        try:
            text = raw.decode(encoding)
        except (UnicodeDecodeError, LookupError):
            continue
        break
    if text is None:
        text = raw.decode("utf-8", errors="replace")
    return _ANSI_SGR_RE.sub("", text).replace("\r\n", "\n")


async def spawn_shell(command: str, cwd: str | Path) -> asyncio.subprocess.Process:
    """Spawn *command* (wrapped for the platform shell) in *cwd*, with stdout
    and stderr merged into one pipe. Off Windows the child becomes a session
    leader so :func:`kill_process_tree` can take down its whole tree.

    Raises OSError/ValueError when the process cannot be started.
    """
    kwargs: dict[str, Any] = {
        "cwd": str(cwd),
        "stdout": asyncio.subprocess.PIPE,
        "stderr": asyncio.subprocess.STDOUT,
    }
    if os.name != "nt":
        kwargs["start_new_session"] = True
    # Cancellation can arrive while the OS has created the child but asyncio
    # has not handed us its handle. Finish spawning, then reap that child.
    spawning = asyncio.create_task(
        asyncio.create_subprocess_exec(*shell_command(command), **kwargs)
    )
    try:
        return await asyncio.shield(spawning)
    except asyncio.CancelledError:
        while not spawning.done():
            try:
                await asyncio.shield(spawning)
            except asyncio.CancelledError:
                continue
        proc = spawning.result()
        cleanup = asyncio.create_task(kill_process_tree(proc))
        while not cleanup.done():
            try:
                await asyncio.shield(cleanup)
            except asyncio.CancelledError:
                continue
        cleanup.result()
        raise


async def kill_process_tree(proc: asyncio.subprocess.Process) -> None:
    """Finish killing and reaping even if cancellation is requested again."""
    cleanup = asyncio.create_task(_kill_process_tree(proc))
    cancelled = False
    while not cleanup.done():
        try:
            await asyncio.shield(cleanup)
        except asyncio.CancelledError:
            cancelled = True
    cleanup.result()
    if cancelled:
        raise asyncio.CancelledError


async def _kill_process_tree(proc: asyncio.subprocess.Process) -> None:
    """Platform-specific tree termination, shielded by kill_process_tree."""
    if os.name == "nt":
        try:
            result = await asyncio.to_thread(subprocess.run,
                ["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                capture_output=True,
                timeout=15,
            )
            if result.returncode != 0:
                with contextlib.suppress(ProcessLookupError):
                    proc.kill()
        except (OSError, subprocess.SubprocessError):
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
    else:
        try:
            # start_new_session makes the child a session leader, so its
            # process group id equals its pid.
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
    with contextlib.suppress(Exception):
        await proc.wait()


class BashArgs(BaseModel):
    command: str
    timeout_s: float | None = None
    cwd: str = "."
    background: bool = False


class BashTool(BaseTool):
    name = "bash"
    description = (
        "Run a shell command inside the workspace (PowerShell on Windows, "
        "bash elsewhere) and return its combined output and exit code. "
        "Set background=true to start the command as a background job "
        "instead: it returns a job_id immediately and the result is "
        "delivered by the system once the job completes. Requires approval."
    )
    requires_approval = True
    args_model = BashArgs

    async def execute(self, args: BashArgs, ctx: ToolContext) -> ToolOutcome:
        cwd, failure = resolve_or_fail(ctx, args.cwd)
        if failure is not None:
            return failure
        if not cwd.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.cwd}")

        timeout_s = (
            ctx.limits.default_command_timeout_s
            if args.timeout_s is None
            else args.timeout_s
        )
        timeout_s = max(1.0, min(timeout_s, ctx.limits.max_command_timeout_s))

        if args.background:
            manager = ctx.background_manager
            if manager is None:
                return ToolOutcome.failure(
                    "background execution requested but no background manager is configured"
                )
            job_id = await manager.start(args.command, cwd, timeout_s)
            return ToolOutcome(
                success=True,
                job_id=job_id,
                output=(
                    f"已在后台启动命令。\njob_id: {job_id}\n命令: {args.command}\n"
                    "结果将在后台完成后由系统通知。"
                ),
            )

        try:
            proc = await spawn_shell(args.command, cwd)
        except (OSError, ValueError) as exc:
            return ToolOutcome.failure(f"failed to start command: {exc}")

        try:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=timeout_s)
        except asyncio.TimeoutError:
            await kill_process_tree(proc)
            return ToolOutcome.failure(f"command timed out after {timeout_s}s and was killed")
        except asyncio.CancelledError:
            await kill_process_tree(proc)
            raise

        output = decode_shell_output(stdout)
        output = truncate_output(output, ctx.limits.max_command_output_chars)
        if proc.returncode == 0:
            return ToolOutcome(success=True, output=output, exit_code=0)
        return ToolOutcome(
            success=False,
            output=output,
            error=f"command exited with code {proc.returncode}",
            exit_code=proc.returncode,
        )
