// src/monitor.rs
// System resource monitor — gates new build scheduling on CPU/RAM headroom
// and herds already-running Rust build processes launched outside Sheppard.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::config::{GlobalConfig, Priority};
use crate::ipc::{QueuedJobSnapshot, QueuedJobSource, RunningJob, RunningJobSource};

pub struct ResourceMonitor {
    sys: System,
    last_cpu: f32,
    last_ram_pct: f64,
    herded: HashMap<u32, HerdedExternalJob>,
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
            herded: HashMap::new(),
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

    pub fn active_external_count(&self) -> usize {
        self.herded
            .values()
            .filter(|job| job.state == HerdState::Active)
            .count()
    }

    pub fn reconcile_external_herd(
        &mut self,
        managed_pids: &[u32],
        config: &GlobalConfig,
        managed_active: usize,
    ) -> ExternalHerdSnapshot {
        if !config.herd_unmanaged {
            return ExternalHerdSnapshot {
                running: self.external_cargo_jobs(managed_pids, config),
                queued: Vec::new(),
            };
        }

        self.sys.refresh_processes();
        let now = Utc::now();
        let detected = self.detect_external_rust_roots(managed_pids, config);
        let seen: HashSet<u32> = detected.keys().copied().collect();

        self.herded.retain(|pid, job| {
            if seen.contains(pid) {
                true
            } else {
                if job.state == HerdState::Suspended {
                    resume_process_tree(&job.process_pids());
                }
                false
            }
        });

        for detected_job in detected.into_values() {
            match self.herded.get_mut(&detected_job.root_pid) {
                Some(job) => {
                    job.project_dir = detected_job.project_dir;
                    job.alias = detected_job.alias;
                    job.args = detected_job.args;
                    job.child_pids = detected_job.child_pids;
                    job.last_seen = now;
                }
                None => {
                    self.herded.insert(
                        detected_job.root_pid,
                        HerdedExternalJob {
                            root_pid: detected_job.root_pid,
                            project_dir: detected_job.project_dir,
                            alias: detected_job.alias,
                            args: detected_job.args,
                            child_pids: detected_job.child_pids,
                            started_at: detected_job.started_at,
                            last_seen: now,
                            queued_at: now,
                            state: HerdState::Active,
                            reason: None,
                        },
                    );
                }
            }
        }

        self.enforce_external_herd_policy(config, managed_active);
        self.external_herd_snapshot()
    }

    fn detect_external_rust_roots(
        &self,
        managed_pids: &[u32],
        config: &GlobalConfig,
    ) -> HashMap<u32, DetectedRustRoot> {
        let shepherd_shims = known_shepherd_shim_paths();
        let managed: HashSet<u32> = managed_pids.iter().copied().collect();
        let current_pid = std::process::id();

        let process_map: HashMap<u32, ProcessInfo> = self
            .sys
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let pid_u32 = pid_to_u32(pid)?;
                let exe = process.exe().and_then(normalize_path);
                let cwd = process
                    .cwd()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| "<unknown project>".to_string());
                Some((
                    pid_u32,
                    ProcessInfo {
                        pid: pid_u32,
                        parent: process.parent().and_then(pid_to_u32),
                        name: process.name().to_string(),
                        exe,
                        cwd,
                        cmd: process.cmd().to_vec(),
                        started_at: timestamp_to_utc(process.start_time()),
                    },
                ))
            })
            .collect();

        let mut root_pids = HashSet::new();
        for process in process_map.values() {
            if process.pid == current_pid
                || managed.contains(&process.pid)
                || has_ancestor(process.pid, &managed, &process_map)
                || !is_rust_activity_process(&process.name)
                || is_known_shepherd_shim_path(process.exe.as_ref(), &shepherd_shims)
            {
                continue;
            }

            let root_pid = rust_activity_root(process.pid, &process_map);
            let Some(root) = process_map.get(&root_pid) else {
                continue;
            };
            if root.pid == current_pid
                || managed.contains(&root.pid)
                || has_ancestor(root.pid, &managed, &process_map)
                || is_known_shepherd_shim_path(root.exe.as_ref(), &shepherd_shims)
            {
                continue;
            }

            root_pids.insert(root_pid);
        }

        let mut roots = HashMap::new();
        for root_pid in root_pids {
            let Some(root) = process_map.get(&root_pid) else {
                continue;
            };
            let mut child_pids: Vec<u32> = process_map
                .keys()
                .copied()
                .filter(|pid| *pid != root_pid && is_descendant_of(*pid, root_pid, &process_map))
                .collect();
            child_pids.sort_unstable();

            let alias = if root.cwd == "<unknown project>" {
                "external rust".to_string()
            } else {
                config.alias_for(&root.cwd)
            };
            roots.insert(
                root_pid,
                DetectedRustRoot {
                    root_pid,
                    project_dir: root.cwd.clone(),
                    alias,
                    args: cargo_args(&root.cmd),
                    child_pids,
                    started_at: root.started_at,
                },
            );
        }

        roots
    }

    fn enforce_external_herd_policy(&mut self, config: &GlobalConfig, managed_active: usize) {
        let mut ids: Vec<u32> = self.herded.keys().copied().collect();
        ids.sort_by_key(|pid| self.herded.get(pid).map(|job| job.started_at));

        let has_waiting = self
            .herded
            .values()
            .any(|job| job.state == HerdState::Suspended);
        let ram_gate_closed = if has_waiting {
            self.last_ram_pct >= config.herd_ram_resume_pct
        } else {
            self.last_ram_pct >= config.herd_ram_pause_pct
        };
        let gate_closed = ram_gate_closed || self.last_cpu >= config.max_cpu_pct;
        let configured_limit = config.herd_max_active.max(1);
        let slot_room = if config.slots == 0 {
            configured_limit
        } else {
            config
                .slots
                .saturating_sub(managed_active)
                .min(configured_limit)
        };

        let allowed: HashSet<u32> = if gate_closed {
            ids.iter()
                .copied()
                .filter(|pid| {
                    self.herded
                        .get(pid)
                        .map(|job| job.state == HerdState::Active)
                        .unwrap_or(false)
                })
                .take(slot_room.min(1))
                .collect()
        } else {
            ids.iter().copied().take(slot_room).collect()
        };

        let reason = self.herd_pause_reason(config, gate_closed, slot_room);
        for pid in ids {
            let should_run = allowed.contains(&pid);
            let Some(job) = self.herded.get_mut(&pid) else {
                continue;
            };

            match (should_run, job.state) {
                (true, HerdState::Suspended) if !gate_closed => {
                    if resume_process_tree(&job.process_pids()) {
                        job.state = HerdState::Active;
                        job.reason = None;
                    }
                }
                (false, HerdState::Active) => {
                    if suspend_process_tree(&job.process_pids()) {
                        job.state = HerdState::Suspended;
                        job.queued_at = Utc::now();
                        job.reason = Some(reason.clone());
                    }
                }
                (false, HerdState::Suspended) => {
                    job.reason = Some(reason.clone());
                }
                _ => {}
            }
        }
    }

    fn herd_pause_reason(
        &self,
        config: &GlobalConfig,
        gate_closed: bool,
        slot_room: usize,
    ) -> String {
        if self.last_ram_pct >= config.herd_ram_pause_pct {
            format!(
                "RAM {:.1}% >= {:.1}%",
                self.last_ram_pct, config.herd_ram_pause_pct
            )
        } else if self.last_cpu >= config.max_cpu_pct {
            format!("CPU {:.1}% >= {:.1}%", self.last_cpu, config.max_cpu_pct)
        } else if gate_closed {
            "resource gate closed".to_string()
        } else if slot_room == 0 {
            "slot limit reached".to_string()
        } else {
            "waiting for older Rust build".to_string()
        }
    }

    fn external_herd_snapshot(&self) -> ExternalHerdSnapshot {
        let now = Utc::now();
        let mut running = Vec::new();
        let mut queued = Vec::new();

        let mut jobs: Vec<&HerdedExternalJob> = self.herded.values().collect();
        jobs.sort_by_key(|job| job.started_at);

        for job in jobs {
            let elapsed_ms = now
                .signed_duration_since(job.started_at)
                .num_milliseconds()
                .max(0) as u64;

            match job.state {
                HerdState::Active => running.push(RunningJob {
                    job_id: job.job_id(),
                    project_dir: job.project_dir.clone(),
                    alias: job.alias.clone(),
                    args: job.args.clone(),
                    pid: job.root_pid,
                    source: RunningJobSource::ExternalRust,
                    started_at: job.started_at,
                    elapsed_ms,
                }),
                HerdState::Suspended => queued.push(QueuedJobSnapshot {
                    job_id: job.job_id(),
                    project_dir: job.project_dir.clone(),
                    alias: job.alias.clone(),
                    args: job.args.clone(),
                    priority: Priority::Normal,
                    queued_at: job.queued_at,
                    source: QueuedJobSource::SuspendedExternalRust,
                    pid: Some(job.root_pid),
                    child_count: job.child_pids.len(),
                    reason: job.reason.clone(),
                    position: 0,
                }),
            }
        }

        ExternalHerdSnapshot { running, queued }
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
        self.kill_external_rust_pid(target_pid)
    }

    pub fn kill_external_rust_pid(&mut self, target_pid: u32) -> bool {
        self.sys.refresh_processes();
        let mut pids = if let Some(job) = self.herded.remove(&target_pid) {
            job.process_pids()
        } else {
            vec![target_pid]
        };
        pids.reverse();

        let mut killed_any = false;
        for target in pids {
            killed_any |= self.sys.processes().iter().any(|(pid, process)| {
                pid_to_u32(pid)
                    .map(|pid_u32| pid_u32 == target && process.kill())
                    .unwrap_or(false)
            });
        }
        killed_any
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
                || !is_rust_activity_process(process.name())
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

pub struct ExternalHerdSnapshot {
    pub running: Vec<RunningJob>,
    pub queued: Vec<QueuedJobSnapshot>,
}

#[derive(Clone)]
struct DetectedRustRoot {
    root_pid: u32,
    project_dir: String,
    alias: String,
    args: Vec<String>,
    child_pids: Vec<u32>,
    started_at: DateTime<Utc>,
}

struct ProcessInfo {
    pid: u32,
    parent: Option<u32>,
    name: String,
    exe: Option<PathBuf>,
    cwd: String,
    cmd: Vec<String>,
    started_at: DateTime<Utc>,
}

struct HerdedExternalJob {
    root_pid: u32,
    project_dir: String,
    alias: String,
    args: Vec<String>,
    child_pids: Vec<u32>,
    started_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    queued_at: DateTime<Utc>,
    state: HerdState,
    reason: Option<String>,
}

impl HerdedExternalJob {
    fn job_id(&self) -> String {
        format!("external-{}", self.root_pid)
    }

    fn process_pids(&self) -> Vec<u32> {
        let mut pids = Vec::with_capacity(self.child_pids.len() + 1);
        pids.push(self.root_pid);
        pids.extend(self.child_pids.iter().copied());
        pids.sort_unstable();
        pids.dedup();
        pids
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HerdState {
    Active,
    Suspended,
}

fn is_cargo_process(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "cargo" || lower == "cargo.exe"
}

fn is_rust_activity_process(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        stem,
        "cargo" | "rustc" | "rust-lld" | "rustdoc" | "clippy-driver" | "rustfmt"
    ) || stem.starts_with("cargo-")
}

fn rust_activity_root(pid: u32, processes: &HashMap<u32, ProcessInfo>) -> u32 {
    let mut root = pid;
    let mut seen = HashSet::new();

    while seen.insert(root) {
        let Some(process) = processes.get(&root) else {
            break;
        };
        let Some(parent_pid) = process.parent else {
            break;
        };
        let Some(parent) = processes.get(&parent_pid) else {
            break;
        };
        if !is_rust_activity_process(&parent.name) {
            break;
        }
        root = parent_pid;
    }

    root
}

fn has_ancestor(pid: u32, ancestors: &HashSet<u32>, processes: &HashMap<u32, ProcessInfo>) -> bool {
    let mut current = pid;
    let mut seen = HashSet::new();

    while seen.insert(current) {
        let Some(process) = processes.get(&current) else {
            return false;
        };
        let Some(parent_pid) = process.parent else {
            return false;
        };
        if ancestors.contains(&parent_pid) {
            return true;
        }
        current = parent_pid;
    }

    false
}

fn is_descendant_of(pid: u32, root_pid: u32, processes: &HashMap<u32, ProcessInfo>) -> bool {
    let mut current = pid;
    let mut seen = HashSet::new();

    while seen.insert(current) {
        let Some(process) = processes.get(&current) else {
            return false;
        };
        let Some(parent_pid) = process.parent else {
            return false;
        };
        if parent_pid == root_pid {
            return true;
        }
        current = parent_pid;
    }

    false
}

fn pid_to_u32<T: std::fmt::Display>(pid: T) -> Option<u32> {
    pid.to_string().parse::<u32>().ok()
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

fn is_known_shepherd_shim_path(process_exe: Option<&PathBuf>, known_shims: &[PathBuf]) -> bool {
    process_exe
        .map(|exe| known_shims.iter().any(|shim| shim == exe))
        .unwrap_or(false)
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

fn suspend_process_tree(pids: &[u32]) -> bool {
    let mut changed = false;
    for pid in pids.iter().rev() {
        changed |= suspend_process(*pid);
    }
    changed
}

fn resume_process_tree(pids: &[u32]) -> bool {
    let mut changed = false;
    for pid in pids {
        changed |= resume_process(*pid);
    }
    changed
}

#[cfg(windows)]
fn suspend_process(pid: u32) -> bool {
    use std::mem;
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::processthreadsapi::{OpenThread, SuspendThread};
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use winapi::um::winnt::THREAD_SUSPEND_RESUME;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry: THREADENTRY32 = mem::zeroed();
        entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        let mut changed = false;

        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        changed |= SuspendThread(thread) != u32::MAX;
                        CloseHandle(thread);
                    }
                }

                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        changed
    }
}

#[cfg(windows)]
fn resume_process(pid: u32) -> bool {
    use std::mem;
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::processthreadsapi::{OpenThread, ResumeThread};
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use winapi::um::winnt::THREAD_SUSPEND_RESUME;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry: THREADENTRY32 = mem::zeroed();
        entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
        let mut changed = false;

        if Thread32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        for _ in 0..64 {
                            let previous = ResumeThread(thread);
                            if previous == u32::MAX {
                                break;
                            }
                            changed = true;
                            if previous <= 1 {
                                break;
                            }
                        }
                        CloseHandle(thread);
                    }
                }

                if Thread32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        changed
    }
}

#[cfg(not(windows))]
fn suspend_process(_pid: u32) -> bool {
    false
}

#[cfg(not(windows))]
fn resume_process(_pid: u32) -> bool {
    false
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::is_rust_activity_process;

    #[test]
    fn rust_activity_matcher_covers_startup_takeover_processes() {
        for name in [
            "cargo.exe",
            "rustc.exe",
            "rust-lld.exe",
            "rustdoc.exe",
            "clippy-driver.exe",
            "rustfmt.exe",
            "cargo-metadata.exe",
        ] {
            assert!(is_rust_activity_process(name), "{name} should be matched");
        }

        for name in ["node.exe", "npm.exe", "openllm-studio.exe", "shepherd.exe"] {
            assert!(!is_rust_activity_process(name), "{name} should be ignored");
        }
    }
}
