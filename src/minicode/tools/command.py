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
import tempfile
import time
from pathlib import Path
from typing import Any

from pydantic import BaseModel

from minicode.core.models import ToolOutcome
from minicode.tools.base import BaseTool, ToolContext, ToolExecution, resolve_or_fail, tail_output

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
_PIPE_DRAIN_GRACE_S = 1.0


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


def _close_stdout_pipe(proc: asyncio.subprocess.Process) -> None:
    """Release a pipe inherited by a descendant after the shell has exited."""
    if proc.stdout is not None:
        transport = getattr(proc.stdout, "_transport", None)
        if transport is not None:
            transport.close()


async def _wait_for_capture(
    proc: asyncio.subprocess.Process, collector: asyncio.Task[Any]
) -> tuple[Any, bool]:
    """Wait for the shell and a bounded tail drain, even with inherited pipes.

    On Windows ``Process.wait`` can wait for stdout's transport as well as the
    process. A descendant retaining the pipe then leaves it pending after the
    shell has exited. ``returncode`` is set at process exit independently.
    """
    if os.name == "nt":
        while proc.returncode is None:
            await asyncio.sleep(0.05)
    else:
        await proc.wait()
    try:
        return await asyncio.wait_for(asyncio.shield(collector), _PIPE_DRAIN_GRACE_S), False
    except asyncio.TimeoutError:
        _close_stdout_pipe(proc)
        try:
            return await asyncio.wait_for(collector, _PIPE_DRAIN_GRACE_S), True
        except asyncio.TimeoutError:
            return None, True


async def capture_bounded(
    proc: asyncio.subprocess.Process, timeout_s: float | None, max_bytes: int = 100_000_000
) -> tuple[bytes, bool, bool, bool]:
    """Return output and timeout, overflow, and inherited-pipe flags."""
    output = bytearray()

    async def collect() -> bool:
        assert proc.stdout is not None
        while chunk := await proc.stdout.read(64 * 1024):
            remaining = max_bytes - len(output)
            output.extend(chunk[:max(0, remaining)])
            if len(chunk) > remaining:
                await kill_process_tree(proc)
                return True
        return False

    collector = asyncio.create_task(collect())
    try:
        (over_limit, pipe_lingered) = await asyncio.wait_for(
            _wait_for_capture(proc, collector), timeout=timeout_s
        )
        return bytes(output), False, bool(over_limit), pipe_lingered
    except asyncio.TimeoutError:
        await kill_process_tree(proc)
        _close_stdout_pipe(proc)
        await asyncio.gather(collector, return_exceptions=True)
        return bytes(output), True, False, False
    except asyncio.CancelledError:
        await kill_process_tree(proc)
        collector.cancel()
        await asyncio.gather(collector, return_exceptions=True)
        raise
    except Exception:
        await kill_process_tree(proc)
        collector.cancel()
        await asyncio.gather(collector, return_exceptions=True)
        raise


async def spawn_shell(command: str, cwd: str | Path) -> asyncio.subprocess.Process:
    """Spawn *command* (wrapped for the platform shell) in *cwd*, with stdout
    and stderr merged into one pipe. Off Windows the child becomes a session
    leader so :func:`kill_process_tree` can take down its whole tree.

    Raises OSError/ValueError when the process cannot be started.
    """
    kwargs: dict[str, Any] = {
        "cwd": str(cwd),
        # Commands must never consume the host's terminal / NDJSON RPC input.
        # NonInteractive alone does not close stdin for PowerShell or its children.
        "stdin": asyncio.subprocess.DEVNULL,
        "stdout": asyncio.subprocess.PIPE,
        "stderr": asyncio.subprocess.STDOUT,
    }
    if os.name != "nt":
        kwargs["start_new_session"] = True
    else:
        kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW
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
        _close_stdout_pipe(proc)
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
    # The process runner owns timeout enforcement and process-tree teardown.
    execution = ToolExecution(timeout_s=None)
    name = "bash"
    description = (
        "Run a shell command inside the workspace (PowerShell on Windows, "
        "bash elsewhere) and return its combined output and exit code. "
        "Foreground commands are killed after timeout_s (default 300s, capped "
        "at 3600s) so a stuck shell cannot block the turn; set timeout_s to "
        "raise the bound. Set background=true to start the command as a "
        "background job instead: it returns a job_id immediately, runs without "
        "the default bound (timeout_s still applies when given) and its result "
        "is delivered by the system once the job completes. Output shows the last "
        "2,000 lines or 50 KiB, with complete output in an artifact. "
        "Requires approval."
    )
    requires_approval = True
    args_model = BashArgs

    async def execute(self, args: BashArgs, ctx: ToolContext) -> ToolOutcome:
        cwd, failure = resolve_or_fail(ctx, args.cwd)
        if failure is not None:
            return failure
        if not cwd.is_dir():
            return ToolOutcome.failure(f"path is not a directory: {args.cwd}")

        # Foreground commands get the context default so a wedged shell cannot
        # block the turn forever; an explicit timeout_s always wins, and both
        # are clamped to the configured maximum.
        timeout_s = (
            ctx.limits.default_command_timeout_s
            if args.timeout_s is None
            else args.timeout_s
        )
        if timeout_s is not None:
            timeout_s = max(1.0, timeout_s)
            if ctx.limits.max_command_timeout_s is not None:
                timeout_s = min(timeout_s, ctx.limits.max_command_timeout_s)

        if args.background:
            manager = ctx.background_manager
            if manager is None:
                return ToolOutcome.failure(
                    "background execution requested but no background manager is configured"
                )
            # A background job is the escape hatch for work that legitimately
            # runs long, so it stays unbounded unless the caller asked for a
            # bound; the turn is not waiting on it either way.
            job_timeout = None if args.timeout_s is None else timeout_s
            job_id = await manager.start(args.command, cwd, job_timeout)
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

        with tempfile.TemporaryFile(mode="w+b") as captured:
            live_tail = ""
            last_emit = 0.0
            async def collect() -> tuple[int, bool]:
                nonlocal live_tail, last_emit
                assert proc.stdout is not None
                total = 0
                while chunk := await proc.stdout.read(64 * 1024):
                    remaining = ctx.limits.max_command_capture_bytes - total
                    captured.write(chunk[:max(0, remaining)])
                    total += min(len(chunk), max(0, remaining))
                    if ctx.on_output is not None:
                        live_tail = (live_tail + decode_shell_output(chunk))[-2000:]
                        now = time.monotonic()
                        if now - last_emit >= 0.25:
                            await ctx.on_output(live_tail)
                            last_emit = now
                    if len(chunk) > remaining:
                        await kill_process_tree(proc)
                        return total, True
                return total, False

            collector = asyncio.create_task(collect())
            timed_out = False
            try:
                collected, pipe_lingered = await asyncio.wait_for(
                    _wait_for_capture(proc, collector), timeout=timeout_s
                )
                total, quota_hit = collected if collected is not None else (captured.tell(), False)
            except asyncio.TimeoutError:
                timed_out = True
                await kill_process_tree(proc)
                _close_stdout_pipe(proc)
                await asyncio.gather(collector, return_exceptions=True)
                total, quota_hit, pipe_lingered = captured.tell(), False, False
            except asyncio.CancelledError:
                await kill_process_tree(proc)
                collector.cancel()
                await asyncio.gather(collector, return_exceptions=True)
                raise
            except Exception:
                await kill_process_tree(proc)
                collector.cancel()
                await asyncio.gather(collector, return_exceptions=True)
                raise

            captured.seek(0)
            if total <= 2_000_000:
                output = decode_shell_output(captured.read())
                preview = tail_output(output, ctx.limits.max_command_output_chars - 200)
                if preview != output:
                    preview = "...[output truncated; 完整输出见归档]\n" + preview
                outcome = ToolOutcome(
                    output=preview, full_output=output if preview != output else None,
                    success=not timed_out and not quota_hit and proc.returncode == 0,
                    exit_code=proc.returncode,
                )
            else:
                captured.seek(max(0, total - 256_000))
                preview = tail_output(decode_shell_output(captured.read()),
                                      ctx.limits.max_command_output_chars - 200)
                spill = getattr(ctx.artifact_store, "spill_binary_stream", None)
                if callable(spill) and ctx.session_id is not None:
                    ref = await spill(ctx.session_id, "command_output", captured)
                    output = f"...[output truncated; 完整输出见 [artifact:{ref.artifact_id}]]\n{preview}"
                else:
                    output = f"...[前部已截断；归档不可用]\n{preview}"
                outcome = ToolOutcome(
                    success=not timed_out and not quota_hit and proc.returncode == 0,
                    output=output, exit_code=proc.returncode,
                )
            if timed_out:
                return outcome.model_copy(update={
                    "success": False,
                    "error": f"command timed out after {timeout_s}s and was killed",
                })
            if pipe_lingered:
                return outcome.model_copy(update={
                    "success": False,
                    "error": "shell exited but an inherited output pipe stayed open; output capture was stopped",
                })
            if quota_hit:
                return outcome.model_copy(update={
                    "success": False,
                    "error": "command output exceeded capture quota and was killed; narrow or redirect output",
                })
            if proc.returncode != 0:
                return outcome.model_copy(update={
                    "success": False,
                    "error": f"command exited with code {proc.returncode}",
                })
            return outcome
