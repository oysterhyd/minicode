# Bundled software

MiniCode is distributed under the MIT license (runtime/LICENSE).

- The native desktop is built with Go and Mygo 0.2.15. Mygo, Goldmark,
  Chroma, purego, Go text/image packages, LevelDB and Snappy licenses are
  included under `licenses/` beside MiniCode.exe. Exact module versions
  are recorded in `desktop/native/go.mod` and `vendor/modules.txt`.
- Native UI icons use Lucide / Feather; ISC and MIT notices are included
  in `licenses/lucide/LICENSE`.
- CPython 3.12.12 (python-build-standalone): PSF and component licenses;
  runtime/python/LICENSE.txt.
- MinGit 2.56.0: Git GPL v2 and component licenses;
  runtime/git contains the upstream license files. Corresponding
  source: https://github.com/git-for-windows/git/tree/v2.56.0.windows.1
  and https://github.com/git-for-windows/build-extra (MinGit packaging).
- Node.js 22.22.0: MIT and component licenses;
  runtime/node/LICENSE. Source: https://nodejs.org/dist/v22.22.0/
- Ink, React and other terminal UI dependencies: exact versions and integrity
  hashes in runtime/tui/package-lock.json; upstream licenses are
  retained in runtime/tui/node_modules. Node.js serves the terminal TUI.
- Python dependencies: exact versions and wheel hashes in requirements.lock;
  each distribution retains its .dist-info license/metadata in
  runtime/python/Lib/site-packages.

Upstream runtime sources: https://github.com/astral-sh/python-build-standalone
and https://github.com/egoist/mygo/tree/v0.2.15.

No model credentials or personal configuration are included.
