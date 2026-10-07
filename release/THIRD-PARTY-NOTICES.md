# Bundled software

MiniCode is distributed under the MIT license (resources/runtime/LICENSE).

- Electron 44.2.0: MIT; Chromium and other component licenses are in
  LICENSE.electron.txt and LICENSES.chromium.html beside MiniCode.exe.
- CPython 3.12.12 (python-build-standalone): PSF and component licenses;
  resources/runtime/python/LICENSE.txt.
- MinGit 2.56.0: Git GPL v2 and component licenses;
  resources/runtime/git contains the upstream license files. Corresponding
  source: https://github.com/git-for-windows/git/tree/v2.56.0.windows.1
  and https://github.com/git-for-windows/build-extra (MinGit packaging).
- Node.js 22.22.0: MIT and component licenses;
  resources/runtime/node/LICENSE. Source: https://nodejs.org/dist/v22.22.0/
- Ink, React and other terminal UI dependencies: exact versions and integrity
  hashes in resources/runtime/tui/package-lock.json; upstream licenses are
  retained in resources/runtime/tui/node_modules.
- Python dependencies: exact versions and wheel hashes in requirements.lock;
  each distribution retains its .dist-info license/metadata in
  resources/runtime/python/Lib/site-packages.

Upstream runtime sources: https://github.com/astral-sh/python-build-standalone
and https://github.com/electron/electron/tree/v44.2.0.

No model credentials or personal configuration are included.
