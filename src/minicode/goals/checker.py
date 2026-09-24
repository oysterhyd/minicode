"""Goal checker: run acceptance items against a workspace and produce reports.

Command items reuse the shared platform-shell helpers from
:mod:`minicode.tools.command` (shell wrapping, merged stdout/stderr,
tree-safe kill on timeout) so the semantics match the ``bash`` tool
exactly; the import direction goals -> tools keeps the dependency graph
acyclic.
"""

from __future__ import annotations

import asyncio
import hashlib
import os
import time
from pathlib import Path

from pydantic import BaseModel

from minicode.core.clock import utc_now
from minicode.core.paths import PathOutsideWorkspaceError, is_link_or_junction, resolve_in_workspace
from minicode.goals.spec import AcceptanceItem, AcceptanceSpec, ItemKind
from minicode.tools.command import capture_bounded, decode_shell_output, kill_process_tree, spawn_shell
from minicode.tools.files import SKIP_DIRS

_FINGERPRINT_SKIP_DIRS = SKIP_DIRS - {".minicode"}

COMMAND_OUTPUT_LIMIT = 5000
GOAL_COMMAND_TIMEOUT_S = 120.0


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def workspace_fingerprint(workspace: Path) -> str:
    """Content fingerprint of *workspace*.

    Every file (under transient directories other than ``.minicode``) contributes
    ``sha256(relative_posix_path + "\\0" + full_content_hash + str(size))``;
    the sorted list of per-file digests is hashed once more. Empty (or absent)
    workspaces hash to ``sha256("")``. Pure and synchronous.
    """
    root = Path(workspace).resolve()
    digests: list[str] = []
    if root.is_dir():
        for dirpath, dirnames, filenames in os.walk(root):
            ordinary_dirs = []
            for dirname in sorted(dirnames):
                if dirname in _FINGERPRINT_SKIP_DIRS:
                    continue
                child = Path(dirpath) / dirname
                if is_link_or_junction(child):
                    rel = child.relative_to(root).as_posix()
                    try:
                        link_target = os.readlink(child)
                    except OSError as exc:
                        link_target = f"unreadable:{exc.errno}"
                    digests.append(hashlib.sha256(
                        f"{rel}\0symlink:{link_target}".encode("utf-8")
                    ).hexdigest())
                else:
                    ordinary_dirs.append(dirname)
            dirnames[:] = ordinary_dirs
            for name in sorted(filenames):
                file_path = Path(dirpath) / name
                rel = file_path.relative_to(root).as_posix()
                size = -1
                try:
                    if is_link_or_junction(file_path):
                        content_digest = f"symlink:{os.readlink(file_path)}".encode("utf-8")
                    else:
                        size = file_path.stat().st_size
                        content_digest = _sha256_file(file_path).encode("ascii")
                except OSError as exc:
                    # Distinguish unreadable/missing files from empty files;
                    # never reuse the preceding file's size after a failed stat.
                    content_digest = f"unreadable:{exc.errno}".encode("ascii")
                entry = hashlib.sha256()
                entry.update(rel.encode("utf-8"))
                entry.update(b"\0")
                entry.update(content_digest)
                entry.update(str(size).encode("utf-8"))
                digests.append(entry.hexdigest())
    return hashlib.sha256("\n".join(sorted(digests)).encode("utf-8")).hexdigest()


def _truncate_output(text: str, limit: int = COMMAND_OUTPUT_LIMIT) -> str:
    if len(text) <= limit:
        return text
    return text[:limit] + f"\n…(输出已截断，原始 {len(text)} 字符)"


class ProtectedSnapshot:
    """Records the content of protected paths so later tampering is detectable.

    ``capture`` records the sha256 of each path's current content (paths are
    resolved through :func:`minicode.core.paths.resolve_in_workspace`; a path
    that does not exist is recorded as ``None``). ``violations`` then reports
    every path whose content changed, that disappeared, or that *appeared*
    although it did not exist at capture time.
    """

    def __init__(self, workspace: Path, paths: list[str]):
        self.workspace = Path(workspace)
        self.paths = list(paths)
        self._digests: dict[str, str | None] = {}
        self._unresolvable: dict[str, str] = {}
        self._captured = False

    def capture(self) -> None:
        self._digests = {}
        self._unresolvable = {}
        for raw in self.paths:
            try:
                resolved = resolve_in_workspace(self.workspace, raw)
            except PathOutsideWorkspaceError as exc:
                self._digests[raw] = None
                self._unresolvable[raw] = f"受保护路径越界，无法追踪: {exc}"
                continue
            try:
                self._digests[raw] = _sha256_file(resolved) if resolved.is_file() else None
            except OSError as exc:
                self._digests[raw] = None
                self._unresolvable[raw] = f"受保护文件无法读取，无法建立基线: {exc}"
        self._captured = True

    def export(self) -> dict:
        return {"digests": dict(self._digests), "unresolvable": dict(self._unresolvable)}

    def restore(self, state: dict) -> None:
        self._digests = dict(state["digests"])
        self._unresolvable = dict(state["unresolvable"])
        # Missing baseline data must fail closed, never capture modified files.
        for raw in self.paths:
            if raw not in self._digests:
                self._unresolvable[raw] = "缺少会话初始基线，无法验证受保护文件"
        self._captured = True

    def violations(self) -> list[tuple[str, str]]:
        """(path, reason) pairs for every protected path that changed."""
        if not self._captured:
            self.capture()  # no snapshot yet: treat the current state as baseline
        out: list[tuple[str, str]] = []
        for raw in self.paths:
            if raw in self._unresolvable:
                out.append((raw, self._unresolvable[raw]))
                continue
            before = self._digests.get(raw)
            try:
                resolved = resolve_in_workspace(self.workspace, raw)
            except PathOutsideWorkspaceError as exc:
                out.append((raw, f"受保护路径越界，无法追踪: {exc}"))
                continue
            if not resolved.is_file():
                if before is not None:
                    out.append((raw, "受保护文件在快照时存在，验收时已消失"))
                continue
            if before is None:
                out.append((raw, "受保护文件在快照时不存在，验收时新出现"))
            else:
                try:
                    changed = _sha256_file(resolved) != before
                except OSError as exc:
                    out.append((raw, f"受保护文件无法读取: {exc}"))
                    continue
                if changed:
                    out.append((raw, "受保护文件内容在快照后被修改"))
        return out


class GoalItemResult(BaseModel):
    """Outcome of one acceptance item."""

    item_id: str
    kind: ItemKind
    passed: bool
    detail: str
    exit_code: int | None = None
    ran_at: str


class GoalReport(BaseModel):
    """Outcome of one full acceptance run."""

    passed: bool
    items: list[GoalItemResult]
    fingerprint: str
    ran_at: str


class GoalChecker:
    """Runs the acceptance items of a spec against a workspace."""

    # Class attribute (not an __init__ parameter) so the public constructor
    # signature stays as specified while tests can shorten the timeout.
    command_timeout_s: float = GOAL_COMMAND_TIMEOUT_S

    def __init__(
        self,
        spec: AcceptanceSpec,
        workspace: Path,
        protected_snapshot: ProtectedSnapshot | None = None,
    ):
        self.spec = spec
        self.workspace = Path(workspace)
        self.protected_snapshot = protected_snapshot

    async def run(self, *, deadline: float | None = None) -> GoalReport:
        """Run commands, then verify protected files and fingerprint the final state."""
        ran_at = utc_now()
        results: list[GoalItemResult] = []
        # Capture before commands, and check protection after all commands.
        if self.protected_snapshot is not None and not self.protected_snapshot._captured:
            self.protected_snapshot.capture()
        for item in self.spec.items:
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError("turn deadline exceeded")
            if item.type == "command":
                results.append(await self._check_command(item))
            elif item.type == "artifact":
                results.append(self._check_artifact(item))
        snapshot = self.protected_snapshot
        violations = dict(snapshot.violations()) if snapshot is not None else {}
        results.extend(
            self._check_protected(item, violations)
            for item in self.spec.items if item.type == "protected"
        )
        by_id = {result.item_id: result for result in results}
        results = [by_id[item.id] for item in self.spec.items]
        return GoalReport(
            passed=all(r.passed for r in results),
            items=results,
            fingerprint=await asyncio.to_thread(workspace_fingerprint, self.workspace),
            ran_at=ran_at,
        )

    # -- individual checks ---------------------------------------------------

    async def _check_command(self, item: AcceptanceItem) -> GoalItemResult:
        command = item.command or ""
        try:
            proc = await spawn_shell(command, self.workspace)
        except (OSError, ValueError) as exc:
            return self._result(item, passed=False, detail=f"命令无法启动: {exc}")

        try:
            stdout, timed_out, over_limit, pipe_lingered = await capture_bounded(
                proc, self.command_timeout_s
            )
        except asyncio.CancelledError:
            await kill_process_tree(proc)
            raise

        if timed_out:
            return self._result(
                item,
                passed=False,
                detail=f"命令超时（>{self.command_timeout_s:g}s），已强制终止进程树",
            )
        if over_limit:
            return self._result(item, passed=False, detail="验收命令输出超过采集预算，已终止进程树")
        if pipe_lingered:
            return self._result(item, passed=False, detail="命令已退出，但子进程占用输出管道，输出采集已停止")

        output = decode_shell_output(stdout)
        output = _truncate_output(output)
        code = proc.returncode
        suffix = f"；输出: {output}" if output else "；无输出"
        return self._result(
            item, passed=code == 0, detail=f"退出码 {code}{suffix}", exit_code=code
        )

    def _check_artifact(self, item: AcceptanceItem) -> GoalItemResult:
        raw = item.path or ""
        try:
            resolved = resolve_in_workspace(self.workspace, raw)
        except PathOutsideWorkspaceError as exc:
            return self._result(item, passed=False, detail=f"产物路径越界，拒绝检查: {exc}")
        if not resolved.exists():
            return self._result(item, passed=False, detail=f"产物文件不存在: {raw}")
        if not resolved.is_file():
            return self._result(item, passed=False, detail=f"产物路径存在但不是文件: {raw}")
        return self._result(item, passed=True, detail=f"产物文件存在: {raw}")

    def _check_protected(self, item: AcceptanceItem, violations: dict[str, str]) -> GoalItemResult:
        if self.protected_snapshot is None:
            return self._result(item, passed=False, detail="no protected snapshot captured")
        raw = item.path or ""
        reason = violations.get(raw)
        if reason is not None:
            return self._result(item, passed=False, detail=reason)
        return self._result(item, passed=True, detail="受保护文件与快照一致，未被修改")

    # -- helpers --------------------------------------------------------------

    @staticmethod
    def _result(
        item: AcceptanceItem, *, passed: bool, detail: str, exit_code: int | None = None
    ) -> GoalItemResult:
        return GoalItemResult(
            item_id=item.id,
            kind=item.type,
            passed=passed,
            detail=detail,
            exit_code=exit_code,
            ran_at=utc_now(),
        )

    # -- reporting -------------------------------------------------------------

    @staticmethod
    def format_failure_report(report: GoalReport) -> str:
        """Render *report* as a structured Chinese message for the model."""
        if report.passed:
            return (
                "验收通过。"
                f"共 {len(report.items)} 项检查全部通过；工作区指纹 {report.fingerprint}。"
            )
        lines = ["验收未通过，以下检查项未通过："]
        for result in report.items:
            if result.passed:
                continue
            detail = " ".join(str(result.detail).split())  # keep one line per item
            line = f"- [{result.item_id}] 类型={result.kind} 详情：{detail}"
            if result.exit_code is not None:
                line += f"（退出码 {result.exit_code}）"
            lines.append(line)
        lines.append(
            "验收未通过。请修复上述问题后重新提交；不得修改受保护路径；"
            "不要声称完成——只有验收命令退出码为 0 才算通过。"
        )
        return "\n".join(lines)
