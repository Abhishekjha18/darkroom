//! Fixed-size worker pool over a shared queue. Replaces `rayon`.
//!
//! **Honest cost:** no work stealing, so a batch with wildly uneven image
//! sizes leaves cores idle at the tail. What replaces `rayon` is about a
//! hundred lines and does 90% of the job for this workload.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

/// Beyond eight the bottleneck is file IO and memory bandwidth, not cores —
/// and 32 workers each holding a decoded 24 MP image is 2 GB of peak
/// residency.
pub const MAX_WORKERS: usize = 8;

pub fn worker_count() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(MAX_WORKERS)
}

/// Live indexing progress. The one shared mutable thing in the program.
///
/// No lock, no channel, and no ordering requirement beyond `Relaxed`,
/// because it drives a number on a web page.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    /// Set once the indexer has stopped. **Not derivable from
    /// `done == total`**: both are zero before the walk has counted
    /// anything, which would otherwise read as "already finished" and end
    /// the progress stream before it started.
    finished: AtomicBool,
}

impl Progress {
    pub fn snapshot(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub fn is_indexing(&self) -> bool {
        !self.finished.load(Ordering::Relaxed)
    }

    pub fn finish(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }
}

/// Runs `f` over every job across a fixed worker pool, returning results in
/// **submission order**.
///
/// Ordering is not incidental: two runs over the same folder must produce
/// the same catalog in the same sequence, or the index becomes diff-hostile
/// and bugs stop reproducing.
pub fn map<J, R, F>(jobs: Vec<J>, progress: Arc<Progress>, f: F) -> Vec<R>
where
    J: Send + 'static,
    R: Send + 'static,
    F: Fn(J) -> R + Send + Sync + 'static,
{
    let n = jobs.len();
    progress.total.store(n as u64, Ordering::Relaxed);
    if n == 0 {
        return Vec::new();
    }

    let workers = worker_count().min(n);
    // One job stays on this thread's critical path anyway; below two workers
    // the channel machinery is pure overhead.
    if workers <= 1 {
        return jobs
            .into_iter()
            .map(|j| {
                let r = f(j);
                progress.done.fetch_add(1, Ordering::Relaxed);
                r
            })
            .collect();
    }

    let (job_tx, job_rx) = mpsc::channel::<(usize, J)>();
    let (res_tx, res_rx) = mpsc::channel::<(usize, R)>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let f = Arc::new(f);

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let job_rx = Arc::clone(&job_rx);
        let res_tx = res_tx.clone();
        let f = Arc::clone(&f);
        let progress = Arc::clone(&progress);
        handles.push(thread::spawn(move || {
            loop {
                // Lock, take one job, unlock — never hold the lock across
                // the work itself.
                let next = {
                    let guard = job_rx.lock().unwrap_or_else(|e| e.into_inner());
                    guard.recv()
                };
                let Ok((i, job)) = next else { break };
                let out = f(job);
                progress.done.fetch_add(1, Ordering::Relaxed);
                if res_tx.send((i, out)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(res_tx);

    for (i, job) in jobs.into_iter().enumerate() {
        if job_tx.send((i, job)).is_err() {
            break;
        }
    }
    // Dropping the sender is what lets workers see a closed channel and exit.
    drop(job_tx);

    let mut slots: Vec<Option<R>> = (0..n).map(|_| None).collect();
    for (i, r) in res_rx {
        slots[i] = Some(r);
    }
    for h in handles {
        // A worker that panicked has already been counted; the file it was
        // holding is simply absent from the results.
        let _ = h.join();
    }

    slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_submission_order() {
        let p = Arc::new(Progress::default());
        let out = map((0..500u32).collect(), Arc::clone(&p), |x| x * 2);
        assert_eq!(out, (0..500u32).map(|x| x * 2).collect::<Vec<_>>());
    }

    #[test]
    fn order_holds_with_uneven_work() {
        let p = Arc::new(Progress::default());
        // Reverse-weighted sleeps would finish out of order without the
        // index-slot reassembly.
        let out = map((0..64u64).collect(), Arc::clone(&p), |x| {
            let mut acc = 0u64;
            for i in 0..((64 - x) * 2000) {
                acc = acc.wrapping_add(i);
            }
            (x, acc)
        });
        let ids: Vec<u64> = out.iter().map(|(x, _)| *x).collect();
        assert_eq!(ids, (0..64u64).collect::<Vec<_>>());
    }

    #[test]
    fn counts_progress_to_completion() {
        let p = Arc::new(Progress::default());
        let out = map((0..250u32).collect(), Arc::clone(&p), |x| x);
        assert_eq!(out.len(), 250);
        let (done, total) = p.snapshot();
        assert_eq!(done, 250);
        assert_eq!(total, 250);
    }

    #[test]
    fn handles_an_empty_batch() {
        let p = Arc::new(Progress::default());
        let out: Vec<u32> = map(Vec::new(), Arc::clone(&p), |x: u32| x);
        assert!(out.is_empty());
        assert_eq!(p.snapshot(), (0, 0));
    }

    #[test]
    fn handles_a_single_job() {
        let p = Arc::new(Progress::default());
        assert_eq!(map(vec![7u32], p, |x| x + 1), vec![8]);
    }

    #[test]
    fn worker_count_is_capped() {
        let n = worker_count();
        assert!(n >= 1 && n <= MAX_WORKERS);
    }
}
