// src/runner.rs
// Spawns and owns a single `cargo` child process.
// stdout/stderr are streamed to tracing at INFO level.
// child_jobs controls CARGO_BUILD_JOBS (rustc thread count) per invocation.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{info, warn};

pub struct CargoRunner {
    pub pid: u32,
    child: Child,
}

impl CargoRunner {
    /// Spawn `cargo <args>` in `project_dir` with a capped rustc thread count.
    pub async fn spawn(
        project_dir: &str,
        args: &[String],
        job_id: &str,
        child_jobs: usize,
    ) -> Result<Self> {
        let dir = PathBuf::from(project_dir);
        let id = job_id.to_string();
        let cj_str = child_jobs.to_string();

        let mut cmd = Command::new("cargo");
        cmd.args(args)
            .current_dir(&dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Shepherd controls JOB-level concurrency (slots).
            // child_jobs controls THREAD-level concurrency per cargo invocation.
            // Together: slots × child_jobs = total rustc threads — tunable.
            .env("CARGO_BUILD_JOBS", &cj_str)
            .env("CARGO_TERM_COLOR", "always");

        let mut child = cmd.spawn().with_context(|| {
            format!(
                "Failed to spawn `cargo {}` in {}",
                args.join(" "),
                project_dir
            )
        })?;

        let pid = child.id().unwrap_or(0);

        // Stream stdout (cargo puts most build output on stderr, but check output on stdout)
        if let Some(stdout) = child.stdout.take() {
            let id_clone = id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    info!(job = %id_clone, "{}", line);
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let id_clone = id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    info!(job = %id_clone, "{}", line);
                }
            });
        }

        info!(job = %id, pid, ?dir, jobs = child_jobs, "cargo spawned");
        Ok(Self { pid, child })
    }

    /// Wait for the process to finish; returns the exit code (−1 if unavailable).
    pub async fn wait(&mut self) -> i32 {
        match self.child.wait().await {
            Ok(status) => status.code().unwrap_or(-1),
            Err(e) => {
                warn!("Error waiting for child process: {}", e);
                -1
            }
        }
    }

    /// Send SIGKILL / TerminateProcess. Safe to call multiple times.
    pub async fn kill(&mut self) {
        if let Err(e) = self.child.kill().await {
            warn!(
                "Failed to kill child process (may have already exited): {}",
                e
            );
        }
    }
}
