//! Periodic background chores that must never get in the UI's way.
//!
//! One thread (with its own single-threaded tokio runtime, like the
//! watchdog) runs a small schedule of jobs:
//!
//! * **Last login** of each linked character, read from ESI (`/online`): once
//!   at startup, as soon as a character is linked, and then every hour. The
//!   UI hears about it with [`Message::PlayerLastLogon`], only when the value
//!   changed.
//! * **Update check**: looks for a newer Telescope release at startup and then
//!   every few hours. The update dialog opens only when a release that was
//!   not announced before is found (see `UpdateChecker::announce`).
//!
//! Every failure is non-fatal: a job that fails is retried later and never
//! stops the thread. The thread never refreshes tokens (the watchdog owns
//! that, and refresh tokens must not be spent twice); it reloads them from
//! the player database before each run and retries soon when they are not
//! valid yet.

use super::TelescopeApp;
use super::messages::{CharacterSync, Message, Type, send_app_message};
use super::update_checker::UpdateChecker;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant, sleep_until};
use webb::esi::EsiManager;

/// Time between two last-login checks of a character.
const LAST_LOGON_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Retry delay when the character's token wasn't valid (the watchdog will
/// refresh it shortly).
const TOKEN_RETRY: Duration = Duration::from_secs(2 * 60);
/// Time between two update checks.
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// Retry delay after a failed update check (offline, rate limited).
const UPDATE_RETRY: Duration = Duration::from_secs(30 * 60);

/// A chore of the schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    LastLogon(i64),
    UpdateCheck,
}

/// What runs when: every job with the moment it is due.
struct Schedule {
    jobs: Vec<(Job, Instant)>,
}

impl Schedule {
    /// The update check and the last login of every `ids`, all due `now`.
    fn new(ids: &[i64], now: Instant) -> Self {
        let mut jobs = vec![(Job::UpdateCheck, now)];
        jobs.extend(ids.iter().map(|id| (Job::LastLogon(*id), now)));
        Self { jobs }
    }

    /// Applies a link/unlink request. A linked character is checked now.
    /// Returns `false` when every sender is gone and the thread must end.
    fn apply_sync(&mut self, sync: Option<CharacterSync>, now: Instant) -> bool {
        match sync {
            Some(CharacterSync::Add(id)) => self.reschedule(Job::LastLogon(id), now),
            Some(CharacterSync::Remove(id)) => {
                self.jobs.retain(|(job, _)| *job != Job::LastLogon(id));
            }
            None => return false,
        }
        true
    }

    /// Sets when `job` is due next, adding it if it isn't scheduled.
    fn reschedule(&mut self, job: Job, due: Instant) {
        match self.jobs.iter_mut().find(|(item, _)| *item == job) {
            Some(entry) => entry.1 = due,
            None => self.jobs.push((job, due)),
        }
    }

    /// The jobs due at `now`.
    fn due(&self, now: Instant) -> Vec<Job> {
        self.jobs
            .iter()
            .filter(|(_, due)| *due <= now)
            .map(|(job, _)| *job)
            .collect()
    }

    /// When the earliest job is due.
    fn next_due(&self) -> Option<Instant> {
        self.jobs.iter().map(|(_, due)| *due).min()
    }
}

/// The state of the housekeeping thread.
struct Housekeeping {
    esi: EsiManager,
    app_msg: Arc<mpsc::Sender<Message>>,
    schedule: Schedule,
    /// Last login already reported to the UI, per character.
    reported: HashMap<i64, DateTime<Utc>>,
    /// Tag of the last release announced to the UI.
    announced: Option<String>,
    /// The last update check failed (so a streak of failures warns once).
    update_failing: bool,
}

impl Housekeeping {
    /// Runs the schedule until every sender of `receiver` is gone.
    async fn run(mut self, mut receiver: mpsc::Receiver<CharacterSync>) {
        loop {
            for job in self.schedule.due(Instant::now()) {
                let delay = match job {
                    Job::LastLogon(id) => self.check_last_logon(id).await,
                    Job::UpdateCheck => self.check_update().await,
                };
                self.schedule.reschedule(job, Instant::now() + delay);
            }
            let Some(wake) = self.schedule.next_due() else {
                // No job left (not even the update check): nothing to wait for.
                return;
            };
            tokio::select! {
                message = receiver.recv() => {
                    if matches!(message, Some(CharacterSync::Add(_)))
                        && let Err(error) = self.esi.reload_auth()
                    {
                        tracing::warn!(%error, "housekeeping could not reload the tokens");
                    }
                    if !self.schedule.apply_sync(message, Instant::now()) {
                        return;
                    }
                }
                () = sleep_until(wake) => {}
            }
        }
    }

    /// Reads the last login of `id` and reports it if it changed. Returns
    /// when to check again.
    async fn check_last_logon(&mut self, id: i64) -> Duration {
        // The watchdog may have refreshed the tokens since the last run.
        if let Err(error) = self.esi.reload_auth() {
            tracing::warn!(%error, "housekeeping could not reload the tokens");
        }
        if !self.esi.valid_token(id).await {
            tracing::debug!(id, "last login: token not valid yet, retrying soon");
            return TOKEN_RETRY;
        }
        match self.esi.get_last_login(id).await {
            Ok(Some(moment)) => {
                if self.reported.insert(id, moment) != Some(moment) {
                    let _ = send_app_message(&self.app_msg, Message::PlayerLastLogon((id, moment)))
                        .await;
                }
            }
            Ok(None) => tracing::debug!(id, "ESI reports no last login"),
            Err(error) => {
                tracing::warn!(id, %error, "could not read the last login");
            }
        }
        LAST_LOGON_INTERVAL
    }

    /// Looks for a new Telescope release. Returns when to check again.
    async fn check_update(&mut self) -> Duration {
        let outcome = match UpdateChecker::for_telescope() {
            Ok(checker) => checker
                .announce(env!("CARGO_PKG_VERSION"), &mut self.announced)
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        match outcome {
            Ok(message) => {
                self.update_failing = false;
                match message {
                    Some(message) => {
                        let _ = send_app_message(&self.app_msg, message).await;
                    }
                    None => tracing::debug!("Telescope is up to date"),
                }
                UPDATE_INTERVAL
            }
            Err(error) => {
                if !self.update_failing {
                    let _ = send_app_message(
                        &self.app_msg,
                        Message::GenericNotification((
                            Type::Warning,
                            String::from("Housekeeping"),
                            String::from("check_update"),
                            format!("Could not check for a new Telescope release: {error}"),
                        )),
                    )
                    .await;
                }
                self.update_failing = true;
                UPDATE_RETRY
            }
        }
    }
}

impl TelescopeApp {
    /// Starts the housekeeping thread for the linked characters `ids` (maybe
    /// none) and keeps the channel to tell it about link/unlink requests.
    #[tracing::instrument(skip(self))]
    pub fn start_housekeeping(&mut self, ids: Vec<i64>) {
        let (sender, receiver) = mpsc::channel::<CharacterSync>(10);
        let housekeeping = Housekeeping {
            esi: self.esi.clone(),
            app_msg: Arc::clone(&self.app_msg.0),
            schedule: Schedule::new(&ids, Instant::now()),
            reported: HashMap::new(),
            announced: None,
            update_failing: false,
        };
        let spawned = thread::Builder::new()
            .name(String::from("housekeeping"))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(%error, "failed to build the housekeeping runtime");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let _span = tracing::info_span!("housekeeping").entered();
                    housekeeping.run(receiver).await;
                });
            });
        match spawned {
            Ok(_) => self.housekeeping_msg = Some(Arc::new(sender)),
            Err(error) => tracing::error!(%error, "failed to start the housekeeping thread"),
        }
    }

    /// Tells the housekeeping thread a character was linked or unlinked.
    /// Best effort: if the thread is busy or gone, the hourly pass still
    /// covers the character.
    pub(crate) fn notify_housekeeping(&self, sync: CharacterSync) {
        let _ = super::character_link::deliver(self.housekeeping_msg.as_deref(), sync);
    }

    /// Stores the last EVE login ESI reported for a character.
    #[tracing::instrument(skip(self))]
    pub(crate) fn update_player_last_logon(&mut self, player_id: i64, last_logon: DateTime<Utc>) {
        let Some(index) = self
            .esi
            .characters
            .iter()
            .position(|character| character.id == player_id)
        else {
            return;
        };
        if self.esi.characters[index].last_logon == last_logon {
            return;
        }
        self.esi.characters[index].last_logon = last_logon;
        let character = self.esi.characters[index].clone();
        if let Err(error) = self.esi.write_character(&character) {
            self.task_msg.spawn(Message::GenericNotification((
                Type::Warning,
                String::from("Housekeeping"),
                String::from("update_player_last_logon"),
                format!(
                    "Could not store the last login of {}: {error}",
                    character.name
                ),
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jobs(schedule: &Schedule, now: Instant) -> Vec<Job> {
        schedule.due(now)
    }

    #[test]
    fn everything_is_due_at_startup() {
        let now = Instant::now();
        let schedule = Schedule::new(&[1, 2], now);
        assert_eq!(
            jobs(&schedule, now),
            vec![Job::UpdateCheck, Job::LastLogon(1), Job::LastLogon(2)]
        );
    }

    #[test]
    fn a_rescheduled_job_waits_for_its_moment() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        schedule.reschedule(Job::LastLogon(1), now + LAST_LOGON_INTERVAL);
        assert_eq!(jobs(&schedule, now), vec![Job::UpdateCheck]);
        assert_eq!(
            jobs(&schedule, now + LAST_LOGON_INTERVAL),
            vec![Job::UpdateCheck, Job::LastLogon(1)]
        );
    }

    #[test]
    fn the_next_wake_up_is_the_earliest_due_job() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        schedule.reschedule(Job::UpdateCheck, now + UPDATE_INTERVAL);
        schedule.reschedule(Job::LastLogon(1), now + TOKEN_RETRY);
        assert_eq!(schedule.next_due(), Some(now + TOKEN_RETRY));
    }

    #[test]
    fn linking_a_character_checks_it_now_even_if_already_scheduled() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        schedule.reschedule(Job::LastLogon(1), now + LAST_LOGON_INTERVAL);
        assert!(schedule.apply_sync(Some(CharacterSync::Add(1)), now));
        assert!(schedule.apply_sync(Some(CharacterSync::Add(2)), now));
        let due = jobs(&schedule, now);
        assert!(due.contains(&Job::LastLogon(1)));
        assert!(due.contains(&Job::LastLogon(2)));
    }

    #[test]
    fn unlinking_a_character_forgets_only_its_job() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1, 2], now);
        assert!(schedule.apply_sync(Some(CharacterSync::Remove(1)), now));
        assert_eq!(
            jobs(&schedule, now),
            vec![Job::UpdateCheck, Job::LastLogon(2)]
        );
    }

    #[test]
    fn a_closed_channel_ends_the_thread() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        assert!(!schedule.apply_sync(None, now));
    }
}
