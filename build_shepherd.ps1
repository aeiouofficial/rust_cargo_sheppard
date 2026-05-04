# build_shepherd.ps1
# cargo-shepherd Build Script

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoDir = if (Test-Path (Join-Path $scriptDir "Cargo.toml")) {
    $scriptDir
} else {
    Split-Path -Parent $scriptDir
}

Push-Location $repoDir

Write-Host "Building cargo-shepherd..." -ForegroundColor Cyan

$randomId = Get-Random -Minimum 1000 -Maximum 9999
$env:CARGO_TARGET_DIR = "target_tmp_$randomId"
$builtExe = Join-Path $repoDir "$env:CARGO_TARGET_DIR\debug\shepherd.exe"
$rootExe = Join-Path $repoDir "shepherd.exe"

Write-Host "Using isolated target: $env:CARGO_TARGET_DIR" -ForegroundColor DarkGray

$success = $false
$maxRetries = 2
$retryCount = 0

while ($retryCount -lt $maxRetries -and -not $success) {
    cargo build --bin shepherd -j 1

    if ($LASTEXITCODE -eq 0) {
        $success = $true
    } else {
        $retryCount++
        if ($retryCount -lt $maxRetries) {
            Write-Host "Build blocked. Trying one more time..." -ForegroundColor Yellow
            Get-Process shepherd -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Seconds 2
        }
    }
}

if (-not $success) {
    Write-Host "Build failed." -ForegroundColor Red
    Pop-Location
    exit 1
}

Write-Host "Build successful" -ForegroundColor Green

if (Test-Path $builtExe) {
    try {
        Copy-Item $builtExe $rootExe -Force -ErrorAction Stop
        Write-Host "Copied shepherd.exe to root folder" -ForegroundColor Green
    } catch {
        Write-Host "Built successfully, but shepherd.exe is locked. Use: $builtExe" -ForegroundColor Yellow
    }
}

Pop-Location
