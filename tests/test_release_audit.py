"""Publication checks must reject credentials without leaking them in errors."""
import importlib.util
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location("release_audit", Path(__file__).resolve().parents[1] / "release/audit.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


def test_known_credentials_are_rejected_without_printing_values(tmp_path, monkeypatch):
    secret = b"local-private-credential-123456789"
    monkeypatch.setattr(audit, "local_secrets", lambda: {secret})
    (tmp_path / "asset.bin").write_bytes(b"binary-prefix\x00" + secret)
    with pytest.raises(SystemExit) as error:
        audit.audit(tmp_path)
    assert 'asset.bin' in str(error.value)
    assert secret.decode() not in str(error.value)


def test_public_ca_bundles_are_allowed_but_private_keys_are_rejected(tmp_path, monkeypatch):
    monkeypatch.setattr(audit, "local_secrets", lambda: set())
    certificate = tmp_path / "cacert.pem"
    certificate.write_text('-----BEGIN CERTIFICATE-----\npublic\n-----END CERTIFICATE-----')
    audit.audit(tmp_path)
    certificate.write_text('-----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY-----')
    with pytest.raises(SystemExit, match='private key'):
        audit.audit(tmp_path)


def test_personal_config_is_rejected_even_if_it_contains_no_secret(tmp_path, monkeypatch):
    monkeypatch.setattr(audit, "local_secrets", lambda: set())
    (tmp_path / "desktop-config.json").write_text('{}')
    with pytest.raises(SystemExit, match='personal file'):
        audit.audit(tmp_path)
