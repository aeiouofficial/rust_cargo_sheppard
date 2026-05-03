# start_shepherd.ps1
# 🐑 cargo-shepherd TUI Launcher

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

Push-Location $scriptDir

Write-Host "🐑 Starting cargo-shepherd daemon in background..." -ForegroundColor Green
Start-Process powershell -ArgumentList "-NoExit", "-Command", "shepherd.exe daemon"

Start-Sleep -Seconds 2

Write-Host "🐑 Launching TUI Dashboard..." -ForegroundColor Yellow
& .\shepherd.exe tui

Pop-Location
