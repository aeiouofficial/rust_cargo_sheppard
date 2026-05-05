// src/queue.rs
// Priority queue for cargo-shepherd.
// Supports:
//   - O(log P) insert (P = number of priority levels, always ≤ 5)
//   - O(1) pop of highest-priority job
//   - O(n) reprioritization (find by job_id, remove, reinsert at new priority)
//   - FIFO ordering within the same priority level (earlier enqueue time wins)

use crate::config::Priority;
use crate::ipc::DaemonMsg;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

// ─────────────────────────── Job record ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QueuedJob {
    pub job_id: String,
    pub project_dir: String,
    pub alias: String, // display name, pre-resolved from config
    pub args: Vec<String>,
    pub priority: Priority,
    pub queued_at: DateTime<Utc>,
    pub child_jobs: usize, // CARGO_BUILD_JOBS for this specific invocation
    pub attached_tx: Option<mpsc::UnboundedSender<DaemonMsg>>,
}

// ─────────────────────────── PriorityQueue ───────────────────────────────────

/// A sorted Vec where index 0 is always the next job to run.
/// Sorted: highest priority first; within the same priority, earliest enqueue time first.
#[derive(Debug, Default)]
pub struct PriorityQueue {
    inner: Vec<QueuedJob>,
}

impl PriorityQueue {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Insert a job at the correct position to maintain sort order.
    pub fn push(&mut self, job: QueuedJob) {
        let pos = self.inner.partition_point(|existing| {
            // existing should stay before `job` when:
            // - existing has strictly higher priority, OR
            // - same priority AND existing was enqueued earlier
            existing.priority > job.priority
                || (existing.priority == job.priority && existing.queued_at <= job.queued_at)
        });
        self.inner.insert(pos, job);
    }

    /// Remove and return the next job to run (highest priority, earliest enqueue).
    pub fn pop_next(&mut self) -> Option<QueuedJob> {
        if self.inner.is_empty() {
            None
        } else {
            Some(self.inner.remove(0))
        }
    }

    /// Change the priority of a queued job and re-sort.
    /// Returns true if the job was found and updated.
    pub fn set_priority(&mut self, job_id: &str, new_priority: Priority) -> bool {
        if let Some(pos) = self.inner.iter().position(|j| j.job_id == job_id) {
            let mut job = self.inner.remove(pos);
            job.priority = new_priority;
            self.push(job);
            true
        } else {
            false
        }
    }

    /// Remove a specific job by ID (used for user-initiated cancels).
    /// Returns the removed job if found.
    pub fn remove(&mut self, job_id: &str) -> Option<QueuedJob> {
        self.inner
            .iter()
            .position(|j| j.job_id == job_id)
            .map(|pos| self.inner.remove(pos))
    }

    /// Remove all jobs belonging to a project directory.
    pub fn remove_project(&mut self, project_dir: &str) -> Vec<QueuedJob> {
        let (removed, kept): (Vec<_>, Vec<_>) = self
            .inner
            .drain(..)
            .partition(|j| j.project_dir == project_dir);
        self.inner = kept;
        removed
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Snapshot of all queued jobs in order (for status reporting).
    pub fn snapshot(&self) -> Vec<QueuedJob> {
        self.inner.clone()
    }

    /// Position of a job in the queue (0 = next up), or None if not found.
    pub fn position_of(&self, job_id: &str) -> Option<usize> {
        self.inner.iter().position(|j| j.job_id == job_id)
    }
}

// ─────────────────────────── Tests ───────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_job(id: &str, priority: Priority, offset_ms: i64) -> QueuedJob {
        QueuedJob {
            job_id: id.to_string(),
            project_dir: "/tmp/test".to_string(),
            alias: "test".to_string(),
            args: vec!["check".to_string()],
            priority,
            queued_at: Utc::now() + chrono::Duration::milliseconds(offset_ms),
            child_jobs: 2,
            attached_tx: None,
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut q = PriorityQueue::new();
        q.push(make_job("low", Priority::Low, 0));
        q.push(make_job("high", Priority::High, 100));
        q.push(make_job("norm", Priority::Normal, 50));
        q.push(make_job("crit", Priority::Critical, 200));

        assert_eq!(q.pop_next().unwrap().job_id, "crit");
        assert_eq!(q.pop_next().unwrap().job_id, "high");
        assert_eq!(q.pop_next().unwrap().job_id, "norm");
        assert_eq!(q.pop_next().unwrap().job_id, "low");
    }

    #[test]
    fn test_fifo_within_same_priority() {
        let mut q = PriorityQueue::new();
        q.push(make_job("first", Priority::Normal, 0));
        q.push(make_job("second", Priority::Normal, 10));
        q.push(make_job("third", Priority::Normal, 20));

        assert_eq!(q.pop_next().unwrap().job_id, "first");
        assert_eq!(q.pop_next().unwrap().job_id, "second");
        assert_eq!(q.pop_next().unwrap().job_id, "third");
    }

    #[test]
    fn test_reprioritize() {
        let mut q = PriorityQueue::new();
        q.push(make_job("a", Priority::Normal, 0));
        q.push(make_job("b", Priority::Low, 10));

        // b is low priority, let's bump it to critical
        assert!(q.set_priority("b", Priority::Critical));

        // now b should come first
        assert_eq!(q.pop_next().unwrap().job_id, "b");
        assert_eq!(q.pop_next().unwrap().job_id, "a");
    }

    #[test]
    fn test_remove_project() {
        let mut q = PriorityQueue::new();
        let mut job_a = make_job("a", Priority::Normal, 0);
        job_a.project_dir = "/project/foo".to_string();
        let mut job_b = make_job("b", Priority::Normal, 10);
        job_b.project_dir = "/project/bar".to_string();
        let mut job_c = make_job("c", Priority::High, 20);
        job_c.project_dir = "/project/foo".to_string();

        q.push(job_a);
        q.push(job_b);
        q.push(job_c);

        let removed = q.remove_project("/project/foo");
        assert_eq!(removed.len(), 2);
        assert_eq!(q.len(), 1);
        assert_eq!(q.pop_next().unwrap().job_id, "b");
    }
}
