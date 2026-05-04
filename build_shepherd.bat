@echo off
setlocal enabledelayedexpansion

title 🐑 cargo-shepherd - BUILD
set "REPO_DIR=%~dp0"
if not exist "%REPO_DIR%Cargo.toml" set "REPO_DIR=%~dp0..\"
cd /d "%REPO_DIR%"

echo 🐑 Building cargo-shepherd...

set "CARGO_TARGET_DIR=target_tmp_%RANDOM%"
set "BUILT_EXE=%CD%\%CARGO_TARGET_DIR%\debug\shepherd.exe"
set "ROOT_EXE=%CD%\shepherd.exe"

REM Build with retry on lock failures. Use an isolated target so stale locks in
REM the default target directory do not block startup.
cargo build --bin shepherd -j 1
if errorlevel 1 (
    echo ⚠ Build blocked. Cleaning and retrying...
    taskkill /F /IM shepherd.exe /T >nul 2>&1
    timeout /t 2 /nobreak >nul
    cargo build --bin shepherd -j 1
)

if errorlevel 1 (
    echo ✗ Build failed.
    pause
    exit /b 1
)

echo ✓ Build successful

REM Copy exe to root
if exist "%BUILT_EXE%" (
    copy /Y "%BUILT_EXE%" "%ROOT_EXE%" >nul 2>&1
    if errorlevel 1 (
        echo ⚠ Built successfully, but shepherd.exe is locked. Use: "%BUILT_EXE%"
    ) else (
        echo ✓ Copied shepherd.exe to root
    )
)

pause
