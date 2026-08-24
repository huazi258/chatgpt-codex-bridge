@echo off
setlocal EnableExtensions
set "BRIDGE_ROOT=%~dp0"
if /i "%~1"=="--check" goto :check
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%BRIDGE_ROOT%scripts\start-bridge.ps1" %*
exit /b %ERRORLEVEL%

:check
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%BRIDGE_ROOT%scripts\start-bridge.ps1" -Mode check
exit /b %ERRORLEVEL%
