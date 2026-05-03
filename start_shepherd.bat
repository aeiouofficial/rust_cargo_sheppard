@echo off
setlocal enabledelayedexpansion

title 🐑 cargo-shepherd
cd /d "%~dp0"

echo 🐑 Starting cargo-shepherd daemon in background...
start "Shepherd Daemon" cmd /k "shepherd.exe daemon"

timeout /t 2 /nobreak

echo.
echo 🐑 Launching TUI Dashboard...
shepherd.exe tui

pause
