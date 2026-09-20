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
from pathlib import Path

from pydantic import BaseModel

from minicode.core.clock import utc_now
from minicode.core.paths import PathOutsideWorkspaceError, resolve_in_workspace
from minicode.goals.spec import AcceptanceItem, AcceptanceSpec, ItemKind
from minicode.tools.command import kill_process_tree, spawn_shell
from minicode.tools.files import SKIP_DIRS

_FINGERPRINT_READ_CAP = 2 * 1024 * 1024  # hash only the first 2 MiB per file
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

    Every file (under directories other than ``SKIP_DIRS``) contributes
    ``sha256(relative_posix_path + "\\0" + first_2MiB_of_content + str(size))``;
    the sorted list of per-file digests is hashed once more. Empty (or absent)
    workspaces hash to ``sha256("")``. Pure and synchronous.
    """
    root = Path(workspace).resolve()
    digests: list[str] = []
    if root.is_dir():
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
            for name in sorted(filenames):
                file_path = Path(dirpath) / name
                rel = file_path.relative_to(root).as_posix()
                try:
                    size = file_path.stat().st_size
                    with file_path.open("rb") as handle:
                        head = handle.read(_FINGERPRINT_READ_CAP)
                except OSError:
                    head = b""  # unreadable file still contributes its path
                entry = hashlib.sha256()
                entry.update(rel.encode("utf-8"))
                entry.update(b"\0")
                entry.update(head)
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
            self._digests[raw] = _sha256_file(resolved) if resolved.is_file() else None
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
            elif _sha256_file(resolved) != before:
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

    async def run(self) -> GoalReport:
        """Run every item and return a report. Async only for future-proofing."""
        ran_at = utc_now()
        fingerprint = workspace_fingerprint(self.workspace)
        results: list[GoalItemResult] = []
        # Lazily computed at the first protected item, i.e. after all preceding
        # command items ran, so tampering by acceptance commands is caught too.
        violations: dict[str, str] | None = None
        for item in self.spec.items:
            if item.type == "command":
                results.append(await self._check_command(item))
            elif item.type == "artifact":
                results.append(self._check_artifact(item))
            else:  # protected
                if violations is None:
                    snapshot = self.protected_snapshot
                    violations = dict(snapshot.violations()) if snapshot is not None else {}
                results.append(self._check_protected(item, violations))
        return GoalReport(
            passed=all(r.passed for r in results),
            items=results,
            fingerprint=fingerprint,
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
            stdout, _ = await asyncio.wait_for(
                proc.communicate(), timeout=self.command_timeout_s
            )
        except asyncio.TimeoutError:
            await kill_process_tree(proc)
            return self._result(
                item,
                passed=False,
                detail=f"命令超时（>{self.command_timeout_s:g}s），已强制终止进程树",
            )

        output = stdout.decode("utf-8", errors="replace").replace("\r\n", "\n")
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
