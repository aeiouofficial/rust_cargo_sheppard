@echo off
setlocal

title cargo-shepherd
cd /d "%~dp0"

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start_shepherd.ps1"
exit /b %ERRORLEVEL%
