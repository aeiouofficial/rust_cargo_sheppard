// src/config.rs
// Persistent configuration for cargo-shepherd.
// Stored as TOML in the platform-correct config directory:
//   Windows : %APPDATA%\shepherd\config.toml
//   macOS   : ~/Library/Application Support/shepherd/config.toml
//   Linux   : ~/.config/shepherd/config.toml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ─────────────────────────── Priority ────────────────────────────────────────

/// Five levels so there's room to be meaningful about ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Background = 0, // only runs when all slots would otherwise be empty
    Low = 1,
    Normal = 2, // default
    High = 3,
    Critical = 4, // jumps every other job — use for cargo run
}

impl Priority {
    pub fn from_u8(n: u8) -> Self {
        match n {
            0 => Priority::Background,
            1 => Priority::Low,
            3 => Priority::High,
            4 => Priority::Critical,
            _ => Priority::Normal,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            Priority::Background => "BG",
            Priority::Low => "LOW",
            Priority::Normal => "NORM",
            Priority::High => "HIGH",
            Priority::Critical => "CRIT",
        }
    }

    pub fn raised(self) -> Self {
        Priority::from_u8((self.as_u8() + 1).min(4))
    }

    pub fn lowered(self) -> Self {
        Priority::from_u8(self.as_u8().saturating_sub(1))
    }
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ─────────────────────────── Per-project settings ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Absolute, canonicalized path to the project root (Cargo.toml directory).
    pub path: String,

    /// Short display name shown in the TUI and status output.
    /// Defaults to the directory name if not set.
    pub alias: Option<String>,

    /// Default priority for all builds from this project.
    #[serde(default)]
    pub priority: Priority,

    /// Override the global `child_jobs` setting just for this project.
    /// Useful if one project is huge and you want to give it more threads.
    pub child_jobs: Option<usize>,
}

impl ProjectConfig {
    pub fn display_name(&self) -> String {
        self.alias.clone().unwrap_or_else(|| {
            PathBuf::from(&self.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| self.path.clone())
        })
    }
}

// ─────────────────────────── Global config ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// How many cargo builds can run simultaneously.
    /// 0 means unlimited job-level concurrency; resource gates still apply.
    pub slots: usize,

    /// Halt scheduling new jobs if CPU exceeds this percentage.
    pub max_cpu_pct: f32,

    /// Halt scheduling new jobs if RAM exceeds this percentage.
    pub max_ram_pct: f64,

    /// CARGO_BUILD_JOBS for child cargo processes (rustc thread count).
    /// Default: 2 — shepherd controls JOB concurrency; this controls THREAD concurrency per job.
    pub child_jobs: usize,

    /// Log level passed to RUST_LOG. Default: "info"
    pub log_level: String,

    /// How often the TUI polls the daemon for updates (milliseconds).
    pub ui_refresh_ms: u64,

    /// Per-project overrides. Matched by canonicalized path prefix.
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            slots: 0,
            max_cpu_pct: 80.0,
            max_ram_pct: 85.0,
            child_jobs: 2,
            log_level: "info".into(),
            ui_refresh_ms: 500,
            projects: Vec::new(),
        }
    }
}

impl GlobalConfig {
    // ── Path helpers ──────────────────────────────────────────────────────────

    /// Platform-correct path to the config file.
    pub fn config_path() -> Result<PathBuf> {
        let base = dirs::config_dir().context("Cannot determine config directory for this OS")?;
        Ok(base.join("shepherd").join("config.toml"))
    }

    // ── Load / Save ───────────────────────────────────────────────────────────

    /// Load config from disk, creating a default one if none exists.
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            let default = Self::default();
            default.save()?;
            return Ok(default);
        }

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read config: {}", path.display()))?;

        let mut config: Self = toml::from_str(&raw)
            .with_context(|| format!("Invalid config at {}: check TOML syntax", path.display()))?;
        config.normalize_loaded_values();
        Ok(config)
    }

    /// Write config to disk, creating parent directories as needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create config dir: {}", parent.display()))?;
        }

        let toml_str =
            toml::to_string_pretty(self).context("Failed to serialize config to TOML")?;

        std::fs::write(&path, toml_str)
            .with_context(|| format!("Cannot write config: {}", path.display()))?;

        Ok(())
    }

    // ── Project lookup ────────────────────────────────────────────────────────

    /// Find the most specific per-project config for a given directory path.
    /// Tries exact match first, then canonicalized path-prefix matches.
    pub fn project_for(&self, project_dir: &str) -> Option<&ProjectConfig> {
        let canonical = std::fs::canonicalize(project_dir).ok()?;

        self.projects
            .iter()
            .filter_map(|project| {
                let project_path = std::fs::canonicalize(&project.path).ok()?;
                if canonical == project_path || canonical.starts_with(&project_path) {
                    Some((project, project_path.components().count()))
                } else {
                    None
                }
            })
            .max_by_key(|(_, depth)| *depth)
            .map(|(project, _)| project)
    }

    /// Priority for a given directory (falls back to Normal).
    pub fn priority_for(&self, project_dir: &str) -> Priority {
        self.project_for(project_dir)
            .map(|p| p.priority)
            .unwrap_or(Priority::Normal)
    }

    /// Display name for a given directory.
    pub fn alias_for(&self, project_dir: &str) -> String {
        self.project_for(project_dir)
            .map(|p| p.display_name())
            .unwrap_or_else(|| {
                PathBuf::from(project_dir)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| project_dir.to_string())
            })
    }

    /// child_jobs for a given directory (falls back to global).
    pub fn child_jobs_for(&self, project_dir: &str) -> usize {
        self.project_for(project_dir)
            .and_then(|p| p.child_jobs)
            .unwrap_or(self.child_jobs)
    }

    // ── Mutations (saved immediately) ─────────────────────────────────────────

    /// Set the default priority for a project, creating a ProjectConfig entry if needed.
    pub fn set_project_priority(&mut self, project_dir: &str, priority: Priority) -> Result<()> {
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| PathBuf::from(project_dir));
        let path_str = canonical.to_string_lossy().to_string();

        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path_str) {
            p.priority = priority;
        } else {
            self.projects.push(ProjectConfig {
                path: path_str,
                alias: None,
                priority,
                child_jobs: None,
            });
        }

        self.save()
    }

    /// Set an alias for a project directory.
    pub fn set_project_alias(&mut self, project_dir: &str, alias: &str) -> Result<()> {
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| PathBuf::from(project_dir));
        let path_str = canonical.to_string_lossy().to_string();

        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path_str) {
            p.alias = Some(alias.to_string());
        } else {
            self.projects.push(ProjectConfig {
                path: path_str,
                alias: Some(alias.to_string()),
                priority: Priority::Normal,
                child_jobs: None,
            });
        }

        self.save()
    }

    pub fn set_slots(&mut self, slots: usize) -> Result<()> {
        self.slots = normalize_slots(slots);
        self.save()
    }

    /// Set the per-project child_jobs (CARGO_BUILD_JOBS), creating a ProjectConfig entry if needed.
    pub fn set_project_child_jobs(&mut self, project_dir: &str, child_jobs: usize) -> Result<()> {
        if child_jobs == 0 {
            anyhow::bail!("child_jobs must be at least 1");
        }

        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| PathBuf::from(project_dir));
        let path_str = canonical.to_string_lossy().to_string();

        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path_str) {
            p.child_jobs = Some(child_jobs);
        } else {
            self.projects.push(ProjectConfig {
                path: path_str,
                alias: None,
                priority: Priority::Normal,
                child_jobs: Some(child_jobs),
            });
        }

        self.save()
    }

    fn normalize_loaded_values(&mut self) {
        self.slots = normalize_slots(self.slots);
        self.child_jobs = normalize_child_jobs(self.child_jobs);
        for project in &mut self.projects {
            if let Some(child_jobs) = project.child_jobs {
                project.child_jobs = Some(normalize_child_jobs(child_jobs));
            }
        }
    }
}

// ─────────────────────────── Helpers ─────────────────────────────────────────

pub fn normalize_slots(slots: usize) -> usize {
    slots
}

pub fn normalize_child_jobs(child_jobs: usize) -> usize {
    child_jobs.max(1)
}

pub fn slot_limit_label(slots: usize) -> String {
    if slots == 0 {
        "unlimited".to_string()
    } else {
        slots.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_slots_are_unlimited() {
        assert_eq!(GlobalConfig::default().slots, 0);
    }

    #[test]
    fn normalize_slots_keeps_zero_as_unlimited() {
        assert_eq!(normalize_slots(0), 0);
        assert_eq!(normalize_slots(1), 1);
        assert_eq!(normalize_slots(8), 8);
    }

    #[test]
    fn normalize_child_jobs_keeps_cargo_thread_count_positive() {
        assert_eq!(normalize_child_jobs(0), 1);
        assert_eq!(normalize_child_jobs(4), 4);
    }

    #[test]
    fn project_lookup_uses_most_specific_prefix_match() -> Result<()> {
        let unique = format!(
            "cargo-shepherd-config-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let workspace = root.join("workspace");
        let member = workspace.join("member");
        let nested = member.join("src");
        std::fs::create_dir_all(&nested)?;

        let mut config = GlobalConfig::default();
        config.projects.push(ProjectConfig {
            path: workspace.to_string_lossy().to_string(),
            alias: Some("workspace".to_string()),
            priority: Priority::Low,
            child_jobs: Some(2),
        });
        config.projects.push(ProjectConfig {
            path: member.to_string_lossy().to_string(),
            alias: Some("member".to_string()),
            priority: Priority::High,
            child_jobs: Some(4),
        });

        assert_eq!(
            config.alias_for(nested.to_string_lossy().as_ref()),
            "member"
        );
        assert_eq!(
            config.priority_for(nested.to_string_lossy().as_ref()),
            Priority::High
        );
        assert_eq!(config.child_jobs_for(nested.to_string_lossy().as_ref()), 4);

        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
