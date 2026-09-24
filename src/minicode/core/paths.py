"""Workspace path safety: every filesystem tool resolves paths through here."""

from __future__ import annotations

import os
import stat
from pathlib import Path


class PathOutsideWorkspaceError(Exception):
    """Raised when a requested path escapes the workspace boundary."""


def is_link_or_junction(path: Path) -> bool:
    """Detect symlinks and Windows reparse points before recursive traversal."""
    try:
        if path.is_symlink():
            return True
        if os.name == "nt":
            attrs = getattr(path.stat(follow_symlinks=False), "st_file_attributes", 0)
            return bool(attrs & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400))
        return False
    except OSError:
        return True  # fail closed if an entry changed during traversal


def resolve_in_workspace(workspace: Path, user_path: str) -> Path:
    """Resolve *user_path* (relative to the workspace, absolute allowed) and
    guarantee it stays inside *workspace*, following symlinks/junctions.

    Raises PathOutsideWorkspaceError otherwise.
    """
    if not user_path or not user_path.strip():
        raise PathOutsideWorkspaceError("empty path")

    root = workspace.resolve()
    candidate = Path(user_path)
    if candidate.is_absolute():
        resolved = candidate.resolve()
    else:
        resolved = (root / candidate).resolve()

    # normcase makes the comparison case-insensitive on Windows and
    # normalizes drive-letter case.
    root_nc = os.path.normcase(str(root))
    resolved_nc = os.path.normcase(str(resolved))
    if resolved_nc != root_nc and not resolved_nc.startswith(root_nc + os.sep):
        raise PathOutsideWorkspaceError(
            f"path {user_path!r} resolves to {resolved}, outside workspace {root}"
        )
    return resolved
