"""Reject personal files and locally known credentials without printing secrets.

Use alongside Gitleaks (including Git history). Standard library only so this
also runs against an installed distribution without developer dependencies.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
from pathlib import Path

PRIVATE_NAMES = {"desktop-config.json", "provider_config.json", "credentials.json", ".env"}
PRIVATE_PARTS = {".minicode", ".zcode", ".codex", ".claude", ".git"}


def local_secrets():
    secrets = set()
    for key, value in os.environ.items():
        if re.search(r"(API_?KEY|ACCESS_TOKEN|AUTH_TOKEN|SECRET|PASSWORD)$", key, re.I) and len(value) >= 8:
            secrets.add(value.encode())

    def visit(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if re.search(r"api.?key|token|secret|password|authorization", key, re.I) and isinstance(item, str) and len(item) >= 8:
                    secrets.add(item.encode())
                visit(item)
        elif isinstance(value, list):
            for item in value:
                visit(item)

    for relative in (".minicode/desktop-config.json", ".zcode/v2/provider_config.json"):
        file = Path.home() / relative
        if file.exists():
            visit(json.loads(file.read_text(encoding="utf-8-sig")))
    return secrets


def audit(root: Path, source=False):
    if source:
        result = subprocess.run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                                cwd=root, check=True, stdout=subprocess.PIPE)
        files = [root / os.fsdecode(name) for name in result.stdout.split(b"\0") if name]
    else:
        files = list(root.rglob("*"))
    secrets = local_secrets()
    problems = []
    count = 0
    for file in files:
        if not file.is_file():
            continue
        relative = file.relative_to(root)
        name = file.name.lower()
        if (name in PRIVATE_NAMES or (name.startswith(".env.") and name != ".env.example")
                or file.suffix.lower() in {".key", ".db", ".db-journal"}
                or PRIVATE_PARTS.intersection(part.lower() for part in relative.parts)):
            problems.append(f"personal file: {relative}")
        # Check binary assets too, in bounded chunks with overlap.
        overlap = max((len(secret) for secret in secrets), default=1) - 1
        tail = b""
        with file.open("rb") as stream:
            while chunk := stream.read(4 * 1024 * 1024):
                data = tail + chunk
                if file.suffix.lower() == '.pem' and b'PRIVATE KEY-----' in data:
                    problems.append(f"private key: {relative}")
                    break
                if any(secret in data for secret in secrets):
                    problems.append(f"local credential found: {relative}")
                    break
                tail = data[-overlap:] if overlap else b""
        count += 1
    if problems:
        raise SystemExit("Publication audit failed:\n" + "\n".join(sorted(set(problems))))
    print(f"Publication audit passed: {count} files; no personal files or known local credentials.")


def audit_history(root: Path):
    """Check every reachable Git blob for exact locally configured credentials."""
    secrets = local_secrets()
    objects = subprocess.check_output(['git', 'rev-list', '--objects', '--all'], cwd=root)
    names = {line.split(b' ', 1)[0]: line.split(b' ', 1)[-1].decode('utf-8', errors='replace')
             for line in objects.splitlines()}
    metadata = subprocess.check_output(['git', 'cat-file', '--batch-check'],
                                       input=b'\n'.join(names) + b'\n', cwd=root)
    blobs = [line.split()[0] for line in metadata.splitlines() if line.split()[1] == b'blob']
    problems = []
    # A single cat-file process avoids hundreds of process launches.
    with subprocess.Popen(['git', 'cat-file', '--batch'], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE) as reader:
        for sha in blobs:
            reader.stdin.write(sha + b'\n')
            reader.stdin.flush()
            size = int(reader.stdout.readline().split()[2])
            remaining = size
            tail = b''
            overlap = max((len(secret) for secret in secrets), default=1) - 1
            found = False
            while remaining:
                chunk = reader.stdout.read(min(remaining, 4 * 1024 * 1024))
                if not chunk:
                    raise RuntimeError('Unexpected end of Git blob')
                remaining -= len(chunk)
                data = tail + chunk
                found |= any(secret in data for secret in secrets)
                tail = data[-overlap:] if overlap else b''
            reader.stdout.read(1)
            if found:
                problems.append(f'{names[sha]} (blob {sha.decode()})')
        reader.stdin.close()
        if reader.wait() != 0:
            raise RuntimeError('Git history inspection failed')
    if problems:
        raise SystemExit('Local credential in Git history:\n' + '\n'.join(problems))
    print(f'Git history audit passed: {len(blobs)} blobs; no known local credentials.')


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=Path)
    parser.add_argument("--source", action="store_true")
    parser.add_argument("--history", action="store_true")
    args = parser.parse_args()
    audit(args.path.resolve(), args.source)
    if args.history:
        audit_history(args.path.resolve())
