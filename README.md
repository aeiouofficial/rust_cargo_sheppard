# 🐑 cargo-shepherd

<img width="1536" height="1024" alt="aa6d2fbc-f767-49bd-b4a7-dca177b4b5d1" src="https://github.com/user-attachments/assets/98f04c4c-9ae5-4a6d-8473-d36e4635c645" />

<img width="969" height="548" alt="image" src="https://github.com/user-attachments/assets/5e487214-8e39-4d26-a9bb-08bf97ff9d42" />

> **A system-wide Cargo build coordinator for Windows 10/11, macOS, and Linux.**  
> Prevents multiple Rust projects open in parallel VSCode windows from fighting each other, maxing out your CPU/RAM, and deadlocking on Cargo's file locks — with a full interactive TUI dashboard and persistent per-project configuration.

---

[![Built with Rust](https://img.shields.io/badge/Built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)](https://github.com/)
[![Built With Ratatui](https://img.shields.io/badge/Built_With_Ratatui-000?logo=ratatui&logoColor=fff)](https://ratatui.rs/)

---

## The Problem

You open four Rust projects in VSCode. You hit save in one. Everything dies.

```
Blocking waiting for file lock on package cache...
Blocking waiting for file lock on build directory...
[CPU: 100%]   [RAM: 14.2 GB / 16 GB]   [VSCode frozen]
```

This is a **missing infrastructure problem**, not a bug in your code. Cargo has zero cross-project build coordination by design. Every VSCode window spawns its own `rust-analyzer` instance, which fires `cargo check` on every save. With four projects open, four processes fight for the global `.cargo/registry` lock — a deadlock cascade that saturates your system.

**cargo-shepherd is that missing coordination layer.**

---

## How It Works

```
  WITHOUT shepherd                    WITH shepherd
  ──────────────────                  ─────────────────────
  
  [Project A] ──→ cargo check ──┐     [Project A] ──→ shepherd run ──┐
  [Project B] ──→ cargo check ──┼──→  [Project B] ──→ shepherd run ──┼──→ [ DAEMON ]
  [Project C] ──→ cargo check ──┤     [Project C] ──→ shepherd run ──┘        │
  [Project D] ──→ cargo run    ─┘                                     ┌────────▼──────────┐
         │                                                             │  Priority Queue   │
         ▼                                                             │  Slot Semaphore   │
  ┌─────────────────┐                                                  └────────┬──────────┘
  │  .cargo LOCK    │                                                           │
  │  CONFLICT       │                                             Slot 1: [A] cargo check ✓
  │  CPU: 100%      │                                             Slot 2: [B] cargo check ✓
  │  RAM: 💀        │                                             Slot 3: waiting...
  └─────────────────┘                                             [D] cargo run → CRITICAL priority
                                                                  → jumps to front of queue
```

The daemon runs a **true priority scheduler**: five priority levels (Background → Low → Normal → High → Critical), FIFO ordering within the same priority, and live reprioritization of queued jobs — all configurable per-project and persisted to disk.

---

## Features

| Feature | Description |
|---------|-------------|
| **Priority queue** | 5 levels (Background/Low/Normal/High/Critical), FIFO within tier |
| **Live reprioritization** | Change a queued job's priority without cancelling it |
| **TUI dashboard** | Interactive ratatui dashboard — see everything, control everything |
| **Persistent config** | Per-project defaults saved to platform-correct config paths |
| **Project aliases** | Short display names instead of raw paths |
| **Kill/cancel** | Kill a specific job, cancel a queued one, or clear an entire project |
| **Live slot control** | Change concurrent build count without restarting the daemon |
| **Resource gating** | Daemon pauses scheduling if CPU or RAM headroom is low |
| **sccache integration** | Pairs with Mozilla's compiler cache to share artifacts across projects |
| **Cross-platform** | Windows (Named Pipes), macOS/Linux (Unix Sockets) — same binary |


<img width="1254" height="334" alt="taskbar_preview" src="https://github.com/user-attachments/assets/34e4ac6d-0812-45e6-9c8d-5b7c5075c91d" />

---

## Quick Start

```bash
# 1. Install
cargo install --path .

# 2. Start the daemon (keep this open)
shepherd daemon --slots 3

# 3. In any project terminal, use shepherd instead of bare cargo
shepherd run -- build
shepherd run -- run --release

# 4. Open the TUI dashboard
shepherd tui

# 5. See all active builds from the CLI
shepherd status

if thats too complicated for whatever reason for you, you also can use the bats:
All paths are RELATIVE — works when downloaded on any PC/drive (no hardcoded paths).

Workflow for sheps:

First time: double-click bin/build_shepherd.bat

when download + build finished = .exe gets created (you dont start the app with it)
--> start start_shepherd.bat to launch the tui.
```

---

## TUI Dashboard

```
shepherd tui
```

Interactive controls allow you to manage the build queue in real-time. Use `Tab` to switch between Running and Queued panels. Use `+`/`-` to change priority of queued jobs.

---

## CLI Reference

### Core Commands

- `shepherd daemon [--slots N]` : Start the coordinator daemon
- `shepherd run [--dir <path>] [--priority <level>] -- <cargo args>` : Queue a command
- `shepherd tui` : Interactive dashboard
- `shepherd status` : Print status table
- `shepherd kill --project <dir>` : Kill all builds for a project
- `shepherd kill --job <id>` : Kill a specific job
- `shepherd stop` : Shut down the daemon

### Config Commands

- `shepherd config show` : Print current config as TOML
- `shepherd config slots <N>` : Set concurrent build slots
- `shepherd config priority [--dir <path>] <level>` : Set default priority
- `shepherd config alias [--dir <path>] <name>` : Set display alias
- `shepherd config child-jobs [--dir <path>] <N>` : Set per-project thread count

---

## Branding Assets

High-fidelity branding assets are located in the `assets/` directory:
- `assets/app_icon.png` : High-resolution rounded square logo.
- `assets/taskbar_icon.png` : Small-footprint taskbar/dock icon.

---

## Architecture

The project is structured for modularity and cross-platform compatibility:

- `src/main.rs` : CLI entry and subcommand handling.
- `src/daemon.rs` : The core scheduler and IPC server.
- `src/client.rs` : Shared IPC client for CLI and TUI.
- `src/ipc.rs` : Communication protocol and platform-specific transport (Named Pipes/Unix Sockets).
- `src/queue.rs` : Priority queue implementation with FIFO tie-breaking.
- `src/config.rs` : Persistent configuration management.
- `src/monitor.rs` : System resource monitoring for schedule gating.
- `src/runner.rs` : Cargo process management and output streaming.
- `src/tui.rs` : Ratatui-based dashboard.

---
## License

MIT - see [LICENSE](LICENSE)
