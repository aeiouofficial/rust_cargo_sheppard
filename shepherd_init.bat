@echo off
setlocal
cd /d "%~dp0"
REM Wrapper for the location-agnostic release script
powershell -ExecutionPolicy Bypass -File ".\shepherd_init.ps1"
if %errorlevel% neq 0 pause
endlocal
