"""Tests for minicode.tools.files (read, ls, edit, write)."""

from __future__ import annotations

import asyncio
import os
import sys

import pytest

from minicode.tools.base import ToolContext, ToolLimits
from minicode.tools.files import EditTool, LsTool, ReadTool, WriteTool


def make_ctx(tmp_path, **limit_overrides) -> ToolContext:
    limits = ToolLimits(**limit_overrides) if limit_overrides else ToolLimits()
    return ToolContext(workspace=tmp_path, limits=limits)


def run(tool, raw_args, ctx):
    return asyncio.run(tool.run(raw_args, ctx))


# ---------------------------------------------------------------------------
# read
# ---------------------------------------------------------------------------


def test_read_happy_path_line_numbers(tmp_path):
    (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "a.txt"}, ctx)
    assert outcome.success is True
    assert outcome.error is None
    lines = outcome.output.splitlines()
    assert lines == ["1\talpha", "2\tbeta"]


def test_read_offset_and_limit(tmp_path):
    (tmp_path / "a.txt").write_text("l1\nl2\nl3\nl4\nl5\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "a.txt", "offset": 2, "limit": 2}, ctx)
    assert outcome.success is True
    assert outcome.output.splitlines()[:2] == ["2\tl2", "3\tl3"]
    assert "next_offset=4" in outcome.output


def test_read_offset_beyond_eof(tmp_path):
    (tmp_path / "a.txt").write_text("l1\nl2\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "a.txt", "offset": 10}, ctx)
    assert outcome.success is False
    assert "beyond end of file" in (outcome.error or "")
    assert "10" in (outcome.error or "")


def test_read_missing_file(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "nope.txt"}, ctx)
    assert outcome.success is False
    assert "not a file" in (outcome.error or "")


def test_read_too_large(tmp_path):
    (tmp_path / "big.txt").write_text("x" * 200, encoding="utf-8")
    ctx = make_ctx(tmp_path, max_read_bytes=50)
    outcome = run(ReadTool(), {"path": "big.txt"}, ctx)
    assert outcome.success is True
    assert "next_cursor=" in outcome.output
    assert "xxxxx" in outcome.output


def test_read_large_file_pages_by_cursor_without_loading_whole_file(tmp_path):
    (tmp_path / "big.txt").write_text("a" * 300_000 + "\nend\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    first = run(ReadTool(), {"path": "big.txt"}, ctx)
    assert first.success and "next_cursor=" in first.output
    marker = first.output.rsplit("next_cursor=", 1)[1].split("]", 1)[0]
    cursor = int(marker)
    second = run(ReadTool(), {"path": "big.txt", "offset": 1, "cursor": cursor}, ctx)
    assert second.success
    assert "a" * 100 in second.output
    assert cursor > 0


def test_read_defaults_to_two_thousand_lines_and_utf8_page(tmp_path):
    (tmp_path / "lines.txt").write_text("x\n" * 2100, encoding="utf-8")
    lines = run(ReadTool(), {"path": "lines.txt"}, make_ctx(tmp_path))
    assert "next_offset=2001" in lines.output
    assert len([line for line in lines.output.splitlines() if "\tx" in line]) == 2000

    (tmp_path / "wide.txt").write_text("汉" * 30_000, encoding="utf-8")
    first = run(ReadTool(), {"path": "wide.txt"}, make_ctx(tmp_path))
    assert len(first.output.encode("utf-8")) <= 50 * 1024
    cursor = int(first.output.rsplit("next_cursor=", 1)[1].split("]", 1)[0])
    second = run(ReadTool(), {"path": "wide.txt", "cursor": cursor}, make_ctx(tmp_path))
    assert first.success and second.success
    assert "�" not in first.output + second.output
    assert cursor > 0


def test_read_non_utf8(tmp_path):
    (tmp_path / "bin.dat").write_bytes(b"\x80\x81\x82")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "bin.dat"}, ctx)
    assert outcome.success is False
    assert "binary or non-UTF-8" in (outcome.error or "")


def test_read_traversal_outside_workspace(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "../secret"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_read_symlink_escape(tmp_path):
    outside = tmp_path.parent / "outside_secret_target.txt"
    outside.write_text("secret", encoding="utf-8")
    link = tmp_path / "link.txt"
    try:
        os.symlink(outside, link)
    except OSError:
        pytest.skip("symlink creation not permitted on this platform/account")
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"path": "link.txt"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


def test_read_junction_escape(tmp_path, monkeypatch):
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
    outcome = run(ReadTool(), {"path": "jlink/secret.txt"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")
    from minicode.core.paths import is_link_or_junction
    from minicode.goals.checker import workspace_fingerprint
    from minicode.tools.search import GrepTool
    import shutil

    assert is_link_or_junction(link)
    assert "jlink" not in run(LsTool(), {"path": ".", "recursive": True}, ctx).output
    monkeypatch.setattr(shutil, "which", lambda name: None)
    assert "secret" not in run(GrepTool(), {"pattern": "secret"}, ctx).output
    fingerprint = workspace_fingerprint(tmp_path)
    (outside / "secret.txt").write_text("changed-secret", encoding="utf-8")
    assert workspace_fingerprint(tmp_path) == fingerprint


def test_read_invalid_arguments(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(ReadTool(), {"offset": 1}, ctx)  # missing required path
    assert outcome.success is False
    assert "invalid arguments for read" in (outcome.error or "")


def test_read_spec_has_no_top_level_title(tmp_path):
    spec = ReadTool().spec()
    assert spec.name == "read"
    assert spec.requires_approval is False
    assert "title" not in spec.input_schema
    assert "path" in spec.input_schema["properties"]


# ---------------------------------------------------------------------------
# ls
# ---------------------------------------------------------------------------


def test_ls_tree_sorted_and_skip_dirs(tmp_path):
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
    outcome = run(LsTool(), {"recursive": True}, ctx)
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


def test_ls_not_a_directory(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(LsTool(), {"path": "missing_dir"}, ctx)
    assert outcome.success is False
    assert "not a directory" in (outcome.error or "")


def test_ls_truncates_entry_count(tmp_path):
    for i in range(510):
        (tmp_path / f"f{i:03d}.txt").write_text("x", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(LsTool(), {}, ctx)
    assert outcome.success is True
    entries = outcome.output.splitlines()
    assert len(entries) == 501  # 500 shown + next-page marker
    assert "next_offset=500" in entries[-1]
    next_page = run(LsTool(), {"offset": 500}, ctx)
    assert next_page.success
    assert len(next_page.output.splitlines()) == 10


def test_ls_defaults_to_current_directory(tmp_path):
    (tmp_path / "nested").mkdir()
    (tmp_path / "nested" / "inside.txt").write_text("x", encoding="utf-8")
    direct = run(LsTool(), {}, make_ctx(tmp_path))
    recursive = run(LsTool(), {"recursive": True}, make_ctx(tmp_path))
    assert direct.output == "nested/"
    assert "nested/inside.txt" in recursive.output


def test_large_recursive_ls_compacts_paths_without_losing_entries(tmp_path):
    relative = "repository-materials-with-a-long-name/course-assets"
    folder = tmp_path / relative
    folder.mkdir(parents=True)
    for index in range(510):
        (folder / f"chapter-{index:04d}.txt").write_text("x", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    first = run(LsTool(), {"recursive": True}, ctx)
    assert first.success
    assert "目录树（缩进表示路径层级）" in first.output
    assert "next_offset=500" in first.output

    reconstructed: set[str] = set()
    parents: list[str] = []
    for line in first.output.splitlines()[1:]:
        if line.startswith("..."):
            break
        depth = (len(line) - len(line.lstrip(" "))) // 2
        name = line.strip()
        if name.endswith("/"):
            parents = parents[:depth] + [name[:-1]]
        else:
            reconstructed.add("/".join(parents[:depth] + [name]))
    assert len(reconstructed) == 498
    assert f"{relative}/chapter-0000.txt" in reconstructed

    second = run(LsTool(), {"recursive": True, "offset": 500}, ctx)
    assert second.success
    assert len([line for line in second.output.splitlines() if line.endswith(".txt")]) == 12
    flat_size = sum(len(f"{relative}/chapter-{index:04d}.txt\n") for index in range(498))
    assert len(first.output) < flat_size * 0.7


# ---------------------------------------------------------------------------
# edit
# ---------------------------------------------------------------------------


def test_edit_unique_old_text(tmp_path):
    (tmp_path / "code.py").write_text("one\ntwo\nthree\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "code.py", "old_text": "two", "new_text": "TWO"}, ctx)
    assert outcome.success is True
    assert (tmp_path / "code.py").read_text(encoding="utf-8") == "one\nTWO\nthree\n"
    lines = outcome.output.splitlines()
    assert lines[0].startswith("Applied change to code.py (+1 -1)")
    assert "-two" in lines
    assert "+TWO" in lines
    assert "--- a/code.py" in lines
    assert "+++ b/code.py" in lines


def test_edit_old_text_not_found(tmp_path):
    (tmp_path / "a.txt").write_text("alpha\nbeta\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "a.txt", "old_text": "zzz", "new_text": "y"}, ctx)
    assert outcome.success is False
    assert "old_text not found in a.txt" in (outcome.error or "")
    assert "re-read the file" in (outcome.error or "")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "alpha\nbeta\n"


def test_edit_ambiguous_old_text(tmp_path):
    (tmp_path / "a.txt").write_text("dup\ndup\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "a.txt", "old_text": "dup", "new_text": "x"}, ctx)
    assert outcome.success is False
    assert "matches 2 locations" in (outcome.error or "")
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "dup\ndup\n"


def test_edit_replace_all(tmp_path):
    (tmp_path / "a.txt").write_text("dup\ndup\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(
        EditTool(),
        {"path": "a.txt", "old_text": "dup", "new_text": "x", "replace_all": True},
        ctx,
    )
    assert outcome.success is True
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "x\nx\n"


def test_edit_large_file_streams_without_whole_file_cap(tmp_path):
    target = tmp_path / "large.txt"
    target.write_text("prefix\n" + "x" * 300_000 + "\nneedle\n", encoding="utf-8")
    ctx = make_ctx(tmp_path, max_read_bytes=256)
    outcome = run(EditTool(), {
        "path": "large.txt", "old_text": "needle", "new_text": "fixed",
    }, ctx)
    assert outcome.success
    assert "large file" in outcome.output
    assert target.read_text(encoding="utf-8").endswith("\nfixed\n")


def test_edit_large_file_keeps_ambiguity_guard(tmp_path):
    target = tmp_path / "large.txt"
    target.write_text("needle\n" + "x" * 300_000 + "\nneedle\n", encoding="utf-8")
    outcome = run(EditTool(), {
        "path": "large.txt", "old_text": "needle", "new_text": "fixed",
    }, make_ctx(tmp_path, max_read_bytes=256))
    assert not outcome.success
    assert "matches 2 locations" in (outcome.error or "")
    assert target.read_text(encoding="utf-8").count("needle") == 2


def test_edit_on_missing_file_hints_write(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "missing.txt", "old_text": "a", "new_text": "b"}, ctx)
    assert outcome.success is False
    assert "not a file" in (outcome.error or "")
    assert "write" in (outcome.error or "")


def test_edit_requires_old_text(tmp_path):
    (tmp_path / "a.txt").write_text("content\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "a.txt", "old_text": "", "new_text": "x"}, ctx)
    assert outcome.success is False
    assert "invalid arguments for edit" in (outcome.error or "")


def test_edit_traversal_outside_workspace(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(EditTool(), {"path": "../evil.txt", "old_text": "a", "new_text": "b"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")


# ---------------------------------------------------------------------------
# write
# ---------------------------------------------------------------------------


def test_write_creates_new_file_with_parents(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(
        WriteTool(),
        {"path": "sub/dir/new.txt", "content": "hello\nworld\n"},
        ctx,
    )
    assert outcome.success is True
    created = tmp_path / "sub" / "dir" / "new.txt"
    assert created.read_text(encoding="utf-8") == "hello\nworld\n"
    assert "+++ b/sub/dir/new.txt" in outcome.output
    assert "+hello" in outcome.output.splitlines()


def test_write_overwrites_existing_file(tmp_path):
    (tmp_path / "a.txt").write_text("old\n", encoding="utf-8")
    ctx = make_ctx(tmp_path)
    outcome = run(WriteTool(), {"path": "a.txt", "content": "new\n"}, ctx)
    assert outcome.success is True
    assert (tmp_path / "a.txt").read_text(encoding="utf-8") == "new\n"
    lines = outcome.output.splitlines()
    assert lines[0].startswith("Applied change to a.txt")
    assert "-old" in lines
    assert "+new" in lines


def test_write_overwrites_large_file_atomically(tmp_path):
    target = tmp_path / "large.txt"
    target.write_text("x" * 300_000, encoding="utf-8")
    outcome = run(WriteTool(), {"path": "large.txt", "content": "new\n"},
                  make_ctx(tmp_path, max_read_bytes=256))
    assert outcome.success
    assert target.read_text(encoding="utf-8") == "new\n"
    assert "Large diff omitted" in outcome.output


def test_write_refuses_directory_target(tmp_path):
    (tmp_path / "sub").mkdir()
    ctx = make_ctx(tmp_path)
    outcome = run(WriteTool(), {"path": "sub", "content": "x"}, ctx)
    assert outcome.success is False
    assert "not a file" in (outcome.error or "")


def test_write_traversal_outside_workspace(tmp_path):
    ctx = make_ctx(tmp_path)
    outcome = run(WriteTool(), {"path": "../evil.txt", "content": "boom"}, ctx)
    assert outcome.success is False
    assert "outside workspace" in (outcome.error or "")
