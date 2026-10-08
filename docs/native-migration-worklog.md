# Desktop native migration work log

This file records the active migration and its verification evidence. It is not
a release completion report.

## Target

Replace the production Electron/React Desktop with Mygo native UI. Preserve the
existing appearance, component positions, colors, motion, pointer interactions
and complete functionality. Retain the Python runtime, CLI and TUI. Update the
README introduction image with image generation, rewrite documentation, build
and validate the Windows installer, and publish measured size/latency comparisons.

## Baseline

- Repository: `D:\miniclaudecode`; PowerShell 7.
- Mygo 0.2.15, Go 1.27.1. Native smoke compilation and headless UI/NDJSON
  integration verification passed during feasibility assessment.
- Original 1.1.0 installer: 200,622,437 bytes.
- Baseline metadata: `output/baseline/release.json`.
- Original algorithm results: `output/baseline/algorithms.json`.
- Original screenshots: `output/playwright/`, including `home-dark.png`,
  `conversation-dark.png` and `settings-dark.png`.
- Original renderer dev server: port 5173. Playwright CLI session:
  `minicode-baseline`. Fixture: `desktop/tests/ui-fixture.cjs`.
- Windows Computer Use native pipe is unavailable after the prescribed retries
  and reset. Use browser capture for the original renderer and Mygo's own
  capture/testing interfaces for native verification.
- Original UI checks reach a failure at "Queued messages can be removed";
  retain the evidence and verify the native queue implementation independently.

## Architecture and implementation

Go module: `desktop/native`, import path `minicode.desktop`, Mygo pinned to
0.2.15. Native rendering uses Go components, no webview. Current work:

- `internal/bridge`: concurrent NDJSON v2 client and development/packaged
  Python process launch. Graceful shutdown and hidden Windows subprocesses.
- `internal/model`: protocol types, history conversion and feed grouping.
- `internal/workspace`: canonical workspace containment, bounded concurrent
  Git commands, file enumeration/preview, changes, diffs and stage/unstage.
- `internal/content`: bounded Myers diff and unified-diff parsing.
- Controller and native components now compile and run. Native GUI capture
  succeeded; stripped executable was 16,454,656 bytes before the subsequent
  LevelDB migration dependency was added. This is not an installer measurement.
- `go test -count=1 ./...` passed, including real Python NDJSON concurrency,
  service credential redaction/preservation, delegate creation/rename,
  streaming, approval before a real temporary-workspace write, and durable
  history recovery.
- 51 focused existing Python desktop/harness tests passed with a fresh
  workspace `--basetemp`. The first invocation's assertions passed but pytest
  cleanup failed on an existing external temp-directory permission error.
- Native snapshot tests write 18 light/dark captures to `output/native/`.
  Snapshot tests set ReduceMotion to avoid comparing halfway theme transitions.
- Original measured home geometry at 1500x940: main content x=277, y=41,
  w=806, h=890; welcome x=315, y=152.90625, w=720, h=666.1875;
  logo x=651, y=200.90625, w=48, h=48; composer x=347,
  y=367.765625, w=656, h=112. Original scrollbars reserve a 10px gutter.
- Native caret insertion/undo, text-key reservation, paint-only button press
  scaling and horizontal list scrolling are small extensions to vendored
  Mygo. `toolkit/refresh-vendor.ps1` reapplies them after vendoring;
  extension sources are retained as `.go.txt` files.
- Chromium Local Storage settings/history migration reads a temporary copy of
  the app's LevelDB, including UTF-16 strings. A locked original DB migration
  test passes. Python service credentials and SQLite stay in their existing
  locations.

## Completed follow-up

- Completed code-block layout, grouped tools, context/cache/TPS, tasks, usage,
  settings blur, async images and updater controls. Native UI regression and
  real NDJSON integration tests pass.
- Added Windows immediate response frames, nonblocking D3D present and
  refresh-rate paced animation frames; replayable toolkit patches retained.
- Full Python suite: 560 passed, 3 skipped; Ink TUI: 79 passed. Native tests,
  vet and CGO-free Windows/Linux compilation pass. Windows actual GPU frames
  and packaged runtime smoke pass.
- Replaced Electron packaging with MyGo native NSIS and retained bundled
  Python/Git/Node/TUI. Generated a new README image from native references.
- Added official updater/native, default automatic checks, optional automatic
  download/install, signed release archive and CI secret integration. Actual
  SDK rejects signature tampering, installs the valid complete runtime and
  ignores the same version; updated Unicode/space-path runtime smoke passes.
- Final measurements and verification scope are recorded in
  `docs/native-desktop-report.md` and `docs/assets/native-performance.json`.

Physical IME candidate windows, every notification click and full-motion
pixel equivalence have not been manually verified. CPU scene capture and GPU
frame evidence are distinguished; no model/startup/memory improvement claimed.
