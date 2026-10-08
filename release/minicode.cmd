@echo off
setlocal
set "MINICODE_RESOURCES=%~dp0"
if exist "%~dp0resources\runtime\python\python.exe" set "MINICODE_RESOURCES=%~dp0resources"
set "PATH=%MINICODE_RESOURCES%\runtime\python;%MINICODE_RESOURCES%\runtime\python\Scripts;%MINICODE_RESOURCES%\runtime\git\cmd;%PATH%"
set "PYTHONHOME="
set "PYTHONPATH="
set "PYTHONUTF8=1"
set "PYTHONIOENCODING=utf-8"
"%MINICODE_RESOURCES%\runtime\python\python.exe" -I -X utf8 -m minicode %*
exit /b %errorlevel%
