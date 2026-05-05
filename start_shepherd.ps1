# start_shepherd.ps1
# cargo-shepherd startup script

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoDir = if (Test-Path (Join-Path $scriptDir "Cargo.toml")) {
    $scriptDir
} else {
    Split-Path -Parent $scriptDir
}
Push-Location $repoDir

try {
    function Resolve-RealCargoPath {
        param(
            [string]$ShimDir,
            [string]$ShimExe
        )

        if (-not [string]::IsNullOrWhiteSpace($env:SHEPHERD_REAL_CARGO) -and (Test-Path $env:SHEPHERD_REAL_CARGO)) {
            return (Resolve-Path $env:SHEPHERD_REAL_CARGO).Path
        }

        $shimResolved = if (Test-Path $ShimExe) { (Resolve-Path $ShimExe).Path } else { $null }
        $shimDirResolved = if (Test-Path $ShimDir) { (Resolve-Path $ShimDir).Path } else { $null }

        foreach ($entry in ($env:PATH -split ';')) {
            if ([string]::IsNullOrWhiteSpace($entry)) { continue }
            $candidate = Join-Path $entry "cargo.exe"
            if (-not (Test-Path $candidate)) { continue }

            $resolved = (Resolve-Path $candidate).Path
            $parent = Split-Path -Parent $resolved
            if ($shimResolved -and $resolved -ieq $shimResolved) { continue }
            if ($shimDirResolved -and $parent -ieq $shimDirResolved) { continue }
            return $resolved
        }

        return $null
    }

    Write-Host "Initializing cargo-shepherd..." -ForegroundColor Cyan

    $packagedExe = Join-Path $repoDir "bin\shepherd.exe"
    $verifiedExe = Join-Path $repoDir "target_verify_herding_0505\release\shepherd.exe"
    $rootExe = Join-Path $repoDir "shepherd.exe"
    $shepherdExe = $null

    if (Test-Path $rootExe) {
        $shepherdExe = $rootExe
    } elseif (Test-Path $packagedExe) {
        $shepherdExe = $packagedExe
    } elseif (Test-Path $verifiedExe) {
        $shepherdExe = $verifiedExe
    }

    $sourceFiles = @(
        Join-Path $repoDir "Cargo.toml"
        Join-Path $repoDir "build.rs"
        Join-Path $repoDir "assets\app_icon.ico"
    )
    if (Test-Path (Join-Path $repoDir "src")) {
        $sourceFiles += Get-ChildItem (Join-Path $repoDir "src") -Recurse -File | Select-Object -ExpandProperty FullName
    }
    $latestSource = $sourceFiles |
        Where-Object { Test-Path $_ } |
        ForEach-Object { (Get-Item $_).LastWriteTimeUtc } |
        Sort-Object -Descending |
        Select-Object -First 1
    $sourceNewerThanExe = $false
    if (-not [string]::IsNullOrWhiteSpace($shepherdExe) -and $latestSource) {
        $sourceNewerThanExe = $latestSource -gt (Get-Item $shepherdExe).LastWriteTimeUtc
    }

    $forceBuild = $env:SHEPHERD_FORCE_BUILD -eq "1" -or [string]::IsNullOrWhiteSpace($shepherdExe) -or $sourceNewerThanExe
    if ($forceBuild) {
        Write-Host "Building cargo-shepherd..." -ForegroundColor Gray

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
                    Write-Host "Build blocked. Trying one more time..." -ForegroundColor Yellow
                    taskkill /F /IM shepherd.exe /T 2>$null
                    Start-Sleep -Seconds 2
                }
            }
        }

        if (-not $success) {
            Write-Host "`nBuild failed. Close other Cargo/Rust Analyzer builds or run again later." -ForegroundColor Red
            exit 1
        }

        $builtExe = Join-Path $repoDir "$env:CARGO_TARGET_DIR\debug\shepherd.exe"
        if (-not (Test-Path $builtExe)) {
            Write-Host "Build finished but shepherd.exe was not produced." -ForegroundColor Red
            exit 1
        }
        $shepherdExe = $builtExe
        try {
            Copy-Item $builtExe $rootExe -Force -ErrorAction Stop
            $shepherdExe = $rootExe
        } catch {
            Write-Host "Built successfully, but root shepherd.exe is locked; using isolated build." -ForegroundColor Yellow
        }
    } else {
        Write-Host "Using $shepherdExe. Set SHEPHERD_FORCE_BUILD=1 to rebuild." -ForegroundColor DarkGray
    }

    $shimDir = Join-Path $repoDir "bin\shim"
    $cargoShim = Join-Path $shimDir "cargo.exe"
    New-Item -ItemType Directory -Path $shimDir -Force | Out-Null
    $realCargo = Resolve-RealCargoPath -ShimDir $shimDir -ShimExe $cargoShim
    if ([string]::IsNullOrWhiteSpace($realCargo)) {
        Write-Host "Could not find the real cargo.exe before installing the shim. Set SHEPHERD_REAL_CARGO manually." -ForegroundColor Red
        exit 1
    }

    try {
        Copy-Item $shepherdExe $cargoShim -Force -ErrorAction Stop
    } catch {
        Write-Host "Could not refresh $cargoShim. Close any active shimmed cargo process and try again." -ForegroundColor Red
        exit 1
    }

    $env:SHEPHERD_REAL_CARGO = $realCargo
    $pathEntries = @($env:PATH -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if (-not ($pathEntries | Where-Object { $_ -ieq $shimDir })) {
        $env:PATH = "$shimDir;$env:PATH"
    }
    Write-Host "Cargo shim ready: $cargoShim -> $realCargo" -ForegroundColor DarkGray
    Write-Host "Passive Rust herding ready: unmanaged cargo/rustc trees are monitored and suspended instead of killed when gates close." -ForegroundColor DarkGray

    if ($env:SHEPHERD_INSTALL_USER_SHIM -eq "1") {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $userEntries = @($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and $_ -ine $shimDir })
        [Environment]::SetEnvironmentVariable("Path", (($shimDir, $userEntries) -join ';'), "User")
        [Environment]::SetEnvironmentVariable("SHEPHERD_REAL_CARGO", $realCargo, "User")
        Write-Host "Installed shim into the user PATH for newly opened terminals." -ForegroundColor Green
    }

    $slots = if ([string]::IsNullOrWhiteSpace($env:SHEPHERD_SLOTS)) { "0" } else { $env:SHEPHERD_SLOTS }
    if ($slots -notmatch '^\d+$') {
        Write-Host "SHEPHERD_SLOTS must be 0 (unlimited) or a positive integer." -ForegroundColor Red
        exit 1
    }

    $slotLabel = if ($slots -eq "0") { "unlimited" } else { $slots }

    & $shepherdExe status *> $null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "Stopping existing daemon so Sheppard can take coordinator position..." -ForegroundColor Yellow
        & $shepherdExe stop *> $null
        Start-Sleep -Seconds 1
    }

    Write-Host "Starting daemon silently in background (slots: $slotLabel)..." -ForegroundColor Green
    $previousDaemonTray = $env:SHEPHERD_DAEMON_TRAY
    $env:SHEPHERD_DAEMON_TRAY = "0"
    Start-Process -FilePath $shepherdExe -ArgumentList @("daemon", "--slots", $slots) -WorkingDirectory $repoDir -WindowStyle Hidden
    if ($null -eq $previousDaemonTray) {
        Remove-Item Env:SHEPHERD_DAEMON_TRAY -ErrorAction SilentlyContinue
    } else {
        $env:SHEPHERD_DAEMON_TRAY = $previousDaemonTray
    }

    $ready = $false
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        Start-Sleep -Seconds 1
        & $shepherdExe status *> $null
        if ($LASTEXITCODE -eq 0) {
            $ready = $true
            break
        }
    }

    if (-not $ready) {
        Write-Host "Daemon did not become ready. Run '$shepherdExe daemon --slots $slots' to see the error." -ForegroundColor Red
        exit 1
    }

    Write-Host "Launching TUI Dashboard..." -ForegroundColor Yellow
    & $shepherdExe tui
} finally {
    Pop-Location
}
