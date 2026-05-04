@echo off
setlocal enabledelayedexpansion

title 🐑 cargo-shepherd
set "REPO_DIR=%~dp0"
if not exist "%REPO_DIR%Cargo.toml" set "REPO_DIR=%~dp0..\"
cd /d "%REPO_DIR%"

if "%SHEPHERD_SLOTS%"=="" set "SHEPHERD_SLOTS=0"
set "SHEPHERD_BAD_SLOTS="
for /f "delims=0123456789" %%A in ("%SHEPHERD_SLOTS%") do set "SHEPHERD_BAD_SLOTS=1"
if defined SHEPHERD_BAD_SLOTS (
	echo ✗ SHEPHERD_SLOTS must be 0 ^(unlimited^) or a positive integer.
	pause
	exit /b 1
)

set "SHEPHERD_SLOT_LABEL=%SHEPHERD_SLOTS%"
if "%SHEPHERD_SLOTS%"=="0" set "SHEPHERD_SLOT_LABEL=unlimited"

set "CARGO_TARGET_DIR=target_tmp_%RANDOM%"
set "SHEPHERD_EXE=%CD%\shepherd.exe"
set "BUILT_EXE=%CD%\%CARGO_TARGET_DIR%\debug\shepherd.exe"

echo Checking for build locks and compiling with isolated target: %CARGO_TARGET_DIR%
cargo build --bin shepherd -j 1
if errorlevel 1 (
	echo ⚠ Build blocked. Trying one more time...
	taskkill /F /IM shepherd.exe /T >nul 2>&1
	timeout /t 2 /nobreak >nul
	cargo build --bin shepherd -j 1
)

if errorlevel 1 (
	echo ✗ Build failed.
	pause
	exit /b 1
)

if exist "%BUILT_EXE%" (
	copy /Y "%BUILT_EXE%" "%SHEPHERD_EXE%" >nul 2>&1
	if errorlevel 1 (
		echo ⚠ Could not replace shepherd.exe; using isolated build for this session.
		set "SHEPHERD_EXE=%BUILT_EXE%"
	)
)

echo 🐑 Starting cargo-shepherd daemon silently (slots: %SHEPHERD_SLOT_LABEL%)...
powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "$wd=(Get-Location).Path; $exe=$env:SHEPHERD_EXE; Start-Process -FilePath $exe -ArgumentList @('daemon','--slots','%SHEPHERD_SLOTS%') -WorkingDirectory $wd -WindowStyle Hidden"

timeout /t 2 /nobreak >nul

echo.
echo 🐑 Launching TUI Dashboard...
"%SHEPHERD_EXE%" tui

pause
