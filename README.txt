================================================================================
  CARGO-SHEPHERD v0.2.0
  System-Wide Rust Build Coordinator
  MIT License | Windows 11 / macOS / Linux
================================================================================

WHAT IS THIS?
─────────────
cargo-shepherd is a background daemon that coordinates all Cargo (Rust) builds
across every project open on your machine simultaneously. It prevents them from
fighting over shared file locks, saturating your CPU and RAM, and deadlocking
rust-analyzer when you have multiple VSCode windows open.

It includes:
  - A priority queue scheduler (5 levels, live reprioritization)
  - A full interactive TUI dashboard (ratatui)
  - Persistent per-project config (aliases, default priority, thread counts)
  - CLI for every operation
  - Cross-platform (Windows 11, macOS, Linux — same binary)


THE PROBLEM THIS SOLVES
───────────────────────
Open 4 Rust projects in VSCode. Hit save.

  "Blocking waiting for file lock on package cache..."
  [CPU: 100%]  [RAM: maxed]  [VSCode frozen]

This is not a bug in your code. Cargo has no cross-project coordination.
Each VSCode window runs rust-analyzer, which runs "cargo check" on every save,
and each cargo check tries to acquire the global .cargo/registry file lock.
Four windows = four processes in a deadlock cascade.

cargo-shepherd is the missing coordination layer.


QUICK START (WINDOWS 11)
────────────────────────
  1. git clone https://github.com/YOUR_USERNAME/cargo-shepherd.git
  2. cd cargo-shepherd
  3. cargo install --path .
  4. shepherd daemon --slots 3          (keep this terminal open)
  5. shepherd run -- build              (in any project terminal)
  6. shepherd tui                       (open the live dashboard)


COMMANDS
────────
  shepherd daemon [--slots N]           Start the coordinator daemon
  shepherd tui                          Open interactive TUI dashboard
  shepherd status                       Print status to terminal
  shepherd run [--priority <P>] -- ...  Queue a cargo command
  shepherd kill --project <dir>         Kill all builds for a project
  shepherd kill --job <id>              Kill a specific job
  shepherd cancel <job_id>              Cancel a queued job (pre-start)
  shepherd reprioritize <id> <level>    Change queued job's priority live
  shepherd config show                  Print current config as TOML
  shepherd config slots <N>             Set concurrent build count (live)
  shepherd config priority [--dir] <P>  Set default priority for a project
  shepherd config alias [--dir] <name>  Set display name for a project
  shepherd config child-jobs [--dir] N  Set rustc thread count for project
  shepherd config-path                  Print config file location
  shepherd stop                         Shut down the daemon


PRIORITY LEVELS
───────────────
  critical   Jumps to the front — use for cargo run
  high       Runs before normal and below
  normal     Default for all projects
  low        Runs after normal and above
  background Only runs when all slots are otherwise idle


TUI DASHBOARD KEYBINDINGS
─────────────────────────
  j / Down    Select next item
  k / Up      Select previous item
  Tab         Switch between RUNNING and QUEUE panels
  +           Raise priority of selected queued job
  -           Lower priority of selected queued job
  x           Kill selected job
  c           Cancel selected queued job
  X           Kill ALL jobs for the selected job's project
  s           Set slot count (opens input prompt)
  a           Set project alias (opens input prompt)
  r           Force refresh
  q / Esc     Quit TUI (daemon keeps running)


CONFIG FILE
───────────
Location: run "shepherd config-path" to print the exact path.

  Windows : %APPDATA%\shepherd\config.toml
  macOS   : ~/Library/Application Support/shepherd/config.toml
  Linux   : ~/.config/shepherd/config.toml

Example:
  slots         = 3
  max_cpu_pct   = 80.0
  max_ram_pct   = 85.0
  child_jobs    = 2
  log_level     = "info"
  ui_refresh_ms = 500

  [[projects]]
  path       = "/home/user/my-saas"
  alias      = "my-saas"
  priority   = "high"
  child_jobs = 4

  [[projects]]
  path     = "/home/user/utils"
  alias    = "utils"
  priority = "background"


AUTO-START ON WINDOWS 11 (PowerShell as Administrator)
──────────────────────────────────────────────────────
  $action  = New-ScheduledTaskAction `
               -Execute "$env:USERPROFILE\.cargo\bin\shepherd.exe" `
               -Argument "daemon --slots 3"
  $trigger = New-ScheduledTaskTrigger -AtLogOn
  $settings= New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0
  Register-ScheduledTask -TaskName "cargo-shepherd" `
    -Action $action -Trigger $trigger `
    -Settings $settings -RunLevel Highest -Force


AUTO-START ON LINUX (systemd)
─────────────────────────────
  Create: ~/.config/systemd/user/cargo-shepherd.service

    [Unit]
    Description=cargo-shepherd
    After=default.target

    [Service]
    ExecStart=%h/.cargo/bin/shepherd daemon --slots 3
    Restart=on-failure

    [Install]
    WantedBy=default.target

  Then: systemctl --user enable --now cargo-shepherd


RECOMMENDED COMPANION: sccache
────────────────────────────────
cargo-shepherd controls WHEN builds run (concurrency coordination).
sccache controls WHETHER they recompile at all (shared artifact cache).
Together they eliminate both interference and redundant compilation.

  cargo install sccache --locked

  Add to ~/.cargo/config.toml:
    [build]
    jobs = 4
    rustc-wrapper = "sccache"

    [profile.dev]
    incremental = false


RECOMMENDED VSCODE SETTINGS (per project .vscode/settings.json)
────────────────────────────────────────────────────────────────
  {
    "rust-analyzer.checkOnSave": true,
    "rust-analyzer.check.extraArgs": ["--jobs=2"],
    "rust-analyzer.server.extraEnv": { "RA_WORKER_THREADS": "2" },
    "rust-analyzer.cargo.targetDir": true
  }


PROJECT STRUCTURE
─────────────────
  src/main.rs      CLI (clap subcommands, colored output)
  src/daemon.rs    Async daemon, scheduler loop, runner pool
  src/client.rs    Socket/Pipe client shared by CLI and TUI
  src/ipc.rs       Protocol types (JSON over Pipe/Socket)
  src/queue.rs     Priority queue with FIFO tiebreak
  src/config.rs    Persistent TOML config, per-project settings
  src/monitor.rs   CPU/RAM resource gate (sysinfo)
  src/runner.rs    cargo child process spawner
  src/tui.rs       Ratatui 0.29 interactive dashboard


PLATFORM COMPATIBILITY
──────────────────────
Windows:       Named Pipes (\\.\pipe\cargo-shepherd)
macOS/Linux:   Unix Domain Sockets (/tmp/cargo-shepherd.sock)

Native support on all current OS versions. No WSL or external setup required.


WHY I BUILT THIS
────────────────
Working across 4 Rust projects simultaneously in VSCode, I kept hitting
the same wall: the system became completely unresponsive because
rust-analyzer in each window was running cargo check concurrently, all
competing for Cargo's global file lock.

sccache helps with caching. cargo --jobs caps threads within one build.
But nothing coordinates across multiple simultaneous projects at the
system level. cargo-shepherd is that missing piece.


DEPENDENCIES
────────────
  tokio                Async runtime
  ratatui              TUI rendering
  crossterm            Terminal I/O, event stream
  serde / serde_json   IPC protocol serialization
  clap                 CLI argument parsing
  sysinfo              CPU/RAM monitoring
  toml                 Config file format
  dirs                 Platform config paths
  anyhow               Error handling
  tracing              Structured logging
  uuid                 Job IDs
  chrono               Timestamps
  colored              Terminal color output
  futures              Stream trait

All pure Rust. No C libraries. No Docker. No external services.


LICENSE
───────
MIT License. Free to use, modify, and distribute.
See LICENSE file for full terms.


LINKS
─────
  GitHub:  https://github.com/YOUR_USERNAME/cargo-shepherd
  Issues:  https://github.com/YOUR_USERNAME/cargo-shepherd/issues

================================================================================
  Built with Rust. Tested on Windows 11, Ubuntu 24.04, macOS Sonoma.
================================================================================
