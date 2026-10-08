"""Verify the Unicode Shell link target, including Windows 8.3 aliases."""
import sys
from pathlib import Path

import pythoncom
from win32com.shell import shell

link = pythoncom.CoCreateInstance(
    shell.CLSID_ShellLink, None, pythoncom.CLSCTX_INPROC_SERVER, shell.IID_IShellLink
)
# pywin32 uses IShellLinkW; WScript.Shell returns ANSI-mangled paths on some locales.
assert str(shell.IID_IShellLink).lower().startswith("{000214f9-")
link.QueryInterface(pythoncom.IID_IPersistFile).Load(sys.argv[1])
actual = Path(link.GetPath(0)[0]).resolve(strict=True)
expected = Path(sys.argv[2]).resolve(strict=True)
if actual != expected:
    raise RuntimeError(f"Incorrect Start Menu shortcut: {actual} != {expected}")
print("Unicode Start Menu shortcut verified")
