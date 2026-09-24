#!/usr/bin/env python3
"""Render an honestly labelled video replay from a persisted real-model trace.

Optional tools: Pillow and ffmpeg. This is a condensed historical replay,
not a recording of a live terminal or a claim about model reasoning.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import subprocess
import tempfile
import textwrap
from pathlib import Path


def _slides(db: Path) -> tuple[dict, list[tuple[str, str]]]:
    conn = sqlite3.connect(db)
    conn.row_factory = sqlite3.Row
    try:
        session = conn.execute("SELECT * FROM sessions ORDER BY created_at LIMIT 1").fetchone()
        if session is None:
            raise ValueError("database has no session")
        events = conn.execute("SELECT type, timestamp, data FROM events WHERE session_id = ? ORDER BY seq",
                              (session["session_id"],)).fetchall()
        messages = conn.execute("SELECT role, content FROM messages WHERE session_id = ? ORDER BY seq",
                                (session["session_id"],)).fetchall()
        first_user = next((json.loads(row["content"]) for row in messages if row["role"] == "user"), [])
        prompt = " ".join(str(block.get("text", "")) for block in first_user if block.get("type") == "text")
        header = {"session_id": session["session_id"], "model": session["model"],
                  "created_at": session["created_at"],
                  "source_db_sha256": hashlib.sha256(db.read_bytes()).hexdigest()}
        slides = [("真实模型 trace 回放", f"模型：{session['model']}\n会话：{session['session_id']}\n\n任务：{prompt[:700]}")]
        for row in events:
            data = json.loads(row["data"])
            kind = row["type"]
            stamp = row["timestamp"]
            if kind == "assistant_message":
                text = str(data.get("text") or "")
                calls = data.get("tool_calls") or []
                if text or calls:
                    names = ", ".join(str(call.get("name") if isinstance(call, dict) else call)
                                      for call in calls)
                    slides.append(("模型输出", f"{stamp}\n\n{text[:700]}\n\n工具请求：{names}"))
            elif kind == "tool_call_result":
                status = "成功" if data.get("success") else "失败"
                slides.append((f"工具：{data.get('name')} · {status}",
                               f"{stamp}\n退出码：{data.get('exit_code')}\n\n"
                               f"{str(data.get('output_preview') or data.get('error') or '')[:900]}"))
            elif kind == "goal_check":
                items = data.get("items") or []
                details = "\n".join(f"{'✓' if item.get('passed') else '✗'} {item.get('item_id')}"
                                    for item in items)
                slides.append(("宿主验收", f"{stamp}\n通过：{data.get('passed')}\n\n{details}"))
            elif kind == "session_end":
                slides.append(("执行结果", f"{stamp}\n退出原因：{data.get('exit_reason')}\n"
                               f"轮数：{data.get('rounds')}\n\n"
                               "历史执行记录；本视频为压缩回放，不是实时屏幕录制。"))
        return header, slides
    finally:
        conn.close()


def _font(size: int):
    from PIL import ImageFont

    for path in ("C:/Windows/Fonts/msyh.ttc", "C:/Windows/Fonts/arial.ttf",
                 "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"):
        if Path(path).is_file():
            return ImageFont.truetype(path, size)
    return ImageFont.load_default()


def _draw(path: Path, title: str, body: str, index: int, total: int) -> None:
    from PIL import Image, ImageDraw

    image = Image.new("RGB", (1280, 720), "#101820")
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 0, 1280, 12), fill="#31c6a7")
    draw.text((72, 48), title, font=_font(42), fill="#f4f7fa")
    lines: list[str] = []
    for paragraph in body.splitlines():
        lines.extend(textwrap.wrap(paragraph, width=58, break_long_words=True) or [""])
    for number, line in enumerate(lines[:17]):
        draw.text((74, 138 + number * 30), line, font=_font(23), fill="#d5e1ea")
    draw.text((72, 654), "真实模型 trace · 历史执行压缩回放 · 无隐藏思维链",
              font=_font(20), fill="#80a6b6")
    draw.text((1110, 654), f"{index}/{total}", font=_font(20), fill="#80a6b6")
    image.save(path)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    header, slides = _slides(args.db)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="minicode-replay-") as directory:
        root = Path(directory)
        entries = []
        for index, (title, body) in enumerate(slides, 1):
            frame = root / f"frame-{index:03d}.png"
            _draw(frame, title, body, index, len(slides))
            entries.extend([f"file '{frame.as_posix()}'", "duration 4"])
        entries.append(f"file '{frame.as_posix()}'")
        listing = root / "frames.txt"
        listing.write_text("\n".join(entries) + "\n", encoding="utf-8")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-f", "concat", "-safe", "0",
                        "-i", str(listing), "-fps_mode", "vfr", "-c:v", "libx264",
                        "-pix_fmt", "yuv420p", "-crf", "24", str(output)], check=True)
    manifest = {**header, "format": "condensed historical trace replay",
                "slide_count": len(slides), "seconds_per_slide": 4,
                "video": output.name}
    output.with_suffix(".json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
                                           encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
