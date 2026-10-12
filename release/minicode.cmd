@echo off
setlocal
set "MINICODE_RESOURCES=%~dp0"
if exist "%~dp0resources\runtime\minicode.exe" set "MINICODE_RESOURCES=%~dp0resources"
set "PATH=%MINICODE_RESOURCES%\runtime\node;%MINICODE_RESOURCES%\runtime\git\cmd;%PATH%"
"%MINICODE_RESOURCES%\runtime\minicode.exe" %*
exit /b %errorlevel%
