# start_shepherd.ps1
# 🐑 cargo-shepherd Startup Script

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoDir = if (Test-Path (Join-Path $scriptDir "Cargo.toml")) {
    $scriptDir
} else {
    Split-Path -Parent $scriptDir
}
Push-Location $repoDir

Write-Host "🐑 Initializing cargo-shepherd..." -ForegroundColor Cyan

# 1. Ensure dependencies are built
Write-Host "Checking for build locks and compiling..." -ForegroundColor Gray

# NUCLEAR OPTION: Use a randomized target directory to bypass persistent OS file locks
$randomId = Get-Random -Minimum 1000 -Maximum 9999
$env:CARGO_TARGET_DIR = "target_tmp_$randomId"

Write-Host "Using isolated target: $env:CARGO_TARGET_DIR" -ForegroundColor DarkGray

$maxRetries = 2
$retryCount = 0
$success = $false

while ($retryCount -lt $maxRetries -and -not $success) {
    cargo build --bin shepherd -j 1
    
    if ($LASTEXITCODE -eq 0) {
        $success = $true
    } else {
        $retryCount++
        if ($retryCount -lt $maxRetries) {
            Write-Host "⚠ Build blocked. Trying one more time..." -ForegroundColor Yellow
            taskkill /F /IM shepherd.exe /T 2>$null
            Start-Sleep -Seconds 2
        }
    }
}

if (-not $success) {
    Write-Host "`n✗ Build failed. Please close VS Code / Producer.AI and try again." -ForegroundColor Red
    Pop-Location
    exit 1
}

# Move binary to root for easier access if possible. If an existing daemon has
# locked shepherd.exe, run the freshly built isolated-target executable instead.
$builtExe = Join-Path $repoDir "$env:CARGO_TARGET_DIR\debug\shepherd.exe"
$rootExe = Join-Path $repoDir "shepherd.exe"
$shepherdExe = $rootExe
if (Test-Path $builtExe) {
    try {
        Copy-Item $builtExe $rootExe -Force -ErrorAction Stop
    } catch {
        Write-Host "⚠ Could not replace shepherd.exe; using isolated build for this session." -ForegroundColor Yellow
        $shepherdExe = $builtExe
    }
}

# 2. Start the daemon silently in the background.
#    SHEPHERD_SLOTS=0 means unlimited job-level slots; resource gates still apply.
$slots = if ([string]::IsNullOrWhiteSpace($env:SHEPHERD_SLOTS)) { "0" } else { $env:SHEPHERD_SLOTS }
if ($slots -notmatch '^\d+$') {
    Write-Host "✗ SHEPHERD_SLOTS must be 0 (unlimited) or a positive integer." -ForegroundColor Red
    Pop-Location
    exit 1
}

$slotLabel = if ($slots -eq "0") { "unlimited" } else { $slots }
Write-Host "Starting daemon silently in background (slots: $slotLabel)..." -ForegroundColor Green
Start-Process -FilePath $shepherdExe -ArgumentList @("daemon", "--slots", $slots) -WorkingDirectory $repoDir -WindowStyle Hidden

# 3. Wait a moment for IPC to initialize
Start-Sleep -Seconds 2

# 4. Launch the TUI in this window
Write-Host "Launching TUI Dashboard..." -ForegroundColor Yellow
& $shepherdExe tui

Pop-Location
