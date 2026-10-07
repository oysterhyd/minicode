@echo off
setlocal
set "PATH=%~dp0resources\runtime\python;%~dp0resources\runtime\python\Scripts;%~dp0resources\runtime\git\cmd;%PATH%"
set "PYTHONHOME="
set "PYTHONPATH="
set "PYTHONUTF8=1"
set "PYTHONIOENCODING=utf-8"
set "MINICODE_RESOURCES=%~dp0resources"
"%~dp0resources\runtime\python\python.exe" -I -X utf8 -m minicode %*
exit /b %errorlevel%
