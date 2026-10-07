"""CLI launcher for the Ink TUI."""
from __future__ import annotations

import sys
from pathlib import Path

from typer.testing import CliRunner

from minicode.cli import app

runner = CliRunner()


def test_tui_requires_node(tmp_path, monkeypatch):
    monkeypatch.setenv("MINICODE_NODE", "")
    monkeypatch.delenv("MINICODE_NODE", raising=False)
    monkeypatch.setattr("minicode.cli.shutil.which", lambda _name: None)
    result = runner.invoke(app, ["tui", "--workspace", str(tmp_path)])
    assert result.exit_code == 1
    assert "找不到 Node.js" in result.output


def test_tui_requires_built_entry(tmp_path, monkeypatch):
    missing = tmp_path / "missing.js"
    monkeypatch.setenv("MINICODE_NODE", "node")
    monkeypatch.setenv("MINICODE_TUI_ENTRY", str(missing))
    result = runner.invoke(app, ["tui", "--workspace", str(tmp_path)])
    assert result.exit_code == 1
    assert "终端 UI 入口不存在" in result.output


def test_tui_launches_node_entry(tmp_path, monkeypatch):
    entry = tmp_path / "index.js"
    entry.write_text("process.stdout.write(process.argv.slice(2).join(' '))", encoding="utf-8")
    captured: list[list[str]] = []

    class Completed:
        returncode = 0

    def fake_run(command, **_kwargs):
        captured.append(list(command))
        return Completed()

    monkeypatch.setenv("MINICODE_NODE", "node")
    monkeypatch.setenv("MINICODE_TUI_ENTRY", str(entry))
    monkeypatch.setattr("minicode.cli.subprocess.run", fake_run)
    result = runner.invoke(app, ["tui", "--workspace", str(tmp_path), "--provider", "fake", "--yes"])
    assert result.exit_code == 0, result.output
    assert captured
    command = captured[0]
    assert command[0] == "node"
    assert command[1] == str(entry)
    assert "--workspace" in command
    assert "--provider" in command and "fake" in command
    assert "--yes" in command


def test_bridge_reads_fake_script_and_db_env(tmp_path, monkeypatch):
    desktop = Path(__file__).resolve().parents[1] / "desktop"
    if str(desktop) not in sys.path:
        sys.path.insert(0, str(desktop))
    import bridge as bridge_mod  # noqa: E402

    script = tmp_path / "script.json"
    script.write_text('{"turns":[{"text":"from-script"}]}', encoding="utf-8")
    db = tmp_path / "custom.sqlite3"
    monkeypatch.setenv("MINICODE_FAKE_SCRIPT", str(script))
    monkeypatch.setenv("MINICODE_DB_PATH", str(db))
    provider = bridge_mod._fake_provider()
    assert provider.options.turns[0].text == "from-script"
    store = bridge_mod._open_store()
    try:
        assert store._db_path == db.resolve()
    finally:
        store.close()


def test_tui_rejects_missing_workspace(tmp_path, monkeypatch):
    monkeypatch.setenv("MINICODE_NODE", "node")
    monkeypatch.setenv("MINICODE_TUI_ENTRY", str(tmp_path / "index.js"))
    result = runner.invoke(app, ["tui", "--workspace", str(tmp_path / "nope")])
    assert result.exit_code == 1
    assert "工作区不存在" in result.output


def test_tui_packaged_resources_take_precedence_over_checkout(tmp_path, monkeypatch):
    from minicode.cli import _tui_entry, _tui_node

    resources = tmp_path / "installed resources"
    runtime = resources / "runtime"
    entry = runtime / "tui" / "dist" / "index.js"
    node = runtime / "node" / "node.exe"
    entry.parent.mkdir(parents=True)
    node.parent.mkdir()
    entry.write_text("", encoding="utf-8")
    node.write_bytes(b"")
    (runtime / "bridge.py").write_text("", encoding="utf-8")
    monkeypatch.setenv("MINICODE_RESOURCES", str(resources))
    monkeypatch.delenv("MINICODE_TUI_ENTRY", raising=False)
    monkeypatch.delenv("MINICODE_NODE", raising=False)
    monkeypatch.setattr("minicode.cli.shutil.which", lambda _name: None)
    assert _tui_entry() == entry
    assert _tui_node() == str(node)


def test_tui_forwards_zero_budgets_and_active_python(tmp_path, monkeypatch):
    entry = tmp_path / "index.js"
    entry.write_text("", encoding="utf-8")
    monkeypatch.setenv("MINICODE_NODE", "node")
    monkeypatch.setenv("MINICODE_TUI_ENTRY", str(entry))
    captured = {}

    def fake_run(command, **kwargs):
        captured.update(command=command, **kwargs)
        return type("Completed", (), {"returncode": 0})()

    monkeypatch.setattr("minicode.cli.subprocess.run", fake_run)
    result = runner.invoke(app, ["tui", "--workspace", str(tmp_path), "--max-rounds", "0", "--max-tokens", "0", "--max-seconds", "0"])
    assert result.exit_code == 0, result.output
    command = captured["command"]
    for option in ("--max-rounds", "--max-tokens", "--max-seconds"):
        assert float(command[command.index(option) + 1]) == 0
    assert captured["env"]["MINICODE_PYTHON"] == sys.executable
