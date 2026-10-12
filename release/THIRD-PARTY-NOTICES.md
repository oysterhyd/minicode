# Bundled software

MiniCode is distributed under MIT (runtime/LICENSE).

- Rust dependencies are locked in Cargo.lock. Package versions, license
  expressions and repositories: runtime/licenses/rust/dependencies.json.
  LICENSE, COPYING and NOTICE files are retained beside each entry.
- Native Desktop uses Go / MyGo 0.2.15. MyGo, Goldmark, Chroma, purego,
  Go text/image, LevelDB and Snappy notices are under licenses/ beside MiniCode.exe.
  Versions: desktop/native/go.mod and vendor/modules.txt.
- Lucide / Feather icons: ISC and MIT notices under licenses/lucide/.
- MinGit 2.56.0: Git GPL v2 and component notices retained under runtime/git.
  Source: https://github.com/git-for-windows/git/tree/v2.56.0.windows.1
  and https://github.com/git-for-windows/build-extra.
- Node.js 22.22.0: MIT and component notices in runtime/node/LICENSE.
  Source: https://nodejs.org/dist/v22.22.0/
- Ink / React terminal dependencies: locked versions and integrity hashes
  in runtime/tui/package-lock.json, upstream licenses retained in node_modules.
- SQLite is compiled into Rust through rusqlite / libsqlite3-sys.
  SQLite is public domain; wrapper notices are in the Rust notice directory.

MyGo source: https://github.com/egoist/mygo/tree/v0.2.15.
No personal model credentials or user configuration are bundled.
