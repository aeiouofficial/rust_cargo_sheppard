// src/main.rs
// cargo-shepherd — system-wide Cargo build coordinator
// v0.2.0 — priority queue, TUI dashboard, persistent config

mod client;
mod config;
mod daemon;
mod ipc;
mod monitor;
mod queue;
mod runner;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use uuid::Uuid;

use crate::client::ShepherdClient;
use crate::config::{GlobalConfig, Priority};
use crate::ipc::{ClientMsg, DaemonMsg};

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name    = "shepherd",
    about   = "🐑 cargo-shepherd — system-wide Cargo build coordinator",
    version = "0.2.0",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the shepherd daemon (keep this running in a dedicated terminal)
    Daemon {
        /// Max concurrent builds (default: cpu_count / 2, min 1)
        #[arg(short, long)]
        slots: Option<usize>,
    },

    /// Queue a cargo command through the daemon.
    /// Example: shepherd run -- build --release
    Run {
        #[arg(last = true, required = true)]
        args: Vec<String>,

        /// Project directory (defaults to current working directory)
        #[arg(short, long)]
        dir: Option<String>,

        /// Priority: background | low | normal | high | critical
        #[arg(short, long, default_value = "normal")]
        priority: String,
    },

    /// Show all running and queued builds across all projects
    Status,

    /// Open the interactive TUI dashboard
    Tui,

    /// Kill or cancel builds
    Kill {
        /// Kill ALL builds (running + queued) for a project directory
        #[arg(long)]
        project: Option<String>,

        /// Kill or cancel a specific job by ID
        #[arg(long)]
        job: Option<String>,
    },

    /// Cancel a queued job (before it starts). Use 'kill --job' for running jobs.
    Cancel {
        job_id: String,
    },

    /// Change priority of a queued job live
    Reprioritize {
        job_id: String,

        /// New priority: background | low | normal | high | critical
        priority: String,
    },

    /// Persistent config commands (saved to disk, survive daemon restart)
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Print the path to the config file
    ConfigPath,

    /// Shut down the daemon
    Stop,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Show current config as TOML
    Show,

    /// Set global concurrent build slot count
    Slots { count: usize },

    /// Set the default priority for a project directory
    Priority {
        /// Project directory (default: current dir)
        #[arg(short, long)]
        dir: Option<String>,

        /// Priority level: background | low | normal | high | critical
        priority: String,
    },

    /// Set a short display name for a project
    Alias {
        /// Project directory (default: current dir)
        #[arg(short, long)]
        dir: Option<String>,

        alias: String,
    },

    /// Set per-project rustc thread count (CARGO_BUILD_JOBS for that project)
    ChildJobs {
        /// Project directory (default: current dir)
        #[arg(short, long)]
        dir: Option<String>,

        jobs: usize,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Cmd::Daemon { slots } => cmd_daemon(slots).await,
        Cmd::Run { args, dir, priority } => cmd_run(args, dir, priority).await,
        Cmd::Status => cmd_status().await,
        Cmd::Tui => cmd_tui().await,
        Cmd::Kill { project, job } => cmd_kill(project, job).await,
        Cmd::Cancel { job_id } => cmd_cancel(job_id).await,
        Cmd::Reprioritize { job_id, priority } => cmd_reprioritize(job_id, priority).await,
        Cmd::Config(sub) => cmd_config(sub).await,
        Cmd::ConfigPath => {
            println!("{}", GlobalConfig::config_path()?.display());
            Ok(())
        }
        Cmd::Stop => cmd_stop().await,
    }
}

// ── Command implementations ───────────────────────────────────────────────────

async fn cmd_daemon(slots: Option<usize>) -> Result<()> {
    // Init tracing
    let config = GlobalConfig::load()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(config.log_level.parse().unwrap_or_else(|_| "info".parse().unwrap())),
        )
        .init();

    let effective_slots = slots
        .unwrap_or(config.slots)
        .max(1);

    let mut live_config = config;
    live_config.slots = effective_slots;

    println!(
        "{} {} build slot(s)  |  config: {}",
        "🐑 cargo-shepherd daemon starting —".cyan().bold(),
        effective_slots.to_string().yellow().bold(),
        GlobalConfig::config_path()?.display().to_string().dimmed(),
    );

    daemon::Daemon::run(live_config).await
}

async fn cmd_run(args: Vec<String>, dir: Option<String>, priority_str: String) -> Result<()> {
    let priority   = parse_priority(&priority_str)?;
    let project_dir = resolve_dir(dir)?;
    let job_id     = Uuid::new_v4().to_string();

    let mut client = ShepherdClient::connect().await?;

    let msg = ClientMsg::Run {
        job_id:      job_id.clone(),
        project_dir: project_dir.clone(),
        args:        args.clone(),
        priority:    Some(priority),
    };

    match client.send_recv(&msg).await? {
        DaemonMsg::Queued { job_id, position } => {
            println!(
                "{} queued at position {}  ({})  [{}]",
                "✔".green().bold(),
                (position + 1).to_string().yellow(),
                format!("cargo {}", args.join(" ")).cyan(),
                &job_id[..8].dimmed(),
            );
        }
        DaemonMsg::Error { message } => {
            eprintln!("{} {}", "✗".red().bold(), message);
            std::process::exit(1);
        }
        other => eprintln!("Unexpected response: {:?}", other),
    }

    Ok(())
}

async fn cmd_status() -> Result<()> {
    let mut client = ShepherdClient::connect().await?;

    match client.send_recv(&ClientMsg::Status).await? {
        DaemonMsg::StatusReport { report } => {
            println!(
                "\n{}  slots {}/{}  CPU {:.1}%  RAM {:.1}%",
                "═══ cargo-shepherd ═══".cyan().bold(),
                report.slots_active,
                report.slots_total,
                report.cpu_pct,
                report.ram_pct,
            );

            if report.running.is_empty() && report.queued.is_empty() {
                println!("{}", "  No active or queued builds.\n".dimmed());
                return Ok(());
            }

            if !report.running.is_empty() {
                println!("\n{}", "  ● RUNNING".green().bold());
                for job in &report.running {
                    let elapsed = job.elapsed_ms / 1000;
                    println!(
                        "  {:>36}  {}  PID:{:<7}  {}s",
                        job.job_id[..8].dimmed(),
                        format!("cargo {}", job.args.join(" ")).cyan(),
                        job.pid.to_string().dimmed(),
                        elapsed,
                    );
                    println!("  {:>36}  {}", "", format!("[{}]", job.alias).yellow());
                }
            }

            if !report.queued.is_empty() {
                println!("\n{}", "  ○ QUEUED".yellow().bold());
                for job in &report.queued {
                    let prio_colored = match job.priority {
                        Priority::Critical   => job.priority.label().red().bold().to_string(),
                        Priority::High       => job.priority.label().yellow().bold().to_string(),
                        Priority::Normal     => job.priority.label().normal().to_string(),
                        Priority::Low        => job.priority.label().dimmed().to_string(),
                        Priority::Background => job.priority.label().dimmed().to_string(),
                    };
                    println!(
                        "  {:>36}  [{}]  {}. {}  {}",
                        job.job_id[..8].dimmed(),
                        prio_colored,
                        job.position + 1,
                        format!("cargo {}", job.args.join(" ")).cyan(),
                        format!("[{}]", job.alias).yellow(),
                    );
                }
            }

            println!();
        }
        DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
        other => eprintln!("Unexpected: {:?}", other),
    }

    Ok(())
}

async fn cmd_tui() -> Result<()> {
    let config = GlobalConfig::load()?;
    tui::run_tui(config.ui_refresh_ms).await
}

async fn cmd_kill(project: Option<String>, job: Option<String>) -> Result<()> {
    let msg = if let Some(proj) = project {
        let dir = if proj == "." { resolve_dir(None)? } else { proj };
        ClientMsg::KillProject { project_dir: dir }
    } else if let Some(id) = job {
        ClientMsg::KillJob { job_id: id }
    } else {
        eprintln!("Specify --project <dir> or --job <id>");
        std::process::exit(1);
    };

    let mut client = ShepherdClient::connect().await?;
    match client.send_recv(&msg).await? {
        DaemonMsg::Killed { description } => println!("{} {}", "✔".green(), description),
        DaemonMsg::Error  { message }     => eprintln!("{} {}", "✗".red(), message),
        other => eprintln!("Unexpected: {:?}", other),
    }

    Ok(())
}

async fn cmd_cancel(job_id: String) -> Result<()> {
    let mut client = ShepherdClient::connect().await?;
    match client.send_recv(&ClientMsg::CancelJob { job_id }).await? {
        DaemonMsg::Killed { description } => println!("{} {}", "✔".green(), description),
        DaemonMsg::Error  { message }     => eprintln!("{} {}", "✗".red(), message),
        other => eprintln!("Unexpected: {:?}", other),
    }
    Ok(())
}

async fn cmd_reprioritize(job_id: String, priority_str: String) -> Result<()> {
    let new_priority = parse_priority(&priority_str)?;
    let mut client   = ShepherdClient::connect().await?;

    match client.send_recv(&ClientMsg::SetJobPriority { job_id, new_priority }).await? {
        DaemonMsg::PriorityChanged { new_priority, new_position, .. } => {
            println!(
                "{} Priority → {}  (position {})",
                "✔".green(),
                new_priority.label().yellow(),
                new_position + 1,
            );
        }
        DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
        other => eprintln!("Unexpected: {:?}", other),
    }

    Ok(())
}

async fn cmd_config(sub: ConfigCmd) -> Result<()> {
    match sub {
        ConfigCmd::Show => {
            let mut client = ShepherdClient::connect().await?;
            match client.send_recv(&ClientMsg::GetConfig).await? {
                DaemonMsg::ConfigText { toml } => println!("{}", toml),
                _ => {
                    // Daemon not running — read from disk
                    let cfg = GlobalConfig::load()?;
                    println!("{}", toml::to_string_pretty(&cfg)?);
                }
            }
        }

        ConfigCmd::Slots { count } => {
            let mut client = ShepherdClient::connect().await?;
            match client.send_recv(&ClientMsg::SetSlots { slots: count }).await? {
                DaemonMsg::ConfigUpdated { message } => println!("{} {}", "✔".green(), message),
                DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
                _ => {}
            }
        }

        ConfigCmd::Priority { dir, priority } => {
            let project_dir = resolve_dir(dir)?;
            let p           = parse_priority(&priority)?;
            let mut client  = ShepherdClient::connect().await?;
            match client.send_recv(&ClientMsg::SetProjectPriority { project_dir, priority: p }).await? {
                DaemonMsg::ConfigUpdated { message } => println!("{} {}", "✔".green(), message),
                DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
                _ => {}
            }
        }

        ConfigCmd::Alias { dir, alias } => {
            let project_dir = resolve_dir(dir)?;
            let mut client  = ShepherdClient::connect().await?;
            match client.send_recv(&ClientMsg::SetProjectAlias { project_dir, alias }).await? {
                DaemonMsg::ConfigUpdated { message } => println!("{} {}", "✔".green(), message),
                DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
                _ => {}
            }
        }

        ConfigCmd::ChildJobs { dir, jobs } => {
            let project_dir = resolve_dir(dir)?;
            let mut client  = ShepherdClient::connect().await?;
            match client.send_recv(&ClientMsg::SetProjectChildJobs { project_dir, child_jobs: jobs }).await? {
                DaemonMsg::ConfigUpdated { message } => println!("{} {}", "✔".green(), message),
                DaemonMsg::Error { message } => eprintln!("{} {}", "✗".red(), message),
                _ => {}
            }
        }
    }

    Ok(())
}

async fn cmd_stop() -> Result<()> {
    let mut client = ShepherdClient::connect().await?;
    let _ = client.send_recv(&ClientMsg::Shutdown).await;
    println!("{} Daemon stopped.", "✔".green());
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_dir(dir: Option<String>) -> Result<String> {
    let path = match dir {
        Some(d) => std::path::PathBuf::from(d),
        None    => std::env::current_dir()?,
    };
    Ok(path.to_string_lossy().to_string())
}

fn parse_priority(s: &str) -> Result<Priority> {
    match s.to_lowercase().as_str() {
        "background" | "bg" | "0" => Ok(Priority::Background),
        "low"  | "l" | "1"        => Ok(Priority::Low),
        "normal" | "n" | "2"      => Ok(Priority::Normal),
        "high" | "h" | "3"        => Ok(Priority::High),
        "critical" | "crit" | "c" | "4" => Ok(Priority::Critical),
        other => anyhow::bail!(
            "Unknown priority '{}'. Valid values: background, low, normal, high, critical",
            other
        ),
    }
}
