@echo off
setlocal enabledelayedexpansion

title cargo-shepherd
set "SCRIPT_DIR=%~dp0"
set "REPO_DIR=%SCRIPT_DIR%"
if not exist "%REPO_DIR%Cargo.toml" set "REPO_DIR=%SCRIPT_DIR%..\"
cd /d "%REPO_DIR%"

if "%SHEPHERD_SLOTS%"=="" set "SHEPHERD_SLOTS=0"
set "SHEPHERD_BAD_SLOTS="
for /f "delims=0123456789" %%A in ("%SHEPHERD_SLOTS%") do set "SHEPHERD_BAD_SLOTS=1"
if defined SHEPHERD_BAD_SLOTS (
	echo [ERROR] SHEPHERD_SLOTS must be 0 ^(unlimited^) or a positive integer.
	pause
	exit /b 1
)

set "SHEPHERD_SLOT_LABEL=%SHEPHERD_SLOTS%"
if "%SHEPHERD_SLOTS%"=="0" set "SHEPHERD_SLOT_LABEL=unlimited"

set "PACKAGED_EXE=%REPO_DIR%bin\shepherd.exe"
set "VERIFY_EXE=%REPO_DIR%target_verify_exitcode_0504\debug\shepherd.exe"
set "ROOT_EXE=%REPO_DIR%shepherd.exe"
set "SHEPHERD_EXE="

if exist "%ROOT_EXE%" set "SHEPHERD_EXE=%ROOT_EXE%"
if not defined SHEPHERD_EXE if exist "%PACKAGED_EXE%" set "SHEPHERD_EXE=%PACKAGED_EXE%"
if not defined SHEPHERD_EXE if exist "%VERIFY_EXE%" set "SHEPHERD_EXE=%VERIFY_EXE%"
if not defined SHEPHERD_EXE set "SHEPHERD_FORCE_BUILD=1"
if not "%SHEPHERD_FORCE_BUILD%"=="1" if defined SHEPHERD_EXE (
	powershell -NoProfile -ExecutionPolicy Bypass -Command "$root=$env:REPO_DIR; $exe=$env:SHEPHERD_EXE; $sources=@(); if (Test-Path (Join-Path $root 'src')) { $sources += Get-ChildItem (Join-Path $root 'src') -Recurse -File }; foreach ($path in @('Cargo.toml','build.rs','assets\app_icon.ico')) { $full=Join-Path $root $path; if (Test-Path $full) { $sources += Get-Item $full } }; $latest=($sources | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1).LastWriteTimeUtc; if ($latest -and $latest -gt (Get-Item $exe).LastWriteTimeUtc) { exit 11 }"
	if !ERRORLEVEL! GEQ 11 set "SHEPHERD_FORCE_BUILD=1"
)

if "%SHEPHERD_FORCE_BUILD%"=="1" (
	set "CARGO_TARGET_DIR=target_tmp_%RANDOM%"
	set "BUILT_EXE=%CD%\!CARGO_TARGET_DIR!\debug\shepherd.exe"

	echo Building cargo-shepherd with isolated target: !CARGO_TARGET_DIR!
	cargo build --bin shepherd -j 1
	if not "!ERRORLEVEL!"=="0" (
		echo [WARN] Build blocked. Trying one more time...
		taskkill /F /IM shepherd.exe /T >nul 2>&1
		timeout /t 2 /nobreak >nul
		cargo build --bin shepherd -j 1
	)

	if not "!ERRORLEVEL!"=="0" (
		echo [ERROR] Build failed.
		pause
		exit /b 1
	)

	if exist "!BUILT_EXE!" (
		set "SHEPHERD_EXE=!BUILT_EXE!"
		copy /Y "!BUILT_EXE!" "%ROOT_EXE%" >nul 2>&1
		if not errorlevel 1 set "SHEPHERD_EXE=%ROOT_EXE%"
	) else (
		echo [ERROR] Build finished but shepherd.exe was not produced.
		pause
		exit /b 1
	)
) else (
	echo Using %SHEPHERD_EXE%. Set SHEPHERD_FORCE_BUILD=1 to rebuild.
)

set "SHEPHERD_SHIM_DIR=%REPO_DIR%bin\shim"
set "SHEPHERD_CARGO_SHIM=%SHEPHERD_SHIM_DIR%\cargo.exe"
if not exist "%SHEPHERD_SHIM_DIR%" mkdir "%SHEPHERD_SHIM_DIR%" >nul 2>&1

for /f "usebackq delims=" %%A in (`powershell -NoProfile -ExecutionPolicy Bypass -Command "$shimDir=$env:SHEPHERD_SHIM_DIR; $shimExe=$env:SHEPHERD_CARGO_SHIM; $configured=$env:SHEPHERD_REAL_CARGO; if ($configured -and (Test-Path $configured)) { (Resolve-Path $configured).Path; exit 0 }; $shimResolved=if(Test-Path $shimExe){(Resolve-Path $shimExe).Path}else{$null}; $shimDirResolved=if(Test-Path $shimDir){(Resolve-Path $shimDir).Path}else{$null}; foreach($entry in ($env:PATH -split ';')){ if([string]::IsNullOrWhiteSpace($entry)){continue}; $candidate=Join-Path $entry 'cargo.exe'; if(-not(Test-Path $candidate)){continue}; $resolved=(Resolve-Path $candidate).Path; $parent=Split-Path -Parent $resolved; if($shimResolved -and $resolved -ieq $shimResolved){continue}; if($shimDirResolved -and $parent -ieq $shimDirResolved){continue}; $resolved; exit 0 }; exit 1"`) do set "SHEPHERD_REAL_CARGO=%%A"
if not defined SHEPHERD_REAL_CARGO (
	echo [ERROR] Could not find the real cargo.exe before installing the shim. Set SHEPHERD_REAL_CARGO manually.
	pause
	exit /b 1
)

copy /Y "%SHEPHERD_EXE%" "%SHEPHERD_CARGO_SHIM%" >nul 2>&1
if errorlevel 1 (
	echo [ERROR] Could not refresh "%SHEPHERD_CARGO_SHIM%". Close any active shimmed cargo process and try again.
	pause
	exit /b 1
)
set "PATH=%SHEPHERD_SHIM_DIR%;%PATH%"
echo Cargo shim ready: %SHEPHERD_CARGO_SHIM% -^> %SHEPHERD_REAL_CARGO%

if "%SHEPHERD_INSTALL_USER_SHIM%"=="1" (
	powershell -NoProfile -ExecutionPolicy Bypass -Command "$shimDir=$env:SHEPHERD_SHIM_DIR; $realCargo=$env:SHEPHERD_REAL_CARGO; $userPath=[Environment]::GetEnvironmentVariable('Path','User'); $entries=@($userPath -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and $_ -ine $shimDir }); [Environment]::SetEnvironmentVariable('Path', (($shimDir, $entries) -join ';'), 'User'); [Environment]::SetEnvironmentVariable('SHEPHERD_REAL_CARGO', $realCargo, 'User')"
	echo Installed shim into the user PATH for newly opened terminals.
)

"%SHEPHERD_EXE%" status >nul 2>&1
if "!ERRORLEVEL!"=="0" (
	echo Stopping existing daemon so Sheppard can take coordinator position...
	"%SHEPHERD_EXE%" stop >nul 2>&1
	timeout /t 1 /nobreak >nul
)

echo Starting cargo-shepherd daemon silently ^(slots: %SHEPHERD_SLOT_LABEL%^)...
powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "$wd=(Get-Location).Path; $exe=$env:SHEPHERD_EXE; $slots=$env:SHEPHERD_SLOTS; Start-Process -FilePath $exe -ArgumentList @('daemon','--slots',$slots) -WorkingDirectory $wd -WindowStyle Hidden"
for /l %%I in (1,1,10) do (
	"%SHEPHERD_EXE%" status >nul 2>&1
	if "!ERRORLEVEL!"=="0" goto daemon_ready
	timeout /t 1 /nobreak >nul
)
echo [ERROR] Daemon did not become ready. Run "%SHEPHERD_EXE%" daemon --slots %SHEPHERD_SLOTS% to see the error.
pause
exit /b 1

:daemon_ready

echo.
echo Launching TUI Dashboard...
"%SHEPHERD_EXE%" tui

pause
