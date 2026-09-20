"""Oyster Harness startup banner.

Rendered once at harness startup (``minicode run`` / ``chat`` / ``resume``
and on TUI mount): the ASCII wordmark, one compact info line with the
project version, interpreter, platform, model and workspace, then a blank
line so the log stream that follows starts clean.
"""

from __future__ import annotations

import platform
import sys
from importlib import metadata

from rich.console import Console
from rich.text import Text

#: 5-row block glyphs for the letters used by "OYSTER" and "HARNESS".
_GLYPHS: dict[str, tuple[str, str, str, str, str]] = {
    "O": (" ████ ", "█    █", "█    █", "█    █", " ████ "),
    "Y": ("█    █", " █  █ ", "  ██  ", "  ██  ", "  ██  "),
    "S": (" ████ ", "█     ", " ███  ", "    █ ", "████  "),
    "T": ("██████", "  ██  ", "  ██  ", "  ██  ", "  ██  "),
    "E": ("██████", "█     ", "████  ", "█     ", "██████"),
    "R": ("█████ ", "█    █", "█████ ", "█   █ ", "█    █"),
    "H": ("█    █", "█    █", "██████", "█    █", "█    █"),
    "A": (" ████ ", "█    █", "██████", "█    █", "█    █"),
    "N": ("█    █", "██   █", "█ █  █", "█  █ █", "█   ██"),
    "L": ("█     ", "█     ", "█     ", "█     ", "██████"),
}


def render_word(word: str) -> str:
    """Render *word* in the blocky banner font (uppercase A-Z supported)."""
    unknown = [ch for ch in word.upper() if ch not in _GLYPHS]
    if unknown:
        raise ValueError(f"banner font lacks glyphs for: {''.join(sorted(set(unknown)))}")
    rows = [" ".join(row) for row in zip(*(_GLYPHS[ch] for ch in word.upper()))]
    return "\n".join(rows)


def project_version() -> str:
    """Installed package version, falling back to the pyproject value."""
    try:
        return metadata.version("minicode")
    except metadata.PackageNotFoundError:  # running from a source checkout
        return "0.1.0"


def info_line(
    *,
    provider_label: str,
    model_label: str,
    workspace: str | None = None,
) -> str:
    """One compact environment line: version · python · platform · model."""
    python = f"Python {sys.version.split()[0]}"
    system = f"{platform.system()} {platform.release()}".strip()
    parts = [f"minicode v{project_version()}", python, system, f"{provider_label}/{model_label}"]
    if workspace:
        parts.append(workspace)
    return " · ".join(parts)


def banner_text(
    *,
    provider_label: str,
    model_label: str,
    workspace: str | None = None,
) -> Text:
    """The full banner (wordmark + info line) as a rich Text block."""
    text = Text(render_word("OYSTER") + "\n" + render_word("HARNESS"), style="bold cyan")
    text.append("\n")
    text.append(info_line(provider_label=provider_label, model_label=model_label, workspace=workspace), style="dim")
    return text


def print_banner(
    console: Console,
    *,
    provider_label: str,
    model_label: str,
    workspace: str | None = None,
) -> None:
    """Print the startup banner without disturbing the log stream after it."""
    console.print(banner_text(provider_label=provider_label, model_label=model_label, workspace=workspace))
    console.print()
