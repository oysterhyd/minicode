"""Tests discover only their own local resources, never a developer's MCPs."""
import pytest


@pytest.fixture(autouse=True)
def isolated_user_resources(tmp_path_factory, monkeypatch):
    home = tmp_path_factory.mktemp("test-home")
    # Scoped to this test and its subprocesses; pytest restores every value.
    for variable in ("HOME", "USERPROFILE"):
        monkeypatch.setenv(variable, str(home))
    monkeypatch.setenv("APPDATA", str(home / "AppData" / "Roaming"))
    monkeypatch.setenv("LOCALAPPDATA", str(home / "AppData" / "Local"))
    monkeypatch.delenv("PI_DESKTOP_AGENTS_DIR", raising=False)
