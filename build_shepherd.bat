@echo off
setlocal enabledelayedexpansion

title 🐑 cargo-shepherd - BUILD

REM Navigate to release root (bin folder is one level down from root)
cd /d "%~dp0.."

echo 🐑 Building cargo-shepherd...

REM Build with retry on lock failures
cargo build --bin shepherd 2>nul
if errorlevel 1 (
    echo ⚠ Build blocked. Cleaning and retrying...
    taskkill /F /IM shepherd.exe /T 2>nul
    timeout /t 2 /nobreak
    cargo build --bin shepherd
)

if errorlevel 1 (
    echo ✗ Build failed.
    pause
    exit /b 1
)

echo ✓ Build successful

REM Copy exe to release root
copy /Y "target\debug\shepherd.exe" "shepherd.exe" >nul 2>&1
echo ✓ Copied shepherd.exe to root folder

pause
