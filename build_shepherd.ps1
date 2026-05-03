# build_shepherd.ps1
# 🐑 cargo-shepherd Build Script

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$rootDir = if (Test-Path (Join-Path $scriptDir "Cargo.toml")) {
    $scriptDir
} else {
    Split-Path -Parent $scriptDir
}

Push-Location $rootDir

Write-Host "🐑 Building cargo-shepherd..." -ForegroundColor Cyan

$success = $false
$maxRetries = 2
$retryCount = 0

while ($retryCount -lt $maxRetries -and -not $success) {
    $output = cargo build --bin shepherd 2>&1
    
    if ($LASTEXITCODE -eq 0) {
        $success = $true
    } else {
        $retryCount++
        if ($retryCount -lt $maxRetries) {
            Write-Host "⚠ Build blocked. Cleaning and retrying..." -ForegroundColor Yellow
            Get-Process shepherd -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Seconds 2
        }
    }
}

if (-not $success) {
    Write-Host "✗ Build failed." -ForegroundColor Red
    Pop-Location
    exit 1
}

Write-Host "✓ Build successful" -ForegroundColor Green

# Copy exe to root
if (Test-Path "target\debug\shepherd.exe") {
    Copy-Item "target\debug\shepherd.exe" "shepherd.exe" -Force
    Write-Host "✓ Copied shepherd.exe to root folder" -ForegroundColor Green
}

Pop-Location
