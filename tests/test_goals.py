"""Tests for minicode.goals: acceptance spec, fingerprint, protected snapshot,
goal checker, and evidence ledger."""

from __future__ import annotations

import asyncio
import hashlib
import os
import sys
from datetime import datetime
from pathlib import Path

import pytest

from minicode.goals import (
    AcceptanceSpec,
    EvidenceLedger,
    GoalChecker,
    ProtectedSnapshot,
    workspace_fingerprint,
)

EMPTY_SHA = hashlib.sha256(b"").hexdigest()


def _py_cmd(snippet: str) -> str:
    """A command that runs *snippet* with the current interpreter on any shell.

    Same approach as tests/test_tools_command.py: forward-slash interpreter
    path (accepted by both PowerShell and bash), quoted only when needed.
    """
    exe = sys.executable.replace("\\", "/")
    if os.name == "nt":
        if " " in exe:
            return f'& "{exe}" -c "{snippet}"'
        return f'{exe} -c "{snippet}"'
    return f'"{exe}" -c "{snippet}"'


def _load_spec(tmp_path: Path, text: str) -> AcceptanceSpec:
    path = tmp_path / "goal.yaml"
    path.write_text(text, encoding="utf-8")
    return AcceptanceSpec.from_yaml(path)


def _ws(tmp_path: Path) -> Path:
    ws = tmp_path / "ws"
    ws.mkdir()
    return ws


def _run(checker: GoalChecker):
    return asyncio.run(checker.run())


def _spec_dict(items: list[dict]) -> dict:
    return {"items": items}


# ---------------------------------------------------------------------------
# spec.from_yaml
# ---------------------------------------------------------------------------

FULL_YAML = """\
id: pagination-boundary
title: 修复分页越界
max_fix_attempts: 5
items:
  - id: tests-pass
    type: command
    command: python -m pytest test_paginate.py -q
  - id: report-exists
    type: artifact
    path: report.txt
  - id: tests-unchanged
    type: protected
    path: test_paginate.py
"""


def test_from_yaml_full(tmp_path):
    spec = _load_spec(tmp_path, FULL_YAML)
    assert spec.id == "pagination-boundary"
    assert spec.title == "修复分页越界"
    assert spec.max_fix_attempts == 5
    assert [i.id for i in spec.items] == ["tests-pass", "report-exists", "tests-unchanged"]
    assert [i.type for i in spec.items] == ["command", "artifact", "protected"]
    assert spec.items[0].command == "python -m pytest test_paginate.py -q"
    assert spec.items[1].path == "report.txt"
    assert spec.items[2].path == "test_paginate.py"


def test_from_yaml_defaults(tmp_path):
    spec = _load_spec(
        tmp_path, "items:\n  - id: a\n    type: command\n    command: echo hi\n"
    )
    assert spec.id == "goal"
    assert spec.title == ""
    assert spec.max_fix_attempts == 3
    assert len(spec.items) == 1


def test_from_yaml_missing_file(tmp_path):
    with pytest.raises(ValueError, match="不存在"):
        AcceptanceSpec.from_yaml(tmp_path / "nope.yaml")


def test_from_yaml_invalid_yaml_syntax(tmp_path):
    with pytest.raises(ValueError, match="解析失败"):
        _load_spec(tmp_path, "items: [\n")


def test_from_yaml_root_sequence(tmp_path):
    with pytest.raises(ValueError, match="根节点"):
        _load_spec(tmp_path, "- a\n- b\n")


def test_from_yaml_root_scalar(tmp_path):
    with pytest.raises(ValueError, match="根节点"):
        _load_spec(tmp_path, "just-a-string\n")


def test_from_yaml_unknown_top_level_field(tmp_path):
    # "acceptance" is a tempting-but-wrong key name; the schema uses "items".
    text = "acceptance: []\nitems:\n  - id: a\n    type: command\n    command: echo hi\n"
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, text)
    assert "未知字段" in str(excinfo.value)
    assert "acceptance" in str(excinfo.value)


def test_from_yaml_unknown_item_field(tmp_path):
    text = "items:\n  - id: a\n    type: command\n    command: echo hi\n    typo: 1\n"
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, text)
    message = str(excinfo.value)
    assert "未知字段" in message
    assert "typo" in message


def test_from_yaml_error_includes_path_and_line(tmp_path):
    path = tmp_path / "goal.yaml"
    path.write_text(
        "items:\n  - id: a\n    type: bogus\n    path: x\n", encoding="utf-8"
    )
    with pytest.raises(ValueError) as excinfo:
        AcceptanceSpec.from_yaml(path)
    message = str(excinfo.value)
    assert "goal.yaml" in message
    assert "第 3 行" in message  # the offending `type:` line
    assert "type" in message


def test_from_yaml_command_item_without_command(tmp_path):
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, "items:\n  - id: a\n    type: command\n")
    assert "command" in str(excinfo.value)


def test_from_yaml_artifact_item_without_path(tmp_path):
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, "items:\n  - id: a\n    type: artifact\n")
    assert "path" in str(excinfo.value)


def test_from_yaml_protected_item_without_path(tmp_path):
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, "items:\n  - id: a\n    type: protected\n")
    assert "path" in str(excinfo.value)


def test_from_yaml_bad_type_value(tmp_path):
    with pytest.raises(ValueError) as excinfo:
        _load_spec(tmp_path, "items:\n  - id: a\n    type: bogus\n    path: x\n")
    assert "type" in str(excinfo.value)


def test_from_yaml_empty_items(tmp_path):
    with pytest.raises(ValueError, match="items"):
        _load_spec(tmp_path, "items: []\n")


def test_from_yaml_missing_items(tmp_path):
    with pytest.raises(ValueError, match="items"):
        _load_spec(tmp_path, "id: only-top-level\n")


def test_from_yaml_duplicate_item_ids(tmp_path):
    text = (
        "items:\n"
        "  - id: a\n    type: command\n    command: echo hi\n"
        "  - id: a\n    type: command\n    command: echo ho\n"
    )
    with pytest.raises(ValueError, match="重复"):
        _load_spec(tmp_path, text)


# ---------------------------------------------------------------------------
# workspace_fingerprint
# ---------------------------------------------------------------------------


def test_fingerprint_empty_dir(tmp_path):
    assert workspace_fingerprint(tmp_path) == EMPTY_SHA


def test_fingerprint_stable_when_untouched(tmp_path):
    (tmp_path / "a.txt").write_text("hello", encoding="utf-8")
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "b.txt").write_text("world", encoding="utf-8")
    fp1 = workspace_fingerprint(tmp_path)
    fp2 = workspace_fingerprint(tmp_path)
    assert fp1 == fp2
    assert fp1 != EMPTY_SHA


def test_fingerprint_changes_on_content_change(tmp_path):
    target = tmp_path / "a.txt"
    target.write_text("v1", encoding="utf-8")
    fp1 = workspace_fingerprint(tmp_path)
    target.write_text("v2", encoding="utf-8")
    fp2 = workspace_fingerprint(tmp_path)
    assert fp1 != fp2
    # Restoring the content restores the fingerprint (it is a content hash).
    target.write_text("v1", encoding="utf-8")
    assert workspace_fingerprint(tmp_path) == fp1


def test_fingerprint_changes_on_add_and_delete(tmp_path):
    fp0 = workspace_fingerprint(tmp_path)
    (tmp_path / "new.txt").write_text("x", encoding="utf-8")
    fp1 = workspace_fingerprint(tmp_path)
    assert fp1 != fp0
    (tmp_path / "new.txt").unlink()
    assert workspace_fingerprint(tmp_path) == fp0


def test_fingerprint_ignores_skip_dirs(tmp_path):
    (tmp_path / "a.txt").write_text("code", encoding="utf-8")
    fp_before = workspace_fingerprint(tmp_path)
    for skip in (
        ".git",
        "__pycache__",
        ".venv",
        "venv",
        "node_modules",
        ".pytest_cache",
        ".ruff_cache",
    ):
        directory = tmp_path / skip
        directory.mkdir()
        (directory / "junk.bin").write_bytes(b"\x00\x01" * 128)
    assert workspace_fingerprint(tmp_path) == fp_before

    config = tmp_path / ".minicode" / "plugins"
    config.mkdir(parents=True)
    (config / "plugin.json").write_text("{}", encoding="utf-8")
    assert workspace_fingerprint(tmp_path) != fp_before


def test_fingerprint_does_not_read_external_symlink(tmp_path):
    workspace = tmp_path / "repo"
    workspace.mkdir()
    outside = tmp_path / "secret.txt"
    outside.write_text("first", encoding="utf-8")
    try:
        (workspace / "link.txt").symlink_to(outside)
    except (OSError, NotImplementedError):
        pytest.skip("symlink creation unavailable")
    before = workspace_fingerprint(workspace)
    outside.write_text("changed", encoding="utf-8")
    assert workspace_fingerprint(workspace) == before


def test_fingerprint_includes_content_after_first_2mb(tmp_path):
    # A tail change must invalidate previously passing acceptance evidence.
    head = b"a" * (2 * 1024 * 1024)
    target = tmp_path / "big.bin"
    target.write_bytes(head + b"tail1")
    fp1 = workspace_fingerprint(tmp_path)
    target.write_bytes(head + b"tail2")
    fp2 = workspace_fingerprint(tmp_path)
    assert fp1 != fp2
    # But a change within the first 2 MiB does change the fingerprint.
    target.write_bytes(b"b" + head[1:] + b"tail2")
    assert workspace_fingerprint(tmp_path) != fp2


# ---------------------------------------------------------------------------
# ProtectedSnapshot
# ---------------------------------------------------------------------------


def _captured(ws: Path, paths: list[str]) -> ProtectedSnapshot:
    snapshot = ProtectedSnapshot(ws, paths)
    snapshot.capture()
    return snapshot


def test_snapshot_clean_when_untouched(tmp_path):
    ws = _ws(tmp_path)
    (ws / "a.py").write_text("a=1", encoding="utf-8")
    (ws / "b.py").write_text("b=2", encoding="utf-8")
    snapshot = _captured(ws, ["a.py", "b.py"])
    assert snapshot.violations() == []


def test_snapshot_detects_modification(tmp_path):
    ws = _ws(tmp_path)
    (ws / "a.py").write_text("a=1", encoding="utf-8")
    snapshot = _captured(ws, ["a.py"])
    (ws / "a.py").write_text("a=2", encoding="utf-8")
    violations = snapshot.violations()
    assert len(violations) == 1
    path, reason = violations[0]
    assert path == "a.py"
    assert "修改" in reason


def test_snapshot_detects_deletion(tmp_path):
    ws = _ws(tmp_path)
    (ws / "b.py").write_text("b=2", encoding="utf-8")
    snapshot = _captured(ws, ["b.py"])
    (ws / "b.py").unlink()
    violations = snapshot.violations()
    assert len(violations) == 1
    assert violations[0][0] == "b.py"
    assert "消失" in violations[0][1]


def test_snapshot_detects_file_appearing_after_capture(tmp_path):
    ws = _ws(tmp_path)
    snapshot = _captured(ws, ["ghost.txt"])  # does not exist yet
    assert snapshot.violations() == []  # still missing → not a violation
    (ws / "ghost.txt").write_text("appeared", encoding="utf-8")
    violations = snapshot.violations()
    assert len(violations) == 1
    assert violations[0][0] == "ghost.txt"
    assert "出现" in violations[0][1]


def test_snapshot_multiple_violations(tmp_path):
    ws = _ws(tmp_path)
    (ws / "m.py").write_text("m", encoding="utf-8")
    (ws / "d.py").write_text("d", encoding="utf-8")
    snapshot = _captured(ws, ["m.py", "d.py", "ghost.txt"])
    (ws / "m.py").write_text("hacked", encoding="utf-8")
    (ws / "d.py").unlink()
    (ws / "ghost.txt").write_text("here", encoding="utf-8")
    violated = {path for path, _ in snapshot.violations()}
    assert violated == {"m.py", "d.py", "ghost.txt"}


def test_snapshot_reports_unresolvable_path(tmp_path):
    ws = _ws(tmp_path)
    (tmp_path / "s.txt").write_text("outside", encoding="utf-8")  # sibling of ws
    snapshot = _captured(ws, ["../s.txt"])
    violations = snapshot.violations()
    assert violations and violations[0][0] == "../s.txt"
    assert "越界" in violations[0][1]


# ---------------------------------------------------------------------------
# GoalChecker
# ---------------------------------------------------------------------------


def test_checker_command_pass(tmp_path):
    ws = _ws(tmp_path)
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "c", "type": "command", "command": "echo goal-ok"}])
    )
    report = _run(GoalChecker(spec, ws))
    assert report.passed is True
    item = report.items[0]
    assert item.item_id == "c"
    assert item.kind == "command"
    assert item.passed is True
    assert item.exit_code == 0
    assert "goal-ok" in item.detail


def test_checker_command_exit_code_propagates(tmp_path):
    ws = _ws(tmp_path)
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "c", "type": "command", "command": "exit 3"}])
    )
    report = _run(GoalChecker(spec, ws))
    item = report.items[0]
    assert report.passed is False
    assert item.passed is False
    assert item.exit_code == 3


def test_checker_command_runs_in_workspace(tmp_path):
    ws = _ws(tmp_path)
    (ws / "probe.txt").write_text("x", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {
                    "id": "c",
                    "type": "command",
                    "command": _py_cmd(
                        "import os; print(os.path.exists('probe.txt'))"
                    ),
                }
            ]
        )
    )
    report = _run(GoalChecker(spec, ws))
    assert report.passed is True
    assert "True" in report.items[0].detail


def test_checker_command_failure_includes_output(tmp_path):
    ws = _ws(tmp_path)
    spec = AcceptanceSpec.model_validate(
            _spec_dict(
                [{"id": "c", "type": "command", "command": _py_cmd("import sys; print('boom'); sys.exit(2)")}]
            )
    )
    report = _run(GoalChecker(spec, ws))
    item = report.items[0]
    assert item.passed is False
    assert "boom" in item.detail


def test_checker_command_timeout(monkeypatch, tmp_path):
    ws = _ws(tmp_path)
    monkeypatch.setattr(GoalChecker, "command_timeout_s", 0.5)
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [{"id": "c", "type": "command", "command": _py_cmd("import time; time.sleep(5)")}]
        )
    )
    report = _run(GoalChecker(spec, ws))
    item = report.items[0]
    assert item.passed is False
    assert item.exit_code is None
    assert "超时" in item.detail


def test_checker_command_output_truncated(tmp_path):
    ws = _ws(tmp_path)
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "c", "type": "command", "command": _py_cmd("print('x' * 20000)")}])
    )
    report = _run(GoalChecker(spec, ws))
    detail = report.items[0].detail
    assert len(detail) < 6000
    assert "截断" in detail


def test_checker_artifact_cases(tmp_path):
    ws = _ws(tmp_path)
    (ws / "report.txt").write_text("done", encoding="utf-8")
    (ws / "sub").mkdir()
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "ok", "type": "artifact", "path": "report.txt"},
                {"id": "missing", "type": "artifact", "path": "nope.txt"},
                {"id": "dir", "type": "artifact", "path": "sub"},
            ]
        )
    )
    report = _run(GoalChecker(spec, ws))
    by_id = {r.item_id: r for r in report.items}
    assert by_id["ok"].passed is True
    assert by_id["missing"].passed is False
    assert "不存在" in by_id["missing"].detail
    assert by_id["dir"].passed is False  # a directory is not an artifact file
    assert report.passed is False


def test_checker_artifact_relative_traversal_rejected(tmp_path):
    ws = _ws(tmp_path)
    (tmp_path / "secret.txt").write_text("outside", encoding="utf-8")  # sibling of ws
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "traversal", "type": "artifact", "path": "../secret.txt"}])
    )
    report = _run(GoalChecker(spec, ws))
    item = report.items[0]
    assert item.passed is False
    assert "越界" in item.detail


def test_checker_artifact_absolute_path_outside_rejected(tmp_path):
    ws = _ws(tmp_path)
    outside = tmp_path / "outside.txt"
    outside.write_text("x", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "abs", "type": "artifact", "path": str(outside)}])
    )
    report = _run(GoalChecker(spec, ws))
    assert report.items[0].passed is False


def test_checker_protected_without_snapshot(tmp_path):
    ws = _ws(tmp_path)
    (ws / "t.py").write_text("x", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "p", "type": "protected", "path": "t.py"}])
    )
    report = _run(GoalChecker(spec, ws))
    item = report.items[0]
    assert item.passed is False
    assert item.detail == "no protected snapshot captured"


def test_checker_protected_clean_snapshot(tmp_path):
    ws = _ws(tmp_path)
    (ws / "t.py").write_text("x", encoding="utf-8")
    snapshot = ProtectedSnapshot(ws, ["t.py"])
    snapshot.capture()
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "p", "type": "protected", "path": "t.py"}])
    )
    report = _run(GoalChecker(spec, ws, snapshot))
    assert report.passed is True
    assert report.items[0].passed is True


def test_checker_protected_detects_tampering(tmp_path):
    ws = _ws(tmp_path)
    (ws / "t.py").write_text("x", encoding="utf-8")
    (ws / "u.py").write_text("y", encoding="utf-8")
    snapshot = ProtectedSnapshot(ws, ["t.py", "u.py"])
    snapshot.capture()
    (ws / "t.py").write_text("hacked", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "p-t", "type": "protected", "path": "t.py"},
                {"id": "p-u", "type": "protected", "path": "u.py"},
            ]
        )
    )
    report = _run(GoalChecker(spec, ws, snapshot))
    by_id = {r.item_id: r for r in report.items}
    assert by_id["p-t"].passed is False
    assert "修改" in by_id["p-t"].detail
    assert by_id["p-u"].passed is True
    assert report.passed is False


def test_checker_report_shape(tmp_path):
    ws = _ws(tmp_path)
    (ws / "report.txt").write_text("r", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "a", "type": "command", "command": "echo ok"},
                {"id": "b", "type": "artifact", "path": "report.txt"},
            ]
        )
    )
    report = _run(GoalChecker(spec, ws))
    assert report.passed is True
    assert report.fingerprint == workspace_fingerprint(ws)
    # UTC ISO timestamps that parse back.
    datetime.fromisoformat(report.ran_at)
    datetime.fromisoformat(report.items[0].ran_at)
    assert report.ran_at.endswith("+00:00")
    # Items keep the spec order.
    assert [r.item_id for r in report.items] == ["a", "b"]


def test_checker_aggregates_any_failure(tmp_path):
    ws = _ws(tmp_path)
    (ws / "report.txt").write_text("r", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "good", "type": "artifact", "path": "report.txt"},
                {"id": "bad", "type": "command", "command": "exit 1"},
            ]
        )
    )
    report = _run(GoalChecker(spec, ws))
    assert report.passed is False
    assert [r.passed for r in report.items] == [True, False]


# ---------------------------------------------------------------------------
# GoalChecker.format_failure_report
# ---------------------------------------------------------------------------


def test_format_failure_report_lists_failures(tmp_path):
    ws = _ws(tmp_path)
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "c", "type": "command", "command": "exit 7"},
                {"id": "m", "type": "artifact", "path": "missing.txt"},
            ]
        )
    )
    report = _run(GoalChecker(spec, ws))
    text = GoalChecker.format_failure_report(report)
    assert "验收未通过" in text
    assert "[c]" in text
    assert "command" in text
    assert "退出码 7" in text
    assert "[m]" in text
    assert "artifact" in text
    # The mandatory closing instruction, verbatim at the end.
    assert text.rstrip().endswith("只有验收命令退出码为 0 才算通过。")
    assert "不得修改受保护路径" in text
    assert "不要声称完成" in text
    # One line per failed item: item details must not span raw newlines.
    body_lines = [l for l in text.splitlines() if l.startswith("- [")]
    assert len(body_lines) == 2


def test_format_failure_report_passing(tmp_path):
    ws = _ws(tmp_path)
    (ws / "report.txt").write_text("r", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict([{"id": "a", "type": "artifact", "path": "report.txt"}])
    )
    report = _run(GoalChecker(spec, ws))
    text = GoalChecker.format_failure_report(report)
    assert "验收通过" in text
    assert "才算通过" not in text
    assert report.fingerprint in text


# ---------------------------------------------------------------------------
# EvidenceLedger
# ---------------------------------------------------------------------------


def _passing_checker(ws: Path) -> GoalChecker:
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {
                    "id": "check",
                    "type": "command",
                    "command": _py_cmd("assert open('flag.txt').read() == '1'"),
                }
            ]
        )
    )
    return GoalChecker(spec, ws)


def test_evidence_ledger_empty_state(tmp_path):
    ledger = EvidenceLedger()
    assert ledger.latest() is None
    assert ledger.valid_pass("deadbeef") is False


def test_evidence_ledger_binds_evidence_to_fingerprint(tmp_path):
    ws = _ws(tmp_path)
    (ws / "flag.txt").write_text("1", encoding="utf-8")
    checker = _passing_checker(ws)
    ledger = EvidenceLedger()

    report = _run(checker)
    assert report.passed is True
    evidence = ledger.record(report)
    assert len(evidence) == 1
    entry = evidence[0]
    assert entry.item_id == "check"
    assert entry.kind == "command"
    assert entry.exit_code == 0
    assert entry.fingerprint == report.fingerprint
    assert ledger.valid_pass(report.fingerprint) is True
    latest = ledger.latest()
    assert latest is not None and latest[0].fingerprint == report.fingerprint

    # Code changed → new fingerprint → the recorded evidence is stale for it.
    (ws / "flag.txt").write_text("2", encoding="utf-8")
    new_fp = workspace_fingerprint(ws)
    assert new_fp != report.fingerprint
    assert ledger.valid_pass(new_fp) is False

    # Re-acceptance now fails → no new evidence, old evidence stays bound to
    # its own fingerprint only.
    report2 = _run(checker)
    assert report2.passed is False
    assert report2.fingerprint == new_fp
    assert ledger.record(report2) == []
    assert ledger.valid_pass(new_fp) is False
    assert ledger.valid_pass(report.fingerprint) is True  # history preserved
    assert ledger.latest() == []  # most recent record produced no evidence


def test_evidence_ledger_partial_failure_yields_no_evidence(tmp_path):
    ws = _ws(tmp_path)
    (ws / "flag.txt").write_text("1", encoding="utf-8")
    spec = AcceptanceSpec.model_validate(
        _spec_dict(
            [
                {"id": "good", "type": "artifact", "path": "flag.txt"},
                {"id": "bad", "type": "command", "command": "exit 1"},
            ]
        )
    )
    ledger = EvidenceLedger()
    report = _run(GoalChecker(spec, ws))
    assert report.passed is False
    assert ledger.record(report) == []
    assert ledger.valid_pass(report.fingerprint) is False
