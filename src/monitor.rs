// src/monitor.rs
// System resource monitor — gates new build scheduling on CPU/RAM headroom.

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

pub struct ResourceMonitor {
    sys:          System,
    last_cpu:     f32,
    last_ram_pct: f64,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self { sys, last_cpu: 0.0, last_ram_pct: 0.0 }
    }

    /// Refresh the internal counters. Call before reading CPU/RAM.
    pub fn refresh(&mut self) {
        self.sys.refresh_cpu_all();
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

    pub fn cpu_usage(&self)     -> f32  { self.last_cpu }
    pub fn ram_usage_pct(&self) -> f64  { self.last_ram_pct }

    /// Returns true when it's safe to start another build.
    /// Thresholds are supplied by the caller (from config) so they can be
    /// changed live without restarting the daemon.
    pub fn can_start_build(&self, max_cpu: f32, max_ram: f64) -> bool {
        self.last_cpu < max_cpu && self.last_ram_pct < max_ram
    }

    pub fn summary(&self) -> String {
        format!("CPU {:.1}%  RAM {:.1}%", self.last_cpu, self.last_ram_pct)
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self { Self::new() }
}
