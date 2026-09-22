"""Offline execution report rendering for minicode sessions.

Re-exports the public report surface: :func:`render_session_html` turns a
stored session (summary + events + messages, plus optional full tool
outputs) into a single-file, dependency-free HTML page that can be opened
offline in any browser.
"""

from __future__ import annotations

from minicode.reports.html import render_session_html

__all__ = ["render_session_html"]
