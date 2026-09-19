"""Tests for minicode.tools.files (read_file, list_files, apply_patch)."""

from __future__ import annotations

import asyncio
import os
import sys

import pytest

from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.files import ApplyPatchTool, ListFilesTool, ReadFileTool


def make_ctx(tmp_path, **limit_overrides) -> ToolContext:
    limits = ToolLimits(**limit_overrides) if limit_overrides else ToolLimits()
    return ToolContext(workspace=tmp_path, limits=limits)


def run(tool, raw_args, ctx):
    return asyncio.run(tool.run(raw_args, ctx))


# ---------------------------------------------------------------------------
# read_file
# ---------------------------------------------------------------------------


def test_read_file_happy_path_line_numbers(tmp_path):
    (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "a.txt"}, ctx)
    assert outcome.success is True
    assert outcome.error is None
    lines = outcome.output.splitlines()
    assert lines == ["     1\talpha", "     2\tbeta"]


def test_read_file_offset_and_limit(tmp_path):
    (tmp_path / "a.txt").write_text("l1\nl2\nl3\nl4\nl5\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "a.txt", "offset": 2, "limit": 2}, ctx)
    assert outcome.success is True
    assert outcome.output.splitlines() == ["     2\tl2", "     3\tl3"]


def test_read_file_offset_beyond_eof(tmp_path):
    (tmp_path / "a.txt").write_text("l1\nl2\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "a.txt", "offset": 10}, ctx)
    assert outcome.success is False
    assert "beyond end of file" in (outcome.error or "")
    assert "10" in (outcome.error or "")


def test_read_file_missing_file(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "nope.txt"}, ctx)
    assert outcome.success is False
    assert "not a file" in (outcome.error or "")


def test_read_file_too_large(tmp_path):
    (tmp_path / "big.txt").write_text("x" * 200, encoding="utf-8")
    ctx = make_ctx(tmp_path, max_read_bytes=50)
    outcome = run(ReadFileTool(), {"path": "big.txt"}, ctx)
    assert outcome.success is False
    assert "file too large" in (outcome.error or "")
    assert "200" in (outcome.error or "")


def test_read_file_non_utf8(tmp_path):
    (tmp_path / "bin.dat").write_bytes(b"\x80\x81\x82")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "bin.dat"}, ctx)
    assert outcome.success is False
    assert "binary or non-UTF-8" in (outcome.error or "")


def test_read_file_traversal_outside_workspace(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "../secret"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_read_file_symlink_escape(tmp_path):
    outside = tmp_path.parent / "outside_secret_target.txt"
    outside.write_text("secret", encoding="utf-8")
    link = tmp_path / "link.txt"
    try:
        os.symlink(outside, link)
    except OSError:
        pytest.skip("symlink creation not permitted on this platform/account")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "link.txt"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_read_file_junction_escape(tmp_path):
    # Junctions need no special privileges on Windows, so this exercises the
    # reparse-point escape that the symlink test usually has to skip.
    if sys.platform != "win32":
        pytest.skip("directory junctions are a Windows reparse point")
    import _winapi  # Windows-only stdlib module

    outside = tmp_path.parent / "junction_escape_target"
    outside.mkdir(exist_ok=True)
    (outside / "secret.txt").write_text("secret", encoding="utf-8")
    link = tmp_path / "jlink"
    try:
        _winapi.CreateJunction(str(outside), str(link))
    except OSError:
        pytest.skip("junction creation not permitted")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"path": "jlink/secret.txt"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_read_file_invalid_arguments(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadFileTool(), {"offset": 1}, ctx)  # missing required path
    assert outcome.success is False
    assert "invalid arguments for read_file" in (outcome.error or "")


def test_read_file_spec_has_no_top_level_title(tmp_path):
    spec = ReadFileTool().spec()
    assert spec.name == "read_file"
    assert spec.requires_approval is False
    assert "title" not in spec.input_schema
    assert "path" in spec.input_schema["properties"]


# ---------------------------------------------------------------------------
# list_files
# ---------------------------------------------------------------------------


def test_list_files_tree_sorted_and_skip_dirs(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.py").write_text("x", encoding="utf-8")
    (tmp_path / "src" / "sub").mkdir()
    (tmp_path / "src" / "sub" / "util.py").write_text("x", encoding="utf-8")
    (tmp_path / "README.md").write_text("x", encoding="utf-8")
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git" / "config").write_text("x", encoding="utf-8")
    (tmp_path / "__pycache__").mkdir()
    (tmp_path / "__pycache__" / "m.pyc").write_text("x", encoding="utf-8")
    (tmp_path / ".venv").mkdir()
    (tmp_path / ".venv" / "pyvenv.cfg").write_text("x", encoding="utf-8")

    ctx = make_ctx(tmp_path)
    outcome = run(ListFilesTool(), {}, ctx)
    assert outcome.success is True
    entries = outcome.output.splitlines()
    assert entries == sorted(entries)
    assert "README.md" in entries
    assert "src/" in entries
    assert "src/main.py" in entries
    assert "src/sub/" in entries
    assert "src/sub/util.py" in entries
    skipped = [e for e in entries if ".git" in e or "__pycache__" in e or ".venv" in e]
    assert skipped == []


def test_list_files_not_a_directory(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ListFilesTool(), {"path": "missing_dir"}, ctx)
    assert outcome.success is False
    assert "not a directory" in (outcome.error or "")


def test_list_files_truncates_entry_count(tmp_path):
    for i in range(510):
        (tmp_path / f"f{i:03d}.txt").write_text("x", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ListFilesTool(), {}, ctx)
    assert outcome.success is True
    entries = outcome.output.splitlines()
    assert len(entries) == 501  # 500 shown + summary line
    assert entries[-1] == "...[10 more entries]"


# ---------------------------------------------------------------------------
# apply_patch
# ---------------------------------------------------------------------------


def test_apply_patch_create_new_file_with_parents(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(
        ApplyPatchTool(),
        {"path": "sub/dir/new.txt", "new_text": "hello\nworld\n"},
        ctx,
    )
    assert outcome.success is True
    created = tmp_path / "sub" / "dir" / "new.txt"
    assert created.read_text(encoding="utf-8") == "hello\nworld\n"
    assert "+++ b/sub/dir/new.txt" in outcome.output
    assert "+hello" in outcome.output.splitlines()
    assert outcome.output.splitlines()[0] == "--- a/sub/dir/new.txt"


def test_apply_patch_edit_unique_old_text(tmp_path):
    (tmp_path / "code.py").write_text("one\ntwo\nthree\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "code.py", "old_text": "two", "new_text": "TWO"}, ctx)
    assert outcome.success is True
    assert (tmp_path / "code.py").read_text(encoding="utf-8") == "one\nTWO\nthree\n"
    lines = outcome.output.splitlines()
    assert lines[0].startswith("Applied patch to code.py (+1 -1)")
    assert "-two" in lines
    assert "+TWO" in lines
    assert "--- a/code.py" in lines
    assert "+++ b/code.py" in lines


def test_apply_patch_old_text_not_found(tmp_path):
    (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "a.txt", "old_text": "zzz", "new_text": "y"}, ctx)
    assert outcome.success is False
    assert "old_text not found in a.txt" in (outcome.error or "")
    assert "re-read the file" in (outcome.error or "")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "alpha\nbeta\n"


def test_apply_patch_ambiguous_old_text(tmp_path):
    (tmp_path / "a.txt").write_text("dup\ndup\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "a.txt", "old_text": "dup", "new_text": "x"}, ctx)
    assert outcome.success is False
    assert "matches 2 locations" in (outcome.error or "")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "dup\ndup\n"


def test_apply_patch_create_on_existing_file_fails(tmp_path):
    (tmp_path / "a.txt").write_text("existing\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "a.txt", "new_text": "boom\n"}, ctx)
    assert outcome.success is False
    assert "file already exists" in (outcome.error or "")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "existing\n"


def test_apply_patch_empty_old_text_is_creation_mode(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "new.txt", "old_text": "", "new_text": "hi\n"}, ctx)
    assert outcome.success is True
    assert (tmp_path / "new.txt").read_text(encoding="utf-8") == "hi\n"


def test_apply_patch_traversal_outside_workspace(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ApplyPatchTool(), {"path": "../evil.txt", "new_text": "boom"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")
