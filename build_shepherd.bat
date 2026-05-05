@echo off
setlocal

title cargo-shepherd build
cd /d "%~dp0"

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build_shepherd.ps1"
exit /b %ERRORLEVEL%
