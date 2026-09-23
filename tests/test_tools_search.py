"""Tests for minicode.tools.search (search_text, fallback and ripgrep paths)."""

from __future__ import annotations

import asyncio
import shutil

import pytest

from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.search import GrepTool


def make_ctx(tmp_path, **limit_overrides) -> ToolContext:
    limits = ToolLimits(**limit_overrides) if limit_overrides else ToolLimits()
    return ToolContext(workspace=tmp_path, limits=limits)


def run(tool, raw_args, ctx):
    return asyncio.run(tool.run(raw_args, ctx))


def seed_tree(tmp_path) -> None:
    (tmp_path / "a.txt").write_text(
        "find the needle here\nnothing else\n", encoding="utf-8"
    )
    sub = tmp_path / "sub"
    sub.mkdir()
    (sub / "b.txt").write_text("another needle\n", encoding="utf-8")
    (tmp_path / "c.md").write_text("needle in markdown\n", encoding="utf-8")
    (tmp_path / "binary.bin").write_bytes(b"n\x00e\x00e\x00d\x00l\x00e\x00")


# ---------------------------------------------------------------------------
# Python fallback (rg unavailable)
# ---------------------------------------------------------------------------


def test_fallback_finds_needle_in_two_files(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    seed_tree(tmp_path)
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle"}, ctx)
    assert outcome.success is True
    assert "a.txt:1: find the needle here" in outcome.output
    assert "sub/b.txt:1: another needle" in outcome.output
    assert "c.md:1: needle in markdown" in outcome.output
    # binary file must be skipped
    assert "binary.bin" not in outcome.output


def test_fallback_glob_filter(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    seed_tree(tmp_path)
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle", "glob": "*.txt"}, ctx)
    assert outcome.success is True
    assert "a.txt:1" in outcome.output
    assert "sub/b.txt:1" in outcome.output
    assert "c.md" not in outcome.output


def test_fallback_case_sensitivity(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "case.txt").write_text("Needle here\nplain needle\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)

    insensitive = run(GrepTool(), {"pattern": "needle", "case_sensitive": False}, ctx)
    assert insensitive.success is True
    assert "case.txt:1: Needle here" in insensitive.output
    assert "case.txt:2" in insensitive.output

    sensitive = run(GrepTool(), {"pattern": "needle", "case_sensitive": True}, ctx)
    assert sensitive.success is True
    assert "case.txt:1" not in sensitive.output
    assert "case.txt:2: plain needle" in sensitive.output


def test_file_path_default_case_and_long_line(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "one.txt").write_text("Needle\nneedle " + "x" * 800, encoding="utf-8")
    outcome = run(GrepTool(), {"pattern": "needle", "path": "one.txt"}, make_ctx(tmp_path))
    assert "one.txt:1" not in outcome.output
    assert "one.txt:2" in outcome.output
    assert "行已截断" in outcome.output
    assert len(outcome.output.splitlines()[0]) < 550


def test_caller_can_raise_match_count(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "many.txt").write_text("hit\n" * 130, encoding="utf-8")
    outcome = run(GrepTool(), {"pattern": "hit", "max_results": 120}, make_ctx(tmp_path))
    assert len([line for line in outcome.output.splitlines() if line.startswith("many.txt:")]) == 120


def test_fallback_max_results_cap(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "many.txt").write_text(
        "needle 1\nneedle 2\nneedle 3\nneedle 4\nneedle 5\n", encoding="utf-8"
    )
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle", "max_results": 2}, ctx)
    assert outcome.success is True
    assert len(outcome.output.splitlines()) == 3
    assert "达到结果上限" in outcome.output


def test_fallback_max_results_clamped_to_one(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "many.txt").write_text("needle 1\nneedle 2\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle", "max_results": 0}, ctx)
    assert outcome.success is True
    assert len(outcome.output.splitlines()) == 2


def test_fallback_no_matches(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "a.txt").write_text("hello\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "zzzznotfound"}, ctx)
    assert outcome.success is True
    assert outcome.output == "(no matches)"


def test_fallback_invalid_regex(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    (tmp_path / "a.txt").write_text("hello\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "(unclosed"}, ctx)
    assert outcome.success is False
    assert "invalid regex" in (outcome.error or "")


def test_fallback_traversal_outside_workspace(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: None)
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle", "path": ".."}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


# ---------------------------------------------------------------------------
# ripgrep path (rg installed on this machine)
# ---------------------------------------------------------------------------


@pytest.mark.skipif(shutil.which("rg") is None, reason="ripgrep not installed")
def test_ripgrep_search_finds_results(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: "rg")
    seed_tree(tmp_path)
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "needle"}, ctx)
    assert outcome.success is True
    assert "needle" in outcome.output
    # rg paths are normalized to the same relpath:lineno: text format as the fallback
    assert "a.txt:1: find the needle here" in outcome.output
    assert "sub/b.txt:1: another needle" in outcome.output


@pytest.mark.skipif(shutil.which("rg") is None, reason="ripgrep not installed")
def test_ripgrep_search_no_matches(tmp_path, monkeypatch):
    monkeypatch.setattr(shutil, "which", lambda name: "rg")
    (tmp_path / "a.txt").write_text("hello\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(GrepTool(), {"pattern": "zzzznotfound"}, ctx)
    assert outcome.success is True
    assert outcome.output == "(no matches)"
