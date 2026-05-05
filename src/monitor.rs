// src/monitor.rs
// System resource monitor — gates new build scheduling on CPU/RAM headroom
// and adopts already-running cargo.exe processes launched outside Sheppard.

use std::path::{Path, PathBuf};

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::GlobalConfig;
use crate::ipc::{RunningJob, RunningJobSource};

pub struct ResourceMonitor {
    sys: System,
    last_cpu: f32,
    last_ram_pct: f64,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            sys,
            last_cpu: 0.0,
            last_ram_pct: 0.0,
        }
    }

    /// Refresh the internal counters. Call before reading CPU/RAM.
    pub fn refresh(&mut self) {
        self.sys.refresh_all();
        self.sys.refresh_memory();

        let cpus = self.sys.cpus();
        self.last_cpu = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        };

        let total = self.sys.total_memory();
        self.last_ram_pct = if total == 0 {
            0.0
        } else {
            (self.sys.used_memory() as f64 / total as f64) * 100.0
        };
    }

    pub fn cpu_usage(&self) -> f32 {
        self.last_cpu
    }
    pub fn ram_usage_pct(&self) -> f64 {
        self.last_ram_pct
    }

    /// Returns true when it's safe to start another build.
    /// Thresholds are supplied by the caller (from config) so they can be
    /// changed live without restarting the daemon.
    pub fn can_start_build(&self, max_cpu: f32, max_ram: f64) -> bool {
        self.last_cpu < max_cpu && self.last_ram_pct < max_ram
    }

    pub fn external_cargo_jobs(
        &mut self,
        managed_pids: &[u32],
        config: &GlobalConfig,
    ) -> Vec<RunningJob> {
        self.sys.refresh_processes();
        let now = chrono::Utc::now();
        let shepherd_shims = known_shepherd_shim_paths();

        let mut jobs: Vec<RunningJob> = self
            .sys
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let pid_u32 = pid.to_string().parse::<u32>().ok()?;
                if managed_pids.contains(&pid_u32) || pid_u32 == std::process::id() {
                    return None;
                }

                if !is_cargo_process(process.name()) {
                    return None;
                }
                if is_known_shepherd_shim(process.exe(), &shepherd_shims) {
                    return None;
                }

                let project_dir = process
                    .cwd()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| "<unknown project>".to_string());
                let alias = if project_dir == "<unknown project>" {
                    "external cargo".to_string()
                } else {
                    config.alias_for(&project_dir)
                };
                let args = cargo_args(process.cmd());
                let started_at = timestamp_to_utc(process.start_time());
                let elapsed_ms = now
                    .signed_duration_since(started_at)
                    .num_milliseconds()
                    .max(0) as u64;

                Some(RunningJob {
                    job_id: format!("external-{}", pid_u32),
                    project_dir,
                    alias,
                    args,
                    pid: pid_u32,
                    source: RunningJobSource::ExternalCargo,
                    started_at,
                    elapsed_ms,
                })
            })
            .collect();

        jobs.sort_by_key(|job| job.started_at);
        jobs
    }

    pub fn kill_external_cargo_pid(&mut self, target_pid: u32) -> bool {
        self.sys.refresh_processes();
        let shepherd_shims = known_shepherd_shim_paths();
        self.sys.processes().iter().any(|(pid, process)| {
            let Ok(pid_u32) = pid.to_string().parse::<u32>() else {
                return false;
            };
            pid_u32 == target_pid
                && is_cargo_process(process.name())
                && !is_known_shepherd_shim(process.exe(), &shepherd_shims)
                && process.kill()
        })
    }

    pub fn kill_existing_rust_build_processes() -> Vec<KilledBuildProcess> {
        let mut sys = System::new_all();
        sys.refresh_processes();
        let current_pid = std::process::id();
        let shepherd_shims = known_shepherd_shim_paths();

        let mut killed = Vec::new();
        let mut killed_pids = Vec::new();
        for (pid, process) in sys.processes() {
            let Ok(pid_u32) = pid.to_string().parse::<u32>() else {
                continue;
            };
            if pid_u32 == current_pid
                || !is_rust_build_process(process.name())
                || is_known_shepherd_shim(process.exe(), &shepherd_shims)
            {
                continue;
            }

            if process.kill() {
                killed_pids.push(pid_u32);
                killed.push(KilledBuildProcess {
                    pid: pid_u32,
                    name: process.name().to_string(),
                });
            }
        }

        for _ in 0..30 {
            if killed_pids.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            sys.refresh_processes();
            killed_pids.retain(|pid| {
                sys.processes().keys().any(|candidate| {
                    candidate
                        .to_string()
                        .parse::<u32>()
                        .map(|candidate_pid| candidate_pid == *pid)
                        .unwrap_or(false)
                })
            });
        }

        killed.sort_by_key(|process| process.pid);
        killed
    }
}

pub struct KilledBuildProcess {
    pub pid: u32,
    pub name: String,
}

fn is_cargo_process(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "cargo" || lower == "cargo.exe"
}

fn is_rust_build_process(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "cargo" | "cargo.exe" | "rustc" | "rustc.exe" | "rust-lld" | "rust-lld.exe"
    )
}

fn known_shepherd_shim_paths() -> Vec<PathBuf> {
    let Ok(current_exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(exe_dir) = current_exe.parent() else {
        return Vec::new();
    };

    let mut candidates = vec![
        exe_dir.join("cargo.exe"),
        exe_dir.join("shim").join("cargo.exe"),
        exe_dir.join("bin").join("shim").join("cargo.exe"),
    ];

    if let Some(parent) = exe_dir.parent() {
        candidates.push(parent.join("bin").join("shim").join("cargo.exe"));
    }

    candidates
        .into_iter()
        .filter_map(|path| normalize_path(&path))
        .collect()
}

fn is_known_shepherd_shim(process_exe: Option<&Path>, known_shims: &[PathBuf]) -> bool {
    let Some(process_exe) = process_exe.and_then(normalize_path) else {
        return false;
    };

    known_shims.iter().any(|shim| *shim == process_exe)
}

fn normalize_path(path: &Path) -> Option<PathBuf> {
    path.canonicalize()
        .ok()
        .or_else(|| Some(PathBuf::from(path.to_string_lossy().to_ascii_lowercase())))
}

fn cargo_args(cmd: &[String]) -> Vec<String> {
    if cmd.len() > 1 {
        cmd.iter().skip(1).cloned().collect()
    } else {
        vec!["<external cargo>".to_string()]
    }
}

fn timestamp_to_utc(timestamp: u64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp as i64, 0)
        .unwrap_or_else(chrono::Utc::now)
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}
