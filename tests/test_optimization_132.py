"""Behavioral equivalence at Unicode, pairing and package boundaries."""
import importlib.util
import random
from pathlib import Path

import pytest

from minicode.context.compact import _paired_prefix, _pairing_ok
from minicode.core.models import Message, TextBlock, ToolUseBlock, ToolResultBlock
from minicode.tools.base import ToolContext, tail_output, utf8_prefix
from minicode.tools.search import GrepTool


def reference_tail(text, max_bytes, max_lines):
    tail = "".join(text.splitlines(keepends=True)[-max_lines:])
    raw = tail.encode("utf-8")
    return raw[-max_bytes:].decode("utf-8", errors="ignore") if len(raw) > max_bytes else tail


def test_bounded_output_matches_full_copy_with_unicode_and_split_crlf():
    rng = random.Random(132)
    alphabet = "ab中文🙂\r\n\v\f\x1c\x1d\x1e\x85\u2028\u2029"
    for _ in range(1000):
        text = "".join(rng.choices(alphabet, k=rng.randrange(80)))
        for limit in (0, 1, 2, 3, 7, 31, 500):
            assert utf8_prefix(text, limit) == text.encode("utf-8")[:limit].decode("utf-8", errors="ignore")
            for lines in (-1, 0, 1, 2, 5):
                assert tail_output(text, limit, lines) == reference_tail(text, limit, lines)


def test_prefix_pairing_matches_all_candidates_with_duplicate_and_orphan_ids():
    rng = random.Random(132)
    for _ in range(500):
        messages = [Message(role="assistant", content=[ToolUseBlock(id="head", name="read", input={})])]
        units = []
        for _ in range(rng.randrange(1, 20)):
            start = len(messages)
            blocks = []
            for _ in range(rng.randrange(6)):
                call_id = str(rng.randrange(6))
                blocks.append(ToolUseBlock(id=call_id, name="read", input={}) if rng.randrange(2)
                              else ToolResultBlock(tool_use_id=call_id, content="result"))
            messages.extend([Message(role="user", content=[TextBlock(text="task")]),
                             Message(role="assistant", content=blocks)])
            units.append((start, len(messages)))
        for keep in range(len(units)):
            early = units[:len(units) - keep]
            expected = max((k for k in range(1, len(early) + 1)
                            if _pairing_ok(messages, early[0][0], early[k - 1][1])), default=0)
            assert _paired_prefix(messages, early) == expected
    assert _paired_prefix([], []) == 0


@pytest.mark.parametrize("text", ["", "\n", "a\r\nb\rc\n", "a\v\f\x1c\x1d\x1e\x85\u2028\u2029b\n", "中文🙂" * 20000, "a" * 8191 + "\r\nend", "x" * 8191 + "🙂\u2028end"], ids=["empty", "blank", "crlf", "unicode-separators", "long-line", "crlf-boundary", "utf8-boundary"])
def test_streamed_search_lines_preserve_all_splitlines_separators(tmp_path, text):
    file = tmp_path / "text.txt"
    file.write_text(text, encoding="utf-8", newline="")
    assert list(GrepTool._lines(file, ToolContext(tmp_path))) == text.splitlines()


def test_streamed_search_keeps_utf8_replacement_and_binary_sniff(tmp_path):
    file = tmp_path / "bad.txt"
    data = b"a\n\xff\xe4\xb8\xad\r\nb"
    file.write_bytes(data)
    assert list(GrepTool._lines(file, ToolContext(tmp_path))) == data.decode("utf-8", errors="replace").splitlines()
    file.write_bytes(b"a\x00b")
    assert list(GrepTool._lines(file, ToolContext(tmp_path))) == []


def test_streamed_search_unicode_separators_cross_chunk_boundaries(tmp_path):
    rng = random.Random(132)
    text = "".join(rng.choices("abc中文🙂\r\n\v\f\x1c\x1d\x1e\x85\u2028\u2029", k=100000))
    file = tmp_path / "separators.txt"
    file.write_text(text, encoding="utf-8", newline="")
    assert list(GrepTool._lines(file, ToolContext(tmp_path))) == text.splitlines()


def test_runtime_pruning_preserves_code_package_data_and_licenses(tmp_path):
    spec = importlib.util.spec_from_file_location("prune_runtime", Path(__file__).resolve().parents[1] / "release/prune-runtime.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    root = tmp_path / "tui/node_modules/example"
    root.mkdir(parents=True)
    removed = ["index.d.ts", "index.d.mts", "index.d.cts", "index.js.map"]
    retained = ["index.js", "package.json", "LICENSE", "NOTICE", "data.json", "layout.wasm", "source.ts"]
    for name in removed + retained:
        (root / name).write_bytes(b"123")
    assert module.prune(tmp_path) == (4, 12)
    assert sorted(p.name for p in root.iterdir()) == sorted(retained)
    assert module.prune(tmp_path) == (0, 0)
