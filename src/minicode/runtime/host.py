"""Interactive host boundaries that keep the agent's event loop alive."""

from __future__ import annotations

import asyncio
import signal
from collections.abc import Awaitable


async def interruptible(operation: Awaitable):
    """Ctrl+C cancels this activation while the surrounding REPL survives."""
    task = asyncio.create_task(operation)
    previous = signal.getsignal(signal.SIGINT)
    installed = False
    try:
        try:
            signal.signal(signal.SIGINT, lambda *_: task.cancel())
            installed = True
        except ValueError:  # embedded hosts may run outside the main thread
            pass
        return await task
    finally:
        if installed:
            signal.signal(signal.SIGINT, previous)


async def console_input(console, prompt: str) -> str:
    if not console.is_terminal:
        # CliRunner and batch input have no terminal descriptor or raw mode.
        return console.input(prompt)
    from prompt_toolkit import PromptSession
    from rich.text import Text

    return await PromptSession().prompt_async(Text.from_markup(prompt).plain)


async def console_confirm(console, prompt: str) -> bool:
    if not console.is_terminal:
        from rich.prompt import Confirm
        return Confirm.ask(prompt, default=False, console=console)
    while True:
        answer = (await console_input(console, prompt + " [y/N]: ")).strip().lower()
        if answer in {"", "n", "no"}:
            return False
        if answer in {"y", "yes"}:
            return True
