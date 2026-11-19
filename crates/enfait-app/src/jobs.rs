//! Queue of animation jobs, saved in the session: it survives a restart
//! (an interrupted job goes back to the queue).

use enfait_ai::Quality;
use enfait_core::BlockId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobKind {
    /// Create (or recreate) the animation of the block.
    Create,
    /// Retouch the animation: (time in s from the start of the block, remark).
    Revise { notes: Vec<(f64, String)> },
    /// Go back to a previous version (no model call).
    Restore { version: u32 },
    /// Create the transition to the next block.
    Transition,
}

impl JobKind {
    /// Jobs of the same family touch the same files: one at a time per block,
    /// and no duplicate in the queue.
    fn family(&self) -> u8 {
        match self {
            Self::Create | Self::Revise { .. } | Self::Restore { .. } => 0,
            Self::Transition => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "message", rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub block: BlockId,
    pub kind: JobKind,
    pub quality: Quality,
    pub status: JobStatus,
    /// When it was queued, started and finished (seconds since the Unix
    /// epoch): progress and estimates of the Montage screen.
    #[serde(default)]
    pub queued_at: u64,
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub finished_at: Option<u64>,
}

impl Job {
    /// Seconds it took (finished jobs).
    pub fn took(&self) -> Option<u64> {
        Some(self.finished_at?.saturating_sub(self.started_at?))
    }
}

fn now() -> u64 {
    enfait_config::projects::now()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    next_id: u64,
    jobs: Vec<Job>,
}

#[derive(Debug)]
pub struct JobQueue {
    path: PathBuf,
    next_id: u64,
    jobs: Vec<Job>,
}

/// Finished jobs kept in the history.
const KEEP_FINISHED: usize = 200;

impl JobQueue {
    /// Reads the queue; jobs running when enfait stopped go back to the
    /// queue.
    pub fn load(path: &Path) -> Self {
        let saved: Saved = std::fs::read(path)
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or_default();
        let mut queue = Self {
            path: path.to_path_buf(),
            next_id: saved.next_id,
            jobs: saved.jobs,
        };
        for job in &mut queue.jobs {
            if job.status == JobStatus::Running {
                job.status = JobStatus::Queued;
                job.started_at = None;
            }
        }
        queue
    }

    pub fn save(&self) {
        let saved = Saved {
            next_id: self.next_id,
            jobs: self.jobs.clone(),
        };
        let result = serde_json::to_vec_pretty(&saved)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let tmp = self.path.with_extension("tmp");
                std::fs::write(&tmp, json)?;
                std::fs::rename(&tmp, &self.path)
            });
        if let Err(e) = result {
            tracing::warn!("file de jobs : {e}");
        }
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    /// Adds a job. A creation already queued for this block is not
    /// duplicated; queued retouches are grouped.
    pub fn push(&mut self, block: BlockId, kind: JobKind, quality: Quality) -> u64 {
        let family = kind.family();
        if let Some(job) = self.jobs.iter_mut().find(|j| {
            j.block == block && j.status == JobStatus::Queued && j.kind.family() == family
        }) {
            match (&mut job.kind, kind) {
                (JobKind::Create, JobKind::Create) => {}
                (JobKind::Revise { notes }, JobKind::Revise { notes: more }) => notes.extend(more),
                (slot, kind) => *slot = kind,
            }
            job.quality = quality;
            let id = job.id;
            self.save();
            return id;
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.jobs.push(Job {
            id,
            block,
            kind,
            quality,
            status: JobStatus::Queued,
            queued_at: now(),
            started_at: None,
            finished_at: None,
        });
        self.trim();
        self.save();
        id
    }

    /// Next queued job, marked running; a job whose block (same family) is
    /// already running waits for its turn.
    pub fn start_next(&mut self) -> Option<Job> {
        let busy: Vec<(BlockId, u8)> = self
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Running)
            .map(|j| (j.block, j.kind.family()))
            .collect();
        let job = self.jobs.iter_mut().find(|j| {
            j.status == JobStatus::Queued && !busy.contains(&(j.block, j.kind.family()))
        })?;
        job.status = JobStatus::Running;
        job.started_at = Some(now());
        let job = job.clone();
        self.save();
        Some(job)
    }

    pub fn finish(&mut self, id: u64, status: JobStatus) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = status;
            job.finished_at = Some(now());
        }
        self.save();
    }

    /// Cancels queued jobs (of a block, or all).
    pub fn cancel_queued(&mut self, block: Option<BlockId>) {
        for job in &mut self.jobs {
            if job.status == JobStatus::Queued && block.is_none_or(|b| b == job.block) {
                job.status = JobStatus::Cancelled;
            }
        }
        self.save();
    }

    /// Last animation job of a block.
    pub fn latest(&self, block: BlockId) -> Option<&Job> {
        self.jobs
            .iter()
            .rev()
            .find(|j| j.block == block && j.kind.family() == 0)
    }

    /// Last job of the transition after a block.
    pub fn latest_transition(&self, block: BlockId) -> Option<&Job> {
        self.jobs
            .iter()
            .rev()
            .find(|j| j.block == block && j.kind == JobKind::Transition)
    }

    /// (running, queued).
    pub fn counts(&self) -> (usize, usize) {
        let running = self
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Running)
            .count();
        let queued = self
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Queued)
            .count();
        (running, queued)
    }

    fn trim(&mut self) {
        let finished = |j: &Job| !matches!(j.status, JobStatus::Queued | JobStatus::Running);
        let excess = self
            .jobs
            .iter()
            .filter(|j| finished(j))
            .count()
            .saturating_sub(KEEP_FINISHED);
        let mut removed = 0;
        self.jobs.retain(|j| {
            if removed < excess && finished(j) {
                removed += 1;
                false
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_survives_restart_and_merges() {
        let dir = std::env::temp_dir().join(format!("enfait-jobs-{}", std::process::id()));
        let path = dir.join("jobs.json");
        let (a, b) = (BlockId::new(), BlockId::new());
        let mut q = JobQueue::load(&path);
        let first = q.push(a, JobKind::Create, Quality::Fast);
        assert_eq!(
            q.push(a, JobKind::Create, Quality::Polished),
            first,
            "no duplicate"
        );
        q.push(
            b,
            JobKind::Revise {
                notes: vec![(1.0, "slower".into())],
            },
            Quality::Fast,
        );
        q.push(
            b,
            JobKind::Revise {
                notes: vec![(2.0, "en rouge".into())],
            },
            Quality::Fast,
        );
        assert_eq!(q.jobs().len(), 2);
        let JobKind::Revise { notes } = &q.jobs()[1].kind else {
            panic!()
        };
        assert_eq!(notes.len(), 2, "retouches grouped");

        // A transition of the same block does not mix with its animation.
        q.push(a, JobKind::Transition, Quality::Fast);
        assert_eq!(q.jobs().len(), 3);
        let running = q.start_next().unwrap();
        assert_eq!((running.block, running.quality), (a, Quality::Polished));
        q.push(a, JobKind::Create, Quality::Fast);
        // `a`'s animation runs: move on to `b`, then to `a`'s transition; the
        // new creation of `a` waits for the first one to finish.
        assert_eq!(q.start_next().unwrap().block, b);
        assert_eq!(q.start_next().unwrap().kind, JobKind::Transition);
        assert!(q.start_next().is_none());
        q.finish(running.id, JobStatus::Done);
        let again = q.start_next().unwrap();
        assert_eq!((again.block, again.kind.clone()), (a, JobKind::Create));

        // Hard stop: running jobs go back to the queue.
        let mut q = JobQueue::load(&path);
        assert_eq!(q.counts(), (0, 3));
        while let Some(job) = q.start_next() {
            let status = if job.kind == JobKind::Create {
                JobStatus::Failed("quota".into())
            } else {
                JobStatus::Done
            };
            q.finish(job.id, status);
        }
        assert_eq!(q.counts(), (0, 0));
        assert_eq!(
            q.latest(a).unwrap().status,
            JobStatus::Failed("quota".into())
        );
        assert_eq!(q.latest_transition(a).unwrap().status, JobStatus::Done);
        let id = q.push(a, JobKind::Restore { version: 1 }, Quality::Fast);
        assert!(id > again.id, "identifiants toujours nouveaux");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
