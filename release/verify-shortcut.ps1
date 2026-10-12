param([Parameter(Mandatory)][string]$Shortcut,[Parameter(Mandatory)][string]$Expected)
$ErrorActionPreference = 'Stop'
if (-not ('MiniCodeShortcut' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class MiniCodeShortcut {
 [ComImport, Guid("00021401-0000-0000-C000-000000000046")] private class ShellLink {}
 [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] private interface IShellLinkW {
  void GetPath([Out,MarshalAs(UnmanagedType.LPWStr)] StringBuilder path,int max,IntPtr data,uint flags);
  void GetIDList(out IntPtr id);void SetIDList(IntPtr id);void GetDescription(StringBuilder value,int max);void SetDescription(string value);
  void GetWorkingDirectory(StringBuilder value,int max);void SetWorkingDirectory(string value);void GetArguments(StringBuilder value,int max);void SetArguments(string value);
  void GetHotkey(out short value);void SetHotkey(short value);void GetShowCmd(out int value);void SetShowCmd(int value);void GetIconLocation(StringBuilder value,int max,out int index);void SetIconLocation(string value,int index);void SetRelativePath(string value,uint reserved);void Resolve(IntPtr hwnd,uint flags);void SetPath(string value);
 }
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] private static extern uint GetFinalPathNameByHandleW(IntPtr handle,StringBuilder path,uint size,uint flags);
 public static string Canonical(string path) {using(var stream=File.Open(path,FileMode.Open,FileAccess.Read,FileShare.ReadWrite|FileShare.Delete)){var b=new StringBuilder(32768);if(GetFinalPathNameByHandleW(stream.SafeFileHandle.DangerousGetHandle(),b,(uint)b.Capacity,0)==0)throw new System.ComponentModel.Win32Exception();return b.ToString();}}
 public static string Read(string path){object link=new ShellLink();try{((IPersistFile)link).Load(path,0);var b=new StringBuilder(32768);((IShellLinkW)link).GetPath(b,b.Capacity,IntPtr.Zero,0);return Canonical(b.ToString());}finally{Marshal.ReleaseComObject(link);}}
}
'@
}
$actual = [MiniCodeShortcut]::Read($Shortcut)
$target = [MiniCodeShortcut]::Canonical($Expected)
if (-not [StringComparer]::OrdinalIgnoreCase.Equals($actual,$target)) { throw "Incorrect Start Menu shortcut: $actual != $target" }
Write-Host 'Unicode Start Menu shortcut verified'
