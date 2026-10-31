"""Optional native Windows terminal regressions: python tests/agent/cli-pty.py.

Uses the installed pywinpty ConPTY backend; no clipboard is modified.
"""
import json
import os
import re
from pathlib import Path
import select
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest

try:
    from winpty import Backend, PtyProcess
except ImportError:
    PtyProcess = None

ROOT = Path(__file__).resolve().parents[2]
AGENT = Path(os.environ.get("MINICODE_AGENT", ROOT / "target/debug/minicode.exe"))


@unittest.skipUnless(os.name == "nt" and PtyProcess, "requires Windows and pywinpty")
class ConsoleTests(unittest.TestCase):
    def key(self, proc, virtual_key, character, repetitions=1):
        # ConPTY treats raw LF/ETX as transport controls. Insert actual Windows
        # key records in a separate helper attached only to this test console.
        code = r'''
import ctypes,sys
from ctypes import wintypes as w
k=ctypes.WinDLL('kernel32',use_last_error=True)
k.FreeConsole()
if not k.AttachConsole(int(sys.argv[1])):raise ctypes.WinError(ctypes.get_last_error())
class Char(ctypes.Union):_fields_=[('UnicodeChar',w.WCHAR),('AsciiChar',ctypes.c_char)]
class Key(ctypes.Structure):_fields_=[('down',w.BOOL),('repeat',w.WORD),('vk',w.WORD),('scan',w.WORD),('char',Char),('mods',w.DWORD)]
class Event(ctypes.Union):_fields_=[('key',Key)]
class Record(ctypes.Structure):_fields_=[('kind',w.WORD),('event',Event)]
k.CreateFileW.restype=w.HANDLE
h=k.CreateFileW('CONIN$',0xc0000000,3,None,3,0,None)
count=2*int(sys.argv[4]);records=(Record*count)()
for index in range(count):
 down=index%2==0
 records[index].kind=1;event=records[index].event.key
 event.down=down;event.repeat=1;event.vk=int(sys.argv[2]);event.char.UnicodeChar=chr(int(sys.argv[3]));event.mods=8
written=w.DWORD()
k.WriteConsoleInputW.argtypes=[w.HANDLE,ctypes.POINTER(Record),w.DWORD,ctypes.POINTER(w.DWORD)]
if not k.WriteConsoleInputW(h,records,count,ctypes.byref(written)):raise ctypes.WinError(ctypes.get_last_error())
k.CloseHandle.argtypes=[w.HANDLE];k.CloseHandle(h);k.FreeConsole()
'''
        result = subprocess.run([sys.executable, "-c", code, str(proc.pid), str(virtual_key), str(character), str(repetitions)], capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))

    def launch(self, turns):
        directory = tempfile.TemporaryDirectory(prefix="minicode-stage7-pty-")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        script = root / "script.json"
        script.write_text(json.dumps({"turns": turns}), encoding="utf-8")
        env = dict(os.environ, HOME=str(root), USERPROFILE=str(root),
                   MINICODE_DB_PATH=str(root / "sessions.db"),
                   MINICODE_FAKE_SCRIPT=str(script), ANTHROPIC_API_KEY="", COMMANDCODE_API_KEY="", TERM="xterm-256color")
        env.pop("FORCE_COLOR", None)
        powershell = Path(r"C:\Program Files\PowerShell\7")
        if powershell.is_dir(): env["PATH"] = str(powershell) + os.pathsep + env.get("PATH", "")
        proc = PtyProcess.spawn([str(AGENT), "chat", "--provider", "fake", "--script", str(script),
                                 "--workspace", str(root), "--yes"], cwd=str(ROOT), env=env,
                                dimensions=(30, 100), backend=Backend.ConPTY)
        def terminate():
            if proc.pty.isalive():
                subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True, timeout=5)
                for _ in range(30):
                    if not proc.pty.isalive(): break
                    time.sleep(0.05)
            proc.close(force=True)
        self.addCleanup(terminate)
        self.output = ""
        self.expect(proc, "Ctrl+D")
        return proc, root

    def expect(self, proc, text, after=0, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            visible = re.sub(r"\x1b\[(\d*)C", lambda match: " " * int(match[1] or 1), self.output[after:])
            visible = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]", "", visible)
            if text in visible:
                return len(self.output)
            if select.select([proc.fileobj], [], [], 0.05)[0]:
                try:
                    self.output += proc.read(65536)
                except EOFError:
                    break
        self.fail(f"terminal did not emit {text!r}: {self.output[-3000:]}")

    def wait_exit(self, proc):
        deadline = time.monotonic() + 5
        while proc.pty.isalive() and time.monotonic() < deadline:
            if select.select([proc.fileobj], [], [], 0.05)[0]:
                try:
                    self.output += proc.read(65536)
                except EOFError:
                    break
        self.assertFalse(proc.pty.isalive(), self.output[-2000:])

    def test_l13_history_cursor_and_multiline_editing(self):
        proc, root = self.launch([{"text": "FIRST_DONE"}, {"text": "SECOND_DONE"}, {"text": "MULTILINE_DONE"}])
        proc.write("helo\x1b[Dl\r")
        at = self.expect(proc, "FIRST_DONE")
        proc.write("\x1b[A\r")
        self.expect(proc, "SECOND_DONE", at)
        proc.write("line one")
        self.key(proc, 0x4A, 10)
        proc.write("line two\r")
        self.expect(proc, "MULTILINE_DONE")
        proc.write("/quit\r")
        self.wait_exit(proc)
        database = sqlite3.connect(root / "sessions.db")
        try:
            blocks = [json.loads(row[0]) for row in database.execute("SELECT content FROM messages WHERE role='user' ORDER BY seq")]
        finally:
            database.close()
        texts = [block["text"] for content in blocks for block in content if block.get("type") == "text"]
        self.assertEqual([text for text in texts if not text.startswith("<system-reminder>")], ["hello", "hello", "line one\nline two"])

    def test_l11_idle_interrupt_after_a_completed_run(self):
        proc, _ = self.launch([{"text": "RUN_DONE"}])
        proc.write("work\r")
        self.expect(proc, "RUN_DONE")
        self.expect(proc, "❯ ", self.output.rfind("RUN_DONE") + len("RUN_DONE"))
        at=len(self.output)
        self.key(proc, 0x43, 3)
        self.expect(proc, "再按一次", at)
        self.assertTrue(proc.pty.isalive())
        self.key(proc, 0x43, 3)
        self.wait_exit(proc)

    def test_l11_running_interrupt_cancels(self):
        proc, _ = self.launch([{"tool_calls":[{"name":"powershell","arguments":{"command":"Start-Sleep -Seconds 30"}}]},{"text":"done"}])
        self.expect(proc, "❯ ")
        proc.write("run command\r")
        self.expect(proc, "Start-Sleep")
        time.sleep(0.2)
        self.key(proc, 0x43, 3)
        self.expect(proc, "cancelled")
        proc.write("/quit\r")
        self.wait_exit(proc)

    def test_l11_second_running_interrupt_forces_exit(self):
        proc, _ = self.launch([{"tool_calls":[{"name":"powershell","arguments":{"command":"Start-Sleep -Seconds 30"}}]},{"text":"done"}])
        self.expect(proc, "❯ ")
        proc.write("run command\r")
        self.expect(proc, "Start-Sleep")
        time.sleep(0.2)
        self.key(proc, 0x43, 3, repetitions=2)
        self.wait_exit(proc)
        self.assertEqual(proc.exitstatus, 130)


if __name__ == "__main__":
    unittest.main(verbosity=2)
