//! A [`Lane`] to share between the threads that ask and the one that makes
//! the requests.

use crate::lane::{Job, Lane, LaneConfig, Next, Priority, Queued, Submitted};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// A request that [`LaneQueue::next`] says to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent<T> {
    pub key: u64,
    pub payload: T,
    pub priority: Priority,
}

/// The requests of one endpoint, shared: any thread submits or cancels, and
/// the one that makes them waits in [`LaneQueue::next`].
pub struct LaneQueue<T> {
    lane: Mutex<Lane<T>>,
    wake: Notify,
}

impl<T> LaneQueue<T> {
    pub fn new(config: LaneConfig) -> Self {
        Self {
            lane: Mutex::new(Lane::new(config)),
            wake: Notify::new(),
        }
    }

    /// The time the lane works with. Tokio's clock, so that a test can stop
    /// and move it.
    fn now() -> Instant {
        tokio::time::Instant::now().into_std()
    }

    fn lane(&self) -> MutexGuard<'_, Lane<T>> {
        self.lane
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues `job` (see [`Lane::submit`]) and wakes whoever waits.
    pub fn submit(&self, job: Job<T>) -> Submitted {
        let submitted = self.lane().submit(job, Self::now());
        self.wake.notify_waiters();
        submitted
    }

    /// Forgets the request for `key`.
    pub fn cancel(&self, key: u64) -> bool {
        let cancelled = self.lane().cancel(key);
        if cancelled {
            self.wake.notify_waiters();
        }
        cancelled
    }

    /// Replaces the background requests with `jobs` (the plan is made
    /// again), leaving the interactive ones where they are.
    pub fn replace_background(&self, jobs: impl IntoIterator<Item = Job<T>>) {
        let now = Self::now();
        {
            let mut lane = self.lane();
            lane.clear_background();
            for job in jobs {
                lane.submit(job, now);
            }
        }
        self.wake.notify_waiters();
    }

    /// Nothing goes before `wait` from now (see [`Lane::hold_until`]).
    pub fn hold_for(&self, wait: Duration) {
        self.lane().hold_until(Self::now() + wait);
        self.wake.notify_waiters();
    }

    pub fn len(&self) -> usize {
        self.lane().len()
    }

    pub fn is_empty(&self) -> bool {
        self.lane().is_empty()
    }

    pub fn contains(&self, key: u64) -> bool {
        self.lane().contains(key)
    }

    pub fn config(&self) -> LaneConfig {
        self.lane().config()
    }

    /// How many requests of each priority wait.
    pub fn lengths(&self) -> [usize; 4] {
        self.lane().lengths()
    }

    /// The first `limit` requests in the order they will go (see
    /// [`Lane::forecast`]).
    pub fn forecast(&self, limit: usize) -> Vec<Queued>
    where
        T: Clone,
    {
        self.lane().forecast(Self::now(), limit)
    }

    /// Waits until a request may be made and returns it. Whatever is
    /// submitted, cancelled or held meanwhile is taken into account.
    pub async fn next(&self) -> Sent<T> {
        loop {
            // Registered before looking, so that nothing submitted in between
            // is missed.
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let step = self.lane().next(Self::now());
            match step {
                Next::Send {
                    key,
                    payload,
                    priority,
                } => {
                    return Sent {
                        key,
                        payload,
                        priority,
                    };
                }
                Next::Wait(wait) => {
                    let _ = tokio::time::timeout(wait, notified).await;
                }
                Next::Empty => notified.await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test(start_paused = true)]
    async fn next_waits_for_the_delay_of_a_request() {
        let queue = LaneQueue::new(LaneConfig::api());
        queue.submit(Job::new(1, "a", Priority::Interactive).after(Duration::from_millis(250)));
        let start = tokio::time::Instant::now();
        let sent = queue.next().await;
        assert_eq!(sent.key, 1);
        assert!(start.elapsed() >= Duration::from_millis(250));
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_submitted_while_waiting_wakes_the_consumer() {
        let queue = Arc::new(LaneQueue::new(LaneConfig::api()));
        let consumer = {
            let queue = Arc::clone(&queue);
            tokio::spawn(async move { queue.next().await })
        };
        tokio::time::sleep(Duration::from_secs(5)).await;
        queue.submit(Job::new(7, "late", Priority::Interactive));
        let sent = consumer.await.unwrap();
        assert_eq!((sent.key, sent.payload), (7, "late"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_hold_delays_the_next_request() {
        let queue = LaneQueue::new(LaneConfig::api());
        queue.submit(Job::new(1, (), Priority::Interactive));
        queue.hold_for(Duration::from_secs(3));
        let start = tokio::time::Instant::now();
        queue.next().await;
        assert!(start.elapsed() >= Duration::from_secs(3));
    }
}
