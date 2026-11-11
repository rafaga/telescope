//! Periodic background chores that must never get in the UI's way.
//!
//! One thread (with its own single-threaded tokio runtime, like the
//! watchdog) runs a small schedule of jobs:
//!
//! * **Last login** of each linked character, read from ESI (`/online`): once
//!   at startup, as soon as a character is linked, and then every hour. The
//!   UI hears about it with [`Message::PlayerLastLogon`], only when the value
//!   changed.
//! * **Day counts**: hands the zKillboard source the order in which to fill
//!   in the kills of the last day of every system, in the background: first
//!   the systems around the linked characters (at most five jumps), the
//!   nearest first, then the rest of the region they are in, and last the
//!   regions next to it, with the requests the rest leaves free. The other
//!   systems are asked about when a tooltip shows them. The source does the
//!   paced requests; this only makes the plan: at startup, as soon as a
//!   character changes system (the new surroundings are the next thing
//!   asked), when the first count of the plan comes due (the ones around the
//!   characters last an hour) and, at the latest, every few minutes.
//! * **System kills**: the first reading of the heat of the maps. ESI's
//!   `universe/system_kills` gives the kills of the last hour of the whole
//!   universe in one public request, so it is asked for as soon as a map
//!   shows the heat, and then an hour after the answer it got (ESI refreshes
//!   it once an hour). The feed and the query API make up for it as their
//!   killmails come in (see `ZKillboardHandle::hour_kills`). Nothing is
//!   asked while no map shows the heat, nor while the feed covers the whole
//!   hour by itself; see [`SystemKillsPolicy`].
//! * **Wormholes**: the signatures of Thera and Turnur from EvE-Scout, kept
//!   as a layer of links (see `wormhole_links`). They are asked for only
//!   while the setting is on and a map that shows them is open, and again
//!   when the answer the API said it holds runs out (the client keeps the
//!   pace: it does not ask while an answer is valid, and it leaves an
//!   endpoint alone for a while after a bad one); see [`WormholesPolicy`].
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
use super::intel::dispatch::AlarmShared;
use super::intel::zkillboard::ZKillboardHandle;
use super::messages::{CharacterSync, Message, Type, send_app_message};
use super::update_checker::UpdateChecker;
use super::wormhole_links::WormholeShared;
use chrono::{DateTime, TimeDelta, Utc};
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
    DayCounts,
    SystemKills,
    Wormholes,
}

/// When the wormholes of EvE-Scout are asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WormholesPolicy {
    /// How often to look whether a map shows the wormholes, while none does.
    idle: Duration,
    /// The least time between two requests, whatever the API says it holds
    /// the answer for.
    least: Duration,
    /// The most time between two requests, whatever the API says.
    most: Duration,
    /// Delay before asking again after a failure that is not a pause the
    /// client announced.
    retry: Duration,
    /// Delay when the client could not be made at all.
    unavailable: Duration,
}

impl Default for WormholesPolicy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(15),
            least: Duration::from_secs(30),
            most: Duration::from_secs(60 * 60),
            retry: Duration::from_secs(2 * 60),
            unavailable: Duration::from_secs(60 * 60),
        }
    }
}

impl WormholesPolicy {
    /// When to ask again after an answer that the API holds for `max_age`
    /// (5 minutes in practice); a missing one is taken as the least.
    fn after_answer(&self, max_age: Option<Duration>) -> Duration {
        max_age.unwrap_or(self.least).clamp(self.least, self.most)
    }

    /// When to ask again after `error`: when the client says it is leaving
    /// the endpoint alone until then, or after the usual retry.
    fn after_error(&self, error: &eve_scout::Error) -> Duration {
        match error {
            eve_scout::Error::Backoff { retry_in } => (*retry_in).max(self.least),
            _ => self.retry,
        }
    }
}

/// When the hourly answer of ESI's `system_kills` is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemKillsPolicy {
    /// How often ESI refreshes the answer.
    every: TimeDelta,
    /// How long after it is due the answer is asked for, so that ESI has it.
    margin: TimeDelta,
    /// How often to look whether a map shows the heat, while none does.
    idle: Duration,
    /// Delay before asking again when the answer is still the old one.
    retry: Duration,
    /// How many times in a row the old answer is asked for again before
    /// waiting `slow_retry`.
    retries: u32,
    /// Delay after a failure, or after `retries` old answers.
    slow_retry: Duration,
}

impl Default for SystemKillsPolicy {
    fn default() -> Self {
        Self {
            every: TimeDelta::hours(1),
            margin: TimeDelta::seconds(90),
            idle: Duration::from_secs(15),
            retry: Duration::from_secs(60),
            retries: 5,
            slow_retry: Duration::from_secs(10 * 60),
        }
    }
}

impl SystemKillsPolicy {
    /// How long until the answer after the one computed at `modified` is
    /// ready, or `None` when it already is.
    fn wait(&self, modified: DateTime<Utc>, now: DateTime<Utc>) -> Option<Duration> {
        (modified + self.every + self.margin - now)
            .to_std()
            .ok()
            .filter(|wait| !wait.is_zero())
    }

    /// When to ask again after an answer computed at `modified` (if ESI
    /// said), when it was the old answer `stale` times in a row.
    fn after_answer(
        &self,
        modified: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
        stale: u32,
    ) -> Duration {
        let Some(modified) = modified else {
            return self.slow_retry;
        };
        self.wait(modified, now).unwrap_or(if stale < self.retries {
            self.retry
        } else {
            self.slow_retry
        })
    }
}

/// What runs when: every job with the moment it is due.
struct Schedule {
    jobs: Vec<(Job, Instant)>,
}

impl Schedule {
    /// The update check, the day counts, the system kills and the last login
    /// of every `ids`, all due `now`.
    fn new(ids: &[i64], now: Instant) -> Self {
        let mut jobs = vec![
            (Job::UpdateCheck, now),
            (Job::DayCounts, now),
            (Job::SystemKills, now),
            (Job::Wormholes, now),
        ];
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
    /// The source that fills in the day counts and keeps the hourly kills.
    zkillboard: ZKillboardHandle,
    /// When the hourly kills are asked for.
    kills_policy: SystemKillsPolicy,
    /// How many times in a row ESI answered with the hour already known.
    kills_stale: u32,
    /// The last read of the hourly kills failed (so a streak warns once).
    kills_failing: bool,
    /// The characters' locations and the stargates, for the plan.
    alarm: Arc<AlarmShared>,
    /// The client of EvE-Scout, `None` if it could not be made.
    eve_scout: Option<eve_scout::Client>,
    /// Where the wormholes are kept and whether they are wanted.
    wormholes: Arc<WormholeShared>,
    /// When the wormholes are asked for.
    wormholes_policy: WormholesPolicy,
    /// The last read of the wormholes failed (so a streak warns once).
    wormholes_failing: bool,
}

impl Housekeeping {
    /// Runs the schedule until every sender of `receiver` is gone.
    async fn run(mut self, mut receiver: mpsc::Receiver<CharacterSync>) {
        let alarm = Arc::clone(&self.alarm);
        loop {
            for job in self.schedule.due(Instant::now()) {
                let delay = match job {
                    Job::LastLogon(id) => self.check_last_logon(id).await,
                    Job::UpdateCheck => self.check_update().await,
                    Job::DayCounts => self.plan_day_counts(),
                    Job::SystemKills => self.check_system_kills().await,
                    Job::Wormholes => self.check_wormholes().await,
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
                () = alarm.moved.notified() => {
                    // A character changed system: its new surroundings are
                    // planned now, not at the next round.
                    self.schedule.reschedule(Job::DayCounts, Instant::now());
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

    /// Tells the zKillboard source which systems to fill in the kills of the
    /// last day of, and in which order. Returns when to do it again: soon
    /// when the stargates are not loaded yet, otherwise when the first count
    /// of the plan comes due (or in a few minutes, if that is later).
    fn plan_day_counts(&self) -> Duration {
        let settings = self.zkillboard.prefetch_settings();
        let origins = self
            .alarm
            .locations
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let jumps = self
            .alarm
            .jumps
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if jumps.is_empty() {
            return settings.retry_every;
        }
        self.zkillboard.prefetch_days(&jumps, &origins);
        self.zkillboard.day_replan_in()
    }

    /// Reads the kills of the last hour of every system from ESI and hands
    /// them to the zKillboard source, which shows them in the heat until its
    /// own killmails make up for them. Nothing is asked while no map shows
    /// the heat, nor while the answer kept is still the current hour.
    /// Returns when to look again.
    async fn check_system_kills(&mut self) -> Duration {
        // The feed gives the whole hour by itself: ESI is asked only for what
        // is lagging, that is, when it does not (it starts, fails or is off).
        if !self.zkillboard.heat_wanted() || self.zkillboard.covers_the_hour() {
            return self.kills_policy.idle;
        }
        let now = Utc::now();
        if let Some(wait) = self
            .zkillboard
            .hour_modified()
            .and_then(|modified| self.kills_policy.wait(modified, now))
        {
            // The heat may be switched off and on again meanwhile: look at
            // least that often.
            return wait.min(self.kills_policy.slow_retry);
        }
        match self.esi.get_system_kills().await {
            Ok(hour) => {
                let fresh = self.zkillboard.record_hour(&hour);
                self.kills_stale = if fresh {
                    0
                } else {
                    self.kills_stale.saturating_add(1)
                };
                self.kills_failing = false;
                tracing::debug!(
                    fresh,
                    systems = hour.systems.len(),
                    "hourly kills read from ESI"
                );
                self.kills_policy
                    .after_answer(hour.modified, Utc::now(), self.kills_stale)
                    .min(self.kills_policy.slow_retry.max(self.kills_policy.retry))
            }
            Err(error) => {
                if !self.kills_failing {
                    tracing::warn!(%error, "could not read the kills of the last hour from ESI");
                }
                self.kills_failing = true;
                self.kills_policy.slow_retry
            }
        }
    }

    /// Reads the wormhole signatures of Thera and Turnur from EvE-Scout and
    /// replaces the layer of links with them. Nothing is asked while the
    /// setting is off or no map shows the wormholes. Returns when to look
    /// again.
    async fn check_wormholes(&mut self) -> Duration {
        if !self.wormholes.is_wanted() {
            return self.wormholes_policy.idle;
        }
        let Some(client) = self.eve_scout.clone() else {
            return self.wormholes_policy.unavailable;
        };
        match client.signatures(None).await {
            Ok(answer) => {
                self.wormholes.replace(&answer.data);
                // The maps draw them when the UI runs next.
                crate::repaint::request();
                self.wormholes_failing = false;
                tracing::debug!(
                    signatures = answer.data.len(),
                    from_cache = answer.from_cache,
                    "wormholes read from EvE-Scout"
                );
                self.wormholes_policy.after_answer(answer.max_age)
            }
            Err(error) => {
                if !self.wormholes_failing {
                    tracing::warn!(%error, "could not read the wormholes from EvE-Scout");
                }
                self.wormholes_failing = true;
                self.wormholes_policy.after_error(&error)
            }
        }
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
            zkillboard: self.intel_zkillboard.clone(),
            kills_policy: SystemKillsPolicy::default(),
            kills_stale: 0,
            kills_failing: false,
            alarm: Arc::clone(&self.alarm_shared),
            eve_scout: match eve_scout::Client::new(eve_scout::ClientConfig::default()) {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::error!(%error, "failed to make the EvE-Scout client");
                    None
                }
            },
            wormholes: Arc::clone(&self.wormholes),
            wormholes_policy: WormholesPolicy::default(),
            wormholes_failing: false,
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
            vec![
                Job::UpdateCheck,
                Job::DayCounts,
                Job::SystemKills,
                Job::Wormholes,
                Job::LastLogon(1),
                Job::LastLogon(2)
            ]
        );
    }

    #[test]
    fn a_rescheduled_job_waits_for_its_moment() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        schedule.reschedule(Job::LastLogon(1), now + LAST_LOGON_INTERVAL);
        assert_eq!(
            jobs(&schedule, now),
            vec![
                Job::UpdateCheck,
                Job::DayCounts,
                Job::SystemKills,
                Job::Wormholes
            ]
        );
        assert_eq!(
            jobs(&schedule, now + LAST_LOGON_INTERVAL),
            vec![
                Job::UpdateCheck,
                Job::DayCounts,
                Job::SystemKills,
                Job::Wormholes,
                Job::LastLogon(1)
            ]
        );
    }

    #[test]
    fn the_next_wake_up_is_the_earliest_due_job() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        schedule.reschedule(Job::UpdateCheck, now + UPDATE_INTERVAL);
        schedule.reschedule(Job::DayCounts, now + UPDATE_INTERVAL);
        schedule.reschedule(Job::SystemKills, now + UPDATE_INTERVAL);
        schedule.reschedule(Job::Wormholes, now + UPDATE_INTERVAL);
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
            vec![
                Job::UpdateCheck,
                Job::DayCounts,
                Job::SystemKills,
                Job::Wormholes,
                Job::LastLogon(2)
            ]
        );
    }

    #[test]
    fn the_wormholes_are_asked_for_again_when_the_api_says_the_answer_runs_out() {
        let policy = WormholesPolicy::default();
        assert_eq!(
            policy.after_answer(Some(Duration::from_secs(300))),
            Duration::from_secs(300)
        );
        // Not faster than the least, nor slower than the most.
        assert_eq!(
            policy.after_answer(Some(Duration::from_secs(1))),
            policy.least
        );
        assert_eq!(
            policy.after_answer(Some(Duration::from_secs(48 * 60 * 60))),
            policy.most
        );
        // An answer that says nothing about it is asked again soon.
        assert_eq!(policy.after_answer(None), policy.least);
    }

    #[test]
    fn a_failure_waits_for_the_pause_the_client_announced_or_the_usual_retry() {
        let policy = WormholesPolicy::default();
        let paused = eve_scout::Error::Backoff {
            retry_in: Duration::from_secs(240),
        };
        assert_eq!(policy.after_error(&paused), Duration::from_secs(240));
        let soon = eve_scout::Error::Backoff {
            retry_in: Duration::from_secs(1),
        };
        assert_eq!(policy.after_error(&soon), policy.least);
        let failed = eve_scout::Error::Decode(String::from("not json"));
        assert_eq!(policy.after_error(&failed), policy.retry);
    }

    #[test]
    fn the_next_hourly_answer_is_asked_an_hour_and_a_margin_after_the_last() {
        let policy = SystemKillsPolicy::default();
        let now = Utc::now();
        assert_eq!(
            policy.wait(now - TimeDelta::minutes(10), now),
            Some(Duration::from_secs(50 * 60 + 90))
        );
        // An answer older than that has a successor waiting.
        assert_eq!(policy.wait(now - TimeDelta::minutes(62), now), None);
    }

    #[test]
    fn an_old_answer_is_asked_again_soon_and_then_slowly() {
        let policy = SystemKillsPolicy::default();
        let now = Utc::now();
        let old = Some(now - TimeDelta::hours(2));
        assert_eq!(policy.after_answer(old, now, 0), policy.retry);
        assert_eq!(
            policy.after_answer(old, now, policy.retries - 1),
            policy.retry
        );
        assert_eq!(
            policy.after_answer(old, now, policy.retries),
            policy.slow_retry
        );
        // Without a date there is nothing to count from.
        assert_eq!(policy.after_answer(None, now, 0), policy.slow_retry);
    }

    #[test]
    fn a_closed_channel_ends_the_thread() {
        let now = Instant::now();
        let mut schedule = Schedule::new(&[1], now);
        assert!(!schedule.apply_sync(None, now));
    }
}
