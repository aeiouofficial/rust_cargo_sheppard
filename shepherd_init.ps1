# shepherd_init.ps1
# 🐑 cargo-shepherd Release Initializer
# Location-agnostic script for GitHub releases

$scriptDir = $PSScriptRoot
Set-Location $scriptDir

Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "   🐑 cargo-shepherd v0.2.0 Initializer   " -ForegroundColor Cyan
Write-Host "==========================================" -ForegroundColor Cyan
Write-Host "Location: $scriptDir`n" -ForegroundColor Gray

# 1. Check for binary
$exePath = Join-Path $scriptDir "shepherd.exe"
if (!(Test-Path $exePath)) {
    # Check in target/debug or target/release if they exist
    if (Test-Path "target\release\shepherd.exe") { $exePath = "target\release\shepherd.exe" }
    elseif (Test-Path "target\debug\shepherd.exe") { $exePath = "target\debug\shepherd.exe" }
}

# 2. Check for Cargo (source build)
$hasCargo = (Get-Command cargo -ErrorAction SilentlyContinue) -ne $null
$hasSource = Test-Path "Cargo.toml"

if (Test-Path $exePath) {
    Write-Host "✔ Found executable: $exePath" -ForegroundColor Green
    Write-Host "Starting Daemon..." -ForegroundColor Gray
    Start-Process powershell -ArgumentList "-NoExit", "-Command", "& '$exePath' daemon"
    Start-Sleep -Seconds 1
    Write-Host "Launching TUI..." -ForegroundColor Yellow
    & $exePath tui
}
elseif ($hasSource -and $hasCargo) {
    Write-Host "ℹ No binary found, but source code and Cargo are present." -ForegroundColor Cyan
    $choice = Read-Host "Do you want to build and start cargo-shepherd now? (Y/N)"
    if ($choice -eq "Y" -or $choice -eq "y") {
        Write-Host "Compiling... (this may take a minute)" -ForegroundColor Gray
        
        # NUCLEAR OPTION: Randomized target dir to bypass persistent locks
        $randomId = Get-Random -Minimum 1000 -Maximum 9999
        $env:CARGO_TARGET_DIR = "target_tmp_$randomId"

        $maxRetries = 2
        $retryCount = 0
        $success = $false

        while ($retryCount -lt $maxRetries -and -not $success) {
            cargo build --release -j 1
            if ($LASTEXITCODE -eq 0) {
                $success = $true
            } else {
                $retryCount++
                if ($retryCount -lt $maxRetries) {
                    Write-Host "⚠ Build blocked. Retrying in 2s..." -ForegroundColor Yellow
                    taskkill /F /IM shepherd.exe /T 2>$null
                    Start-Sleep -Seconds 2
                }
            }
        }

        if ($success) {
            Write-Host "✔ Build successful." -ForegroundColor Green
            # Copy to current dir for easy access
            Copy-Item "$env:CARGO_TARGET_DIR\release\shepherd.exe" ".\shepherd.exe" -Force
            Start-Process powershell -ArgumentList "-NoExit", "-Command", ".\shepherd.exe daemon"
            Start-Sleep -Seconds 1
            .\shepherd.exe tui
        } else {
            Write-Host "✗ Build failed. Please close your IDE or Antivirus." -ForegroundColor Red
        }
    }
}
else {
    Write-Host "✗ Error: No binary found and cannot build from source." -ForegroundColor Red
    Write-Host "Please ensure 'shepherd.exe' is in this folder or 'cargo' is installed." -ForegroundColor Gray
    Write-Host "`nIf you need to install Rust, visit: https://rustup.rs" -ForegroundColor Cyan
}

Write-Host "`nPress any key to exit..."
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
