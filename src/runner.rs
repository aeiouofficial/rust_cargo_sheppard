// src/runner.rs
// Spawns and owns a single `cargo` child process.
// stdout/stderr are streamed to tracing at INFO level.
// child_jobs controls CARGO_BUILD_JOBS (rustc thread count) per invocation.

use crate::ipc::{CargoOutputStream, DaemonMsg};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
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
        attached_tx: Option<mpsc::UnboundedSender<DaemonMsg>>,
    ) -> Result<Self> {
        let dir = PathBuf::from(project_dir);
        let id = job_id.to_string();
        let cj_str = child_jobs.to_string();

        let cargo_path = resolve_real_cargo()?;
        let mut cmd = Command::new(&cargo_path);
        cmd.args(args)
            .current_dir(&dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Shepherd controls JOB-level concurrency (slots).
            // child_jobs controls THREAD-level concurrency per cargo invocation.
            // Together: slots × child_jobs = total rustc threads — tunable.
            .env("CARGO_BUILD_JOBS", &cj_str)
            .env("CARGO_TERM_COLOR", "always");

        if std::env::var_os("RUSTC_WRAPPER").is_none() && command_exists_on_path("sccache") {
            cmd.env("RUSTC_WRAPPER", "sccache");
            info!(job = %id, "sccache detected; enabling RUSTC_WRAPPER=sccache");
        }

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
            let output_tx = attached_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(tx) = &output_tx {
                        let _ = tx.send(DaemonMsg::CargoOutput {
                            job_id: id_clone.clone(),
                            stream: CargoOutputStream::Stdout,
                            line: line.clone(),
                        });
                    }
                    info!(job = %id_clone, "{}", line);
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let id_clone = id.clone();
            let output_tx = attached_tx.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(tx) = &output_tx {
                        let _ = tx.send(DaemonMsg::CargoOutput {
                            job_id: id_clone.clone(),
                            stream: CargoOutputStream::Stderr,
                            line: line.clone(),
                        });
                    }
                    info!(job = %id_clone, "{}", line);
                }
            });
        }

        info!(job = %id, pid, ?dir, ?cargo_path, jobs = child_jobs, "cargo spawned");
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

pub fn resolve_real_cargo() -> Result<PathBuf> {
    if let Some(configured) = std::env::var_os("SHEPHERD_REAL_CARGO") {
        if !configured.as_os_str().is_empty() {
            return Ok(PathBuf::from(configured));
        }
    }

    find_real_cargo_on_path().context(
        "Could not find the real Cargo executable. Set SHEPHERD_REAL_CARGO to the rustup/cargo path.",
    )
}

fn find_real_cargo_on_path() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let current_exe = std::env::current_exe().ok().and_then(canonicalize_lossy);
    let current_dir = current_exe
        .as_ref()
        .and_then(|path| path.parent())
        .and_then(|path| canonicalize_lossy(path.to_path_buf()));
    let extensions = executable_extensions("cargo");

    for dir in std::env::split_paths(&path_var) {
        for extension in &extensions {
            let mut file_name = OsString::from("cargo");
            file_name.push(extension);
            let candidate = dir.join(file_name);
            if !candidate.is_file() {
                continue;
            }

            let canonical = canonicalize_lossy(candidate.clone()).unwrap_or(candidate);
            if current_exe.as_ref() == Some(&canonical) {
                continue;
            }
            if let (Some(parent), Some(current_dir)) = (canonical.parent(), current_dir.as_ref()) {
                if canonicalize_lossy(parent.to_path_buf()).as_ref() == Some(current_dir) {
                    continue;
                }
            }

            return Some(canonical);
        }
    }

    None
}

fn canonicalize_lossy(path: PathBuf) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn command_exists_on_path(command: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    let command_path = PathBuf::from(command);
    if command_path.components().count() > 1 {
        return command_path.is_file();
    }

    let extensions = executable_extensions(command);
    std::env::split_paths(&path_var).any(|dir| {
        extensions.iter().any(|extension| {
            let mut file_name = OsString::from(command);
            file_name.push(extension);
            dir.join(file_name).is_file()
        })
    })
}

#[cfg(windows)]
fn executable_extensions(command: &str) -> Vec<OsString> {
    if PathBuf::from(command).extension().is_some() {
        return vec![OsString::new()];
    }

    let pathext =
        std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
    let mut extensions = vec![OsString::new()];
    extensions.extend(
        pathext
            .to_string_lossy()
            .split(';')
            .filter(|extension| !extension.trim().is_empty())
            .map(|extension| OsString::from(extension.to_ascii_lowercase())),
    );
    extensions
}

#[cfg(not(windows))]
fn executable_extensions(_command: &str) -> Vec<OsString> {
    vec![OsString::new()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_command_is_not_found_on_path() {
        let unique = format!(
            "cargo-shepherd-missing-command-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        assert!(!command_exists_on_path(&unique));
    }
}
