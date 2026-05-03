@echo off
setlocal enabledelayedexpansion

title 🐑 cargo-shepherd
cd /d "%~dp0"

if "%SHEPHERD_SLOTS%"=="" set "SHEPHERD_SLOTS=0"
set "SHEPHERD_SLOT_LABEL=%SHEPHERD_SLOTS%"
if "%SHEPHERD_SLOTS%"=="0" set "SHEPHERD_SLOT_LABEL=unlimited"

echo 🐑 Starting cargo-shepherd daemon silently (slots: %SHEPHERD_SLOT_LABEL%)...
powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "$wd=(Get-Location).Path; $exe=Join-Path $wd 'shepherd.exe'; Start-Process -FilePath $exe -ArgumentList @('daemon','--slots','%SHEPHERD_SLOTS%') -WorkingDirectory $wd -WindowStyle Hidden"

timeout /t 2 /nobreak >nul

echo.
echo 🐑 Launching TUI Dashboard...
shepherd.exe tui

pause
