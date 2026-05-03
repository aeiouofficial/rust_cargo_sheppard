// src/ipc.rs
// IPC protocol for cargo-shepherd.
// Transport: newline-delimited JSON over a local socket.
// Socket location is chosen at runtime (cross-platform).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use crate::config::Priority;

// ─────────────────────────── Socket path ─────────────────────────────────────

/// Returns the platform-correct socket path (used on Unix).
///   Unix    : /tmp/cargo-shepherd.sock
///   Windows : %TEMP%\cargo-shepherd.sock  (used by client for Unix socket fallback)
pub fn socket_path() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir().join("cargo-shepherd.sock")
    } else {
        PathBuf::from("/tmp/cargo-shepherd.sock")
    }
}

/// Returns the Windows named pipe name.
/// Used by the daemon and client on Windows.
#[cfg(windows)]
pub fn pipe_name() -> String {
    r"\\.\pipe\cargo-shepherd".to_string()
}

// ─────────────────────────── Messages ────────────────────────────────────────

/// Client → Daemon
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    // ── Build lifecycle ───────────────────────────────────────────────────────

    /// Queue a new cargo command.
    Run {
        job_id:      String,
        project_dir: String,
        args:        Vec<String>,
        /// Override priority for this specific invocation.
        /// If None, daemon looks up the project's configured default.
        priority:    Option<Priority>,
    },

    // ── Queue manipulation ────────────────────────────────────────────────────

    /// Change the priority of a queued (not yet running) job.
    SetJobPriority { job_id: String, new_priority: Priority },

    /// Cancel a queued job before it starts.
    CancelJob { job_id: String },

    /// Kill all running and queued jobs for a project directory.
    KillProject { project_dir: String },

    /// Kill a specific job by ID (running or queued).
    KillJob { job_id: String },

    // ── Config management (persisted to disk immediately) ─────────────────────

    /// Set the default priority for a project. Saved to config.toml.
    SetProjectPriority { project_dir: String, priority: Priority },

    /// Set a display alias for a project. Saved to config.toml.
    SetProjectAlias { project_dir: String, alias: String },

    /// Change the global slot count live (also saved to config.toml).
    SetSlots { slots: usize },

    /// Set per-project child_jobs (CARGO_BUILD_JOBS). Saved to config.toml.
    SetProjectChildJobs { project_dir: String, child_jobs: usize },

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Get current status (running + queued jobs + resource stats).
    Status,

    /// Get the full current config as TOML text.
    GetConfig,

    // ── Daemon lifecycle ──────────────────────────────────────────────────────
    Shutdown,
}

/// Daemon → Client
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMsg {
    /// Job was accepted and placed in the queue.
    Queued  { job_id: String, position: usize },

    /// A queued job started running.
    Started { job_id: String, pid: u32 },

    /// A running job finished.
    Finished { job_id: String, exit_code: i32, duration_ms: u64 },

    /// A job or set of jobs was killed or cancelled.
    Killed { description: String },

    /// Priority of a queued job was changed.
    PriorityChanged { job_id: String, new_priority: Priority, new_position: usize },

    /// Full status snapshot (response to Status query).
    StatusReport { report: StatusReport },

    /// Config TOML text (response to GetConfig).
    ConfigText { toml: String },

    /// Generic success for config mutations.
    ConfigUpdated { message: String },

    /// Unrecoverable error.
    Error { message: String },

    ShuttingDown,
}

// ─────────────────────────── Status types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusReport {
    pub running:      Vec<RunningJob>,
    pub queued:       Vec<QueuedJobSnapshot>,
    pub slots_total:  usize,
    pub slots_active: usize,
    pub cpu_pct:      f32,
    pub ram_pct:      f64,
}

impl StatusReport {
    /// Returns an empty report (used as TUI default before first fetch).
    pub fn empty() -> Self {
        Self {
            running:      Vec::new(),
            queued:       Vec::new(),
            slots_total:  0,
            slots_active: 0,
            cpu_pct:      0.0,
            ram_pct:      0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningJob {
    pub job_id:      String,
    pub project_dir: String,
    pub alias:       String,
    pub args:        Vec<String>,
    pub pid:         u32,
    pub started_at:  DateTime<Utc>,
    /// Elapsed milliseconds (computed by daemon at snapshot time).
    pub elapsed_ms:  u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedJobSnapshot {
    pub job_id:      String,
    pub project_dir: String,
    pub alias:       String,
    pub args:        Vec<String>,
    pub priority:    Priority,
    pub queued_at:   DateTime<Utc>,
    /// Position in the queue (0 = next to run).
    pub position:    usize,
}
