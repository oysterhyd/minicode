"""Host-side tests that are never copied into an agent evaluation workspace."""

from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path

from minicode.tools.command import capture_bounded, decode_shell_output

HIDDEN_DIR = Path(__file__).resolve().parent / "hidden_tests"
HIDDEN_TIMEOUT_S = 120.0


async def check_hidden(task_id: str, workspace: Path) -> tuple[bool, str]:
    """Run one task's held-out assertions in a separate bounded process."""
    test_path = HIDDEN_DIR / task_id / "test_hidden.py"
    if not test_path.is_file():
        return False, f"missing host-side test: {test_path}"
    env = os.environ.copy()
    env["PYTHONPATH"] = str(Path(workspace).resolve())
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    kwargs = {"cwd": str(workspace), "env": env,
              "stdout": asyncio.subprocess.PIPE, "stderr": asyncio.subprocess.STDOUT}
    if os.name != "nt":
        kwargs["start_new_session"] = True
    try:
        process = await asyncio.create_subprocess_exec(
            sys.executable, "-m", "pytest", str(test_path), "-q", "-p", "no:cacheprovider",
            **kwargs,
        )
        output, timed_out, overflow = await capture_bounded(
            process, HIDDEN_TIMEOUT_S, max_bytes=1_000_000
        )
    except OSError as exc:
        return False, f"host-side test could not start: {exc}"
    detail = decode_shell_output(output)[-2000:]
    if timed_out:
        return False, f"host-side test timed out after {HIDDEN_TIMEOUT_S:g}s\n{detail}"
    if overflow:
        return False, f"host-side test output exceeded 1 MB\n{detail}"
    return process.returncode == 0, detail
