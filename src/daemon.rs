// src/daemon.rs
// cargo-shepherd background daemon.
//
// Architecture change from v0.1:
//   OLD: tokio::Semaphore (FIFO — ignores priority)
//   NEW: dedicated scheduler loop + PriorityQueue
//        - Scheduler wakes on Notify whenever a slot may have opened or a job was added
//        - Picks highest-priority queued job
//        - True priority scheduling with reprioritization support
//
// Cross-platform IPC:
//   Unix:    tokio::net::UnixListener (domain socket)
//   Windows: tokio::net::windows::named_pipe (named pipe server)

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{error, info};

use crate::config::{slot_limit_label, GlobalConfig};
use crate::ipc::{ClientMsg, DaemonMsg, QueuedJobSnapshot, RunningJob, StatusReport};
use crate::monitor::ResourceMonitor;
use crate::queue::{PriorityQueue, QueuedJob};
use crate::runner::CargoRunner;

// ─────────────────────────── Shared state ────────────────────────────────────

struct RunningEntry {
    job: RunningJob,
    kill_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

struct SharedState {
    running: HashMap<String, RunningEntry>, // job_id → entry
    queue: PriorityQueue,
    active: usize,
    config: GlobalConfig,
    monitor: ResourceMonitor,
}

impl SharedState {
    fn new(config: GlobalConfig) -> Self {
        Self {
            running: HashMap::new(),
            queue: PriorityQueue::new(),
            active: 0,
            config,
            monitor: ResourceMonitor::new(),
        }
    }

    fn status_report(&mut self) -> StatusReport {
        self.monitor.refresh();

        let queued = self.queue.snapshot();
        let now = Utc::now();

        StatusReport {
            running: self
                .running
                .values()
                .map(|entry| {
                    let mut job = entry.job.clone();
                    job.elapsed_ms = now
                        .signed_duration_since(job.started_at)
                        .num_milliseconds()
                        .max(0) as u64;
                    job
                })
                .collect(),
            queued: queued
                .iter()
                .enumerate()
                .map(|(i, j)| QueuedJobSnapshot {
                    job_id: j.job_id.clone(),
                    project_dir: j.project_dir.clone(),
                    alias: j.alias.clone(),
                    args: j.args.clone(),
                    priority: j.priority,
                    queued_at: j.queued_at,
                    position: i,
                })
                .collect(),
            slots_total: self.config.slots,
            slots_active: self.active,
            cpu_pct: self.monitor.cpu_usage(),
            ram_pct: self.monitor.ram_usage_pct(),
        }
    }

    fn can_start_another(&mut self) -> bool {
        (self.config.slots == 0 || self.active < self.config.slots)
            && self
                .monitor
                .can_start_build(self.config.max_cpu_pct, self.config.max_ram_pct)
    }

    fn request_kill_job(&mut self, job_id: &str) -> KillJobOutcome {
        let resolved_id = match self.resolve_job_id(job_id) {
            JobIdResolution::Found(id) => id,
            JobIdResolution::Ambiguous => return KillJobOutcome::Ambiguous,
            JobIdResolution::NotFound => return KillJobOutcome::NotFound,
        };

        if self.queue.remove(&resolved_id).is_some() {
            return KillJobOutcome::RemovedQueued;
        }

        match self.running.get_mut(&resolved_id) {
            Some(entry) => match entry.kill_tx.take() {
                Some(kill_tx) => {
                    let _ = kill_tx.send(());
                    KillJobOutcome::SignaledRunning
                }
                None => KillJobOutcome::AlreadyRequested,
            },
            None => KillJobOutcome::NotFound,
        }
    }

    fn set_queued_job_priority(
        &mut self,
        job_id: &str,
        new_priority: crate::config::Priority,
    ) -> SetPriorityOutcome {
        match self.resolve_job_id(job_id) {
            JobIdResolution::Found(resolved_id) => {
                if self.queue.set_priority(&resolved_id, new_priority) {
                    SetPriorityOutcome::Changed {
                        job_id: resolved_id.clone(),
                        new_position: self.queue.position_of(&resolved_id).unwrap_or(0),
                    }
                } else if self.running.contains_key(&resolved_id) {
                    SetPriorityOutcome::AlreadyRunning
                } else {
                    SetPriorityOutcome::NotFound
                }
            }
            JobIdResolution::Ambiguous => SetPriorityOutcome::Ambiguous,
            JobIdResolution::NotFound => SetPriorityOutcome::NotFound,
        }
    }

    fn cancel_queued_job(&mut self, job_id: &str) -> CancelJobOutcome {
        match self.resolve_job_id(job_id) {
            JobIdResolution::Found(resolved_id) => {
                if self.queue.remove(&resolved_id).is_some() {
                    CancelJobOutcome::Cancelled(resolved_id)
                } else if self.running.contains_key(&resolved_id) {
                    CancelJobOutcome::AlreadyRunning
                } else {
                    CancelJobOutcome::NotFound
                }
            }
            JobIdResolution::Ambiguous => CancelJobOutcome::Ambiguous,
            JobIdResolution::NotFound => CancelJobOutcome::NotFound,
        }
    }

    fn resolve_job_id(&self, job_id: &str) -> JobIdResolution {
        let mut matches = Vec::new();

        for queued in self.queue.snapshot() {
            if queued.job_id == job_id {
                return JobIdResolution::Found(queued.job_id);
            }
            if queued.job_id.starts_with(job_id) {
                matches.push(queued.job_id);
            }
        }

        for running_id in self.running.keys() {
            if running_id == job_id {
                return JobIdResolution::Found(running_id.clone());
            }
            if running_id.starts_with(job_id) {
                matches.push(running_id.clone());
            }
        }

        matches.sort();
        matches.dedup();

        match matches.len() {
            0 => JobIdResolution::NotFound,
            1 => JobIdResolution::Found(matches.remove(0)),
            _ => JobIdResolution::Ambiguous,
        }
    }

    fn request_kill_project(&mut self, project_dir: &str) -> (usize, usize) {
        let removed_queued = self.queue.remove_project(project_dir).len();

        let running_ids: Vec<String> = self
            .running
            .iter()
            .filter(|(_, entry)| entry.job.project_dir == project_dir)
            .map(|(id, _)| id.clone())
            .collect();

        let mut killed_running = 0usize;
        for id in running_ids {
            if let Some(entry) = self.running.get_mut(&id) {
                if let Some(kill_tx) = entry.kill_tx.take() {
                    let _ = kill_tx.send(());
                    killed_running += 1;
                }
            }
        }

        (removed_queued, killed_running)
    }
}

enum KillJobOutcome {
    RemovedQueued,
    SignaledRunning,
    AlreadyRequested,
    Ambiguous,
    NotFound,
}

enum SetPriorityOutcome {
    Changed { job_id: String, new_position: usize },
    AlreadyRunning,
    Ambiguous,
    NotFound,
}

enum CancelJobOutcome {
    Cancelled(String),
    AlreadyRunning,
    Ambiguous,
    NotFound,
}

enum JobIdResolution {
    Found(String),
    Ambiguous,
    NotFound,
}

// ─────────────────────────── Daemon ──────────────────────────────────────────

pub struct Daemon;

impl Daemon {
    pub async fn run(config: GlobalConfig) -> Result<()> {
        let state = Arc::new(Mutex::new(SharedState::new(config)));
        let notify = Arc::new(Notify::new());
        let (spawn_tx, spawn_rx) = mpsc::unbounded_channel::<QueuedJob>();

        // Spawn the scheduler loop
        tokio::spawn(scheduler_loop(
            Arc::clone(&state),
            Arc::clone(&notify),
            spawn_tx,
        ));

        // Spawn the job runner pool
        tokio::spawn(runner_pool(
            Arc::clone(&state),
            Arc::clone(&notify),
            spawn_rx,
        ));

        // Platform-specific listener
        #[cfg(unix)]
        {
            run_unix_listener(state, notify).await
        }
        #[cfg(windows)]
        {
            run_windows_listener(state, notify).await
        }
    }
}

// ─────────────────────────── Unix listener ────────────────────────────────────

#[cfg(unix)]
async fn run_unix_listener(state: Arc<Mutex<SharedState>>, notify: Arc<Notify>) -> Result<()> {
    use tokio::net::UnixListener;

    let socket = socket_path();

    // Remove stale socket from previous run
    let _ = std::fs::remove_file(&socket);

    let listener = UnixListener::bind(&socket)?;

    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    }

    let slots = state.lock().await.config.slots;
    info!(
        "Daemon listening on {} ({} slots)",
        socket.display(),
        slot_limit_label(slots)
    );

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                let notify = Arc::clone(&notify);
                tokio::spawn(async move {
                    let (reader, writer) = stream.into_split();
                    if let Err(e) = handle_connection(reader, writer, state, notify).await {
                        error!("Connection error: {}", e);
                    }
                });
            }
            Err(e) => error!("Accept error: {}", e),
        }
    }
}

// ─────────────────────────── Windows named pipe listener ──────────────────────

#[cfg(windows)]
async fn run_windows_listener(state: Arc<Mutex<SharedState>>, notify: Arc<Notify>) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let pipe_name = crate::ipc::pipe_name();
    let slots = state.lock().await.config.slots;
    info!(
        "Daemon listening on {} ({} slots)",
        pipe_name,
        slot_limit_label(slots)
    );

    // Create the first pipe instance
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&pipe_name)?;

    loop {
        // Wait for a client to connect
        server.connect().await?;

        // Swap in a new server for the next client before handling this one
        let next_server = ServerOptions::new().create(&pipe_name)?;
        let current_server = std::mem::replace(&mut server, next_server);

        let state = Arc::clone(&state);
        let notify = Arc::clone(&notify);

        tokio::spawn(async move {
            let (reader, writer) = tokio::io::split(current_server);
            if let Err(e) = handle_connection(reader, writer, state, notify).await {
                error!("Connection error: {}", e);
            }
        });
    }
}

// ─────────────────────────── Scheduler loop ──────────────────────────────────
// Wakes whenever a job is added OR a job finishes.
// Picks the highest-priority queued job and sends it to the runner pool.

async fn scheduler_loop(
    state: Arc<Mutex<SharedState>>,
    notify: Arc<Notify>,
    spawn_tx: mpsc::UnboundedSender<QueuedJob>,
) {
    loop {
        notify.notified().await;

        let mut s = state.lock().await;
        s.monitor.refresh();

        while s.can_start_another() {
            if let Some(job) = s.queue.pop_next() {
                s.active += 1;
                info!(
                    "Scheduling job {} (priority={:?}) — {}/{} slots",
                    job.job_id,
                    job.priority,
                    s.active,
                    slot_limit_label(s.config.slots),
                );
                let _ = spawn_tx.send(job);
            } else {
                break;
            }
        }
    }
}

// ─────────────────────────── Runner pool ────────────────────────────────────
// Receives jobs from the scheduler and executes them.
// When a job finishes, decrements active count and wakes the scheduler.

async fn runner_pool(
    state: Arc<Mutex<SharedState>>,
    notify: Arc<Notify>,
    mut rx: mpsc::UnboundedReceiver<QueuedJob>,
) {
    while let Some(queued_job) = rx.recv().await {
        let state = Arc::clone(&state);
        let notify = Arc::clone(&notify);

        tokio::spawn(async move {
            let job_id = queued_job.job_id.clone();
            let project_dir = queued_job.project_dir.clone();
            let args = queued_job.args.clone();
            let child_jobs = queued_job.child_jobs;
            let alias = queued_job.alias.clone();

            let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();

            // Register as running
            {
                let mut s = state.lock().await;
                s.running.insert(
                    job_id.clone(),
                    RunningEntry {
                        job: RunningJob {
                            job_id: job_id.clone(),
                            project_dir: project_dir.clone(),
                            alias: alias.clone(),
                            args: args.clone(),
                            pid: 0, // updated below
                            started_at: Utc::now(),
                            elapsed_ms: 0,
                        },
                        kill_tx: Some(kill_tx),
                    },
                );
            }

            let start = Instant::now();

            match CargoRunner::spawn(&project_dir, &args, &job_id, child_jobs).await {
                Ok(mut runner) => {
                    // Update PID
                    {
                        let mut s = state.lock().await;
                        if let Some(entry) = s.running.get_mut(&job_id) {
                            entry.job.pid = runner.pid;
                        }
                    }

                    // Use a fuse-style pattern: pin the kill receiver
                    // so it's properly consumed in the select! macro.
                    let exit_code = tokio::select! {
                        code = runner.wait() => code,
                        _ = kill_rx => {
                            runner.kill().await;
                            -9
                        }
                    };

                    let duration_ms = start.elapsed().as_millis() as u64;
                    info!(
                        "Job {} finished (exit={}, {}ms)",
                        job_id, exit_code, duration_ms
                    );
                }
                Err(e) => {
                    error!("Failed to spawn job {}: {}", job_id, e);
                }
            }

            // Deregister and wake scheduler
            {
                let mut s = state.lock().await;
                s.running.remove(&job_id);
                s.active = s.active.saturating_sub(1);
            }
            notify.notify_one();
        });
    }
}

// ─────────────────────────── Connection handler ──────────────────────────────
// Generic over any AsyncRead + AsyncWrite, enabling both Unix sockets and
// Windows named pipes.

async fn handle_connection<R, W>(
    reader: R,
    mut writer: W,
    state: Arc<Mutex<SharedState>>,
    notify: Arc<Notify>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let msg: ClientMsg = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                send(
                    &mut writer,
                    &DaemonMsg::Error {
                        message: format!("Parse error: {}", e),
                    },
                )
                .await?;
                continue;
            }
        };

        match msg {
            // ── Queue a build ─────────────────────────────────────────────────
            ClientMsg::Run {
                job_id,
                project_dir,
                args,
                priority,
            } => {
                let (resolved_priority, alias, child_jobs) = {
                    let s = state.lock().await;
                    let p = priority.unwrap_or_else(|| s.config.priority_for(&project_dir));
                    let a = s.config.alias_for(&project_dir);
                    let c = s.config.child_jobs_for(&project_dir);
                    (p, a, c)
                };

                let job = QueuedJob {
                    job_id: job_id.clone(),
                    project_dir: project_dir.clone(),
                    alias,
                    args: args.clone(),
                    priority: resolved_priority,
                    queued_at: Utc::now(),
                    child_jobs,
                };

                let position = {
                    let mut s = state.lock().await;
                    s.queue.push(job);
                    s.queue.position_of(&job_id).unwrap_or(0)
                };

                send(&mut writer, &DaemonMsg::Queued { job_id, position }).await?;
                notify.notify_one();
            }

            // ── Reprioritize queued job ───────────────────────────────────────
            ClientMsg::SetJobPriority {
                job_id,
                new_priority,
            } => {
                let mut s = state.lock().await;
                let outcome = s.set_queued_job_priority(&job_id, new_priority);
                drop(s);

                match outcome {
                    SetPriorityOutcome::Changed {
                        job_id,
                        new_position,
                    } => {
                        notify.notify_one();
                        send(
                            &mut writer,
                            &DaemonMsg::PriorityChanged {
                                job_id,
                                new_priority,
                                new_position,
                            },
                        )
                        .await?;
                    }
                    SetPriorityOutcome::AlreadyRunning => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!(
                                    "Job '{}' is already running and cannot be reprioritized",
                                    job_id
                                ),
                            },
                        )
                        .await?;
                    }
                    SetPriorityOutcome::Ambiguous => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!(
                                    "Job prefix '{}' is ambiguous; use more characters",
                                    job_id
                                ),
                            },
                        )
                        .await?;
                    }
                    SetPriorityOutcome::NotFound => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Job '{}' not found", job_id),
                            },
                        )
                        .await?;
                    }
                }
            }

            // ── Cancel a queued job (not yet running) ─────────────────────────
            ClientMsg::CancelJob { job_id } => {
                let mut s = state.lock().await;
                let outcome = s.cancel_queued_job(&job_id);
                drop(s);

                match outcome {
                    CancelJobOutcome::Cancelled(resolved_id) => {
                        notify.notify_one();
                        send(
                            &mut writer,
                            &DaemonMsg::Killed {
                                description: format!("Cancelled queued job {}", resolved_id),
                            },
                        )
                        .await?;
                    }
                    CancelJobOutcome::AlreadyRunning => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!(
                                    "Job '{}' is already running; use kill --job instead",
                                    job_id
                                ),
                            },
                        )
                        .await?;
                    }
                    CancelJobOutcome::Ambiguous => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!(
                                    "Job prefix '{}' is ambiguous; use more characters",
                                    job_id
                                ),
                            },
                        )
                        .await?;
                    }
                    CancelJobOutcome::NotFound => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Job '{}' not found", job_id),
                            },
                        )
                        .await?;
                    }
                }
            }

            // ── Kill running job ──────────────────────────────────────────────
            ClientMsg::KillJob { job_id } => {
                let mut s = state.lock().await;
                let outcome = s.request_kill_job(&job_id);
                drop(s);

                match outcome {
                    KillJobOutcome::RemovedQueued => {
                        notify.notify_one();
                        send(
                            &mut writer,
                            &DaemonMsg::Killed {
                                description: format!("Removed queued job {}", job_id),
                            },
                        )
                        .await?;
                    }
                    KillJobOutcome::SignaledRunning => {
                        send(
                            &mut writer,
                            &DaemonMsg::Killed {
                                description: format!("Kill requested for running job {}", job_id),
                            },
                        )
                        .await?;
                    }
                    KillJobOutcome::AlreadyRequested => {
                        send(
                            &mut writer,
                            &DaemonMsg::Killed {
                                description: format!("Kill already requested for job {}", job_id),
                            },
                        )
                        .await?;
                    }
                    KillJobOutcome::Ambiguous => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!(
                                    "Job prefix '{}' is ambiguous; use more characters",
                                    job_id
                                ),
                            },
                        )
                        .await?;
                    }
                    KillJobOutcome::NotFound => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Job '{}' not found", job_id),
                            },
                        )
                        .await?;
                    }
                }
            }

            // ── Kill entire project ───────────────────────────────────────────
            ClientMsg::KillProject { project_dir } => {
                let mut s = state.lock().await;
                let (removed_queued, killed_running) = s.request_kill_project(&project_dir);
                let total = removed_queued + killed_running;
                drop(s);

                if total > 0 {
                    notify.notify_one();
                }

                send(
                    &mut writer,
                    &DaemonMsg::Killed {
                        description: format!(
                            "Killed {} job(s) for {} ({} running, {} queued)",
                            total, project_dir, killed_running, removed_queued
                        ),
                    },
                )
                .await?;
            }

            // ── Config: set project priority ──────────────────────────────────
            ClientMsg::SetProjectPriority {
                project_dir,
                priority,
            } => {
                let mut s = state.lock().await;
                match s.config.set_project_priority(&project_dir, priority) {
                    Ok(_) => {
                        send(
                            &mut writer,
                            &DaemonMsg::ConfigUpdated {
                                message: format!(
                                    "Set default priority for '{}' to {:?}",
                                    project_dir, priority
                                ),
                            },
                        )
                        .await?
                    }
                    Err(e) => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Config save failed: {}", e),
                            },
                        )
                        .await?
                    }
                }
            }

            // ── Config: set project alias ─────────────────────────────────────
            ClientMsg::SetProjectAlias { project_dir, alias } => {
                let mut s = state.lock().await;
                match s.config.set_project_alias(&project_dir, &alias) {
                    Ok(_) => {
                        send(
                            &mut writer,
                            &DaemonMsg::ConfigUpdated {
                                message: format!("Alias for '{}' set to '{}'", project_dir, alias),
                            },
                        )
                        .await?
                    }
                    Err(e) => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Config save failed: {}", e),
                            },
                        )
                        .await?
                    }
                }
            }

            // ── Config: set project child_jobs ────────────────────────────────
            ClientMsg::SetProjectChildJobs {
                project_dir,
                child_jobs,
            } => {
                let mut s = state.lock().await;
                match s.config.set_project_child_jobs(&project_dir, child_jobs) {
                    Ok(_) => {
                        send(
                            &mut writer,
                            &DaemonMsg::ConfigUpdated {
                                message: format!(
                                    "child_jobs for '{}' set to {}",
                                    project_dir, child_jobs
                                ),
                            },
                        )
                        .await?
                    }
                    Err(e) => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Config save failed: {}", e),
                            },
                        )
                        .await?
                    }
                }
            }

            // ── Config: set slot count ────────────────────────────────────────
            ClientMsg::SetSlots { slots } => {
                let mut s = state.lock().await;
                match s.config.set_slots(slots) {
                    Ok(_) => {
                        let effective_slots = s.config.slots;
                        drop(s);
                        notify.notify_one();
                        send(
                            &mut writer,
                            &DaemonMsg::ConfigUpdated {
                                message: format!(
                                    "Slots set to {} (effective immediately)",
                                    slot_limit_label(effective_slots),
                                ),
                            },
                        )
                        .await?
                    }
                    Err(e) => {
                        send(
                            &mut writer,
                            &DaemonMsg::Error {
                                message: format!("Config save failed: {}", e),
                            },
                        )
                        .await?
                    }
                }
            }

            // ── Status ────────────────────────────────────────────────────────
            ClientMsg::Status => {
                let report = state.lock().await.status_report();
                send(&mut writer, &DaemonMsg::StatusReport { report }).await?;
            }

            // ── Get config ────────────────────────────────────────────────────
            ClientMsg::GetConfig => {
                let s = state.lock().await;
                let toml = toml::to_string_pretty(&s.config).unwrap_or_default();
                drop(s);
                send(&mut writer, &DaemonMsg::ConfigText { toml }).await?;
            }

            // ── Shutdown ──────────────────────────────────────────────────────
            ClientMsg::Shutdown => {
                send(&mut writer, &DaemonMsg::ShuttingDown).await?;
                #[cfg(unix)]
                {
                    let _ = std::fs::remove_file(socket_path());
                }
                info!("Daemon shutting down on client request");
                std::process::exit(0);
            }
        }
    }

    Ok(())
}

// ─────────────────────────── Helper ──────────────────────────────────────────

async fn send<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, msg: &DaemonMsg) -> Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_job(id: &str) -> QueuedJob {
        QueuedJob {
            job_id: id.to_string(),
            project_dir: "H:\\project".to_string(),
            alias: "project".to_string(),
            args: vec!["check".to_string()],
            priority: crate::config::Priority::Normal,
            queued_at: Utc::now(),
            child_jobs: 2,
        }
    }

    #[test]
    fn zero_slots_means_unlimited_daemon_scheduling_slots() {
        let mut config = GlobalConfig::default();
        config.slots = 0;
        config.max_cpu_pct = 100.0;
        config.max_ram_pct = 100.0;

        let mut state = SharedState::new(config);
        state.active = 512;

        assert!(state.can_start_another());
    }

    #[test]
    fn positive_slots_still_cap_daemon_scheduling() {
        let mut config = GlobalConfig::default();
        config.slots = 4;
        config.max_cpu_pct = 100.0;
        config.max_ram_pct = 100.0;

        let mut state = SharedState::new(config);
        state.active = 4;

        assert!(!state.can_start_another());
    }

    #[test]
    fn status_report_updates_running_elapsed_time() {
        let config = GlobalConfig::default();
        let mut state = SharedState::new(config);
        let (kill_tx, _kill_rx) = tokio::sync::oneshot::channel::<()>();

        state.running.insert(
            "job-1".to_string(),
            RunningEntry {
                job: RunningJob {
                    job_id: "job-1".to_string(),
                    project_dir: "H:\\project".to_string(),
                    alias: "project".to_string(),
                    args: vec!["check".to_string()],
                    pid: 123,
                    started_at: Utc::now() - chrono::Duration::milliseconds(1250),
                    elapsed_ms: 0,
                },
                kill_tx: Some(kill_tx),
            },
        );

        let report = state.status_report();

        assert_eq!(report.running.len(), 1);
        assert!(report.running[0].elapsed_ms >= 1000);
    }

    #[test]
    fn kill_request_for_running_job_does_not_decrement_active_slot() {
        let mut state = SharedState::new(GlobalConfig::default());
        let (kill_tx, _kill_rx) = tokio::sync::oneshot::channel::<()>();

        state.active = 1;
        state.running.insert(
            "job-1".to_string(),
            RunningEntry {
                job: RunningJob {
                    job_id: "job-1".to_string(),
                    project_dir: "H:\\project".to_string(),
                    alias: "project".to_string(),
                    args: vec!["check".to_string()],
                    pid: 123,
                    started_at: Utc::now(),
                    elapsed_ms: 0,
                },
                kill_tx: Some(kill_tx),
            },
        );

        assert!(matches!(
            state.request_kill_job("job-1"),
            KillJobOutcome::SignaledRunning
        ));
        assert_eq!(state.active, 1);
        assert!(state.running.contains_key("job-1"));
        assert!(matches!(
            state.request_kill_job("job-1"),
            KillJobOutcome::AlreadyRequested
        ));
    }

    #[test]
    fn queued_job_operations_accept_unique_prefixes() {
        let mut state = SharedState::new(GlobalConfig::default());
        let full_id = "12345678-aaaa-bbbb-cccc-123456789abc";
        state.queue.push(queued_job(full_id));

        match state.set_queued_job_priority("12345678", crate::config::Priority::High) {
            SetPriorityOutcome::Changed { job_id, .. } => assert_eq!(job_id, full_id),
            _ => panic!("unique prefix should reprioritize queued job"),
        }

        match state.cancel_queued_job("12345678") {
            CancelJobOutcome::Cancelled(job_id) => assert_eq!(job_id, full_id),
            _ => panic!("unique prefix should cancel queued job"),
        }
    }

    #[test]
    fn ambiguous_job_prefixes_are_rejected() {
        let mut state = SharedState::new(GlobalConfig::default());
        state
            .queue
            .push(queued_job("12345678-aaaa-bbbb-cccc-123456789abc"));
        state
            .queue
            .push(queued_job("12345678-dddd-eeee-ffff-123456789abc"));

        assert!(matches!(
            state.set_queued_job_priority("12345678", crate::config::Priority::High),
            SetPriorityOutcome::Ambiguous
        ));
        assert!(matches!(
            state.cancel_queued_job("12345678"),
            CancelJobOutcome::Ambiguous
        ));
        assert!(matches!(
            state.request_kill_job("12345678"),
            KillJobOutcome::Ambiguous
        ));
    }
}
