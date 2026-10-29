//! The killmails of a system over the last day, asked of zKillboard's query
//! API on demand.
//!
//! The R2Z2 feed only gives what happens from now on, and the buffer of the
//! node tooltips (see [`super::kill_context`]) keeps one hour of it. The day
//! line of a tooltip, "24 h: 6 kills", comes from the other API
//! (`/api/solarSystemID/<id>/pastSeconds/86400/`), asked once per system the
//! first time its tooltip is shown and kept for an hour (zKillboard caches
//! its answers that long, so asking sooner buys nothing).
//!
//! The API answers with the killmail ids (200 at most per page, newest
//! first, without the last five minutes), not with their contents: the count
//! is all that is taken from it. A full page is shown as "200+".
//!
//! The requests are made one at a time, a second apart, by a thread of their
//! own ([`DayCache::fetch_next`] is its step) so the live feed is never kept
//! waiting by them.
//!
//! # Without the feed
//!
//! While the feed is not running (no rule asks for it) the tooltips still
//! get the last hour of a system, from the same API: the newest killmail ids
//! it lists come with their hash, and ESI gives the killmail itself
//! (`/killmails/<id>/<hash>/`, public, same body as the feed's `esi` part).
//! [`DayCache::fetch_detail`] reads them newest first, until one is older
//! than the hour. They go to a buffer of their own, not to the feed's.

use super::kill_context::KillRecord;
use super::zkillboard::{EsiKillmail, FeedError, Killmail, REQUEST_TIMEOUT, USER_AGENT, Zkb};
use chrono::TimeDelta;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Base of zKillboard's query API.
const API_URL: &str = "https://zkillboard.com/api";

/// Base of ESI, for the killmails the API only lists by id.
const ESI_URL: &str = "https://esi.evetech.net";

/// How many of the newest killmails of a system are read from ESI at most.
const MAX_DETAILS: usize = 10;

/// How far back the day line looks.
const DAY_SECONDS: u64 = 24 * 60 * 60;

/// How long an answer is kept before asking again.
pub(crate) const TTL: Duration = Duration::from_secs(60 * 60);

/// How long to leave a system alone after a failed request.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// Killmails per page of the API: a full page may have more behind it.
const PAGE_SIZE: usize = 200;

/// Systems waiting for their request at most; hovering over more than this
/// before the first answer arrives just waits for the next hover.
const MAX_QUEUED: usize = 32;

/// Pause between two requests.
pub(crate) const PACE: Duration = Duration::from_secs(1);

/// What the API said about a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayCount {
    /// Killmails in the last day (the first page of them).
    pub kills: usize,
    /// Whether a full page came back, so there may be more.
    pub more: bool,
}

/// A killmail the API lists: enough to ask ESI for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KillRef {
    pub killmail_id: u64,
    pub hash: String,
}

/// What the API said about a system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DayAnswer {
    pub count: DayCount,
    /// The newest killmails, newest first.
    pub newest: Vec<KillRef>,
}

/// What a tooltip can say about the last day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DayLine {
    /// The request is on its way.
    Pending,
    /// The count, as of the last answer.
    Known(DayCount),
    /// The last request failed; it is tried again later.
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    /// Queued, or being asked.
    Pending,
    Ready {
        count: DayCount,
        at: Instant,
    },
    /// Being asked again; the old count is shown meanwhile.
    Refreshing(DayCount),
    Failed {
        at: Instant,
    },
}

/// Why a step of the fetcher ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fetched {
    /// Nothing was queued.
    Idle,
    /// One system was asked (whatever the answer).
    Done,
    /// The API refused the request: stay away for a long while.
    Throttled,
}

/// The reads the tooltips need from the API and ESI.
pub(crate) trait DayApi {
    /// The killmails of `system_id` in the last day.
    async fn kills(&self, system_id: usize) -> Result<DayAnswer, FeedError>;

    /// The killmail `kill`, from ESI.
    async fn killmail(&self, kill: &KillRef) -> Result<Killmail, FeedError>;
}

/// What a step of the detail reader did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Detail {
    /// Nothing was waiting.
    Idle,
    /// A killmail was read.
    Read(Killmail),
    /// The killmail was already kept (see `kill_store`): no request was made.
    Known(KillRecord),
    /// A killmail could not be read (gone, or an error): it is skipped.
    Skipped,
    /// ESI refused the request: stay away for a long while.
    Throttled,
}

/// The counts known and the systems waiting to be asked.
#[derive(Debug, Default)]
pub(crate) struct DayCache {
    entries: HashMap<usize, Entry>,
    queue: VecDeque<usize>,
    /// Killmails to read from ESI, per system newest first.
    details: VecDeque<(usize, KillRef)>,
}

impl DayCache {
    /// What to show for `system_id` as of `now`, queueing a request when
    /// there is no fresh answer. The second value tells whether anything is
    /// queued, that is, whether the fetcher must be woken.
    pub(crate) fn look(&mut self, system_id: usize, now: Instant) -> (DayLine, bool) {
        let line = match self.entries.get(&system_id).copied() {
            Some(Entry::Ready { count, at }) if now.duration_since(at) < TTL => {
                DayLine::Known(count)
            }
            Some(Entry::Ready { count, .. }) => {
                self.enqueue(system_id, Entry::Refreshing(count));
                DayLine::Known(count)
            }
            Some(Entry::Refreshing(count)) => DayLine::Known(count),
            Some(Entry::Pending) => DayLine::Pending,
            Some(Entry::Failed { at }) if now.duration_since(at) < RETRY_AFTER => {
                DayLine::Unavailable
            }
            Some(Entry::Failed { .. }) | None => {
                if self.enqueue(system_id, Entry::Pending) {
                    DayLine::Pending
                } else {
                    // The queue is full: say nothing yet.
                    DayLine::Unavailable
                }
            }
        };
        (line, !self.queue.is_empty())
    }

    /// Queues `system_id` with `entry` as its state. `false` when the queue
    /// is full.
    fn enqueue(&mut self, system_id: usize, entry: Entry) -> bool {
        if self.queue.len() >= MAX_QUEUED {
            return false;
        }
        self.entries.insert(system_id, entry);
        self.queue.push_back(system_id);
        true
    }

    /// The counts that are known, with how long ago each was asked, to be
    /// kept on disk.
    pub(crate) fn snapshot(&self, now: Instant) -> Vec<(usize, DayCount, TimeDelta)> {
        self.entries
            .iter()
            .filter_map(|(system, entry)| match entry {
                Entry::Ready { count, at } => {
                    let age = TimeDelta::from_std(now.saturating_duration_since(*at)).ok()?;
                    Some((*system, *count, age))
                }
                _ => None,
            })
            .collect()
    }

    /// Takes back counts kept on disk (see [`Self::snapshot`]) for the
    /// systems nothing is known about yet.
    pub(crate) fn restore(
        &mut self,
        counts: impl IntoIterator<Item = (usize, DayCount, TimeDelta)>,
        now: Instant,
    ) {
        for (system, count, age) in counts {
            if let Ok(age) = age.to_std()
                && let Some(at) = now.checked_sub(age)
            {
                self.entries
                    .entry(system)
                    .or_insert(Entry::Ready { count, at });
            }
        }
    }

    /// Forgets everything.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.queue.clear();
        self.details.clear();
    }

    /// Whether killmails of `system_id` are still waiting to be read.
    pub(crate) fn has_details(&self, system_id: usize) -> bool {
        self.details.iter().any(|(system, _)| *system == system_id)
    }

    /// Gives up the killmails of `system_id` still waiting (the next ones
    /// are older than what is wanted).
    pub(crate) fn drop_details(&mut self, system_id: usize) {
        self.details.retain(|(system, _)| *system != system_id);
    }

    /// Reads the next waiting killmail from ESI.
    ///
    /// A killmail that `known` gives is not asked of ESI again.
    pub(crate) async fn fetch_detail<A: DayApi>(
        cache: &Mutex<Self>,
        api: &A,
        known: impl Fn(u64) -> Option<KillRecord>,
    ) -> (Detail, Option<usize>) {
        let next = {
            let mut cache = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.details.pop_front()
        };
        let Some((system_id, kill)) = next else {
            return (Detail::Idle, None);
        };
        if let Some(record) = known(kill.killmail_id) {
            return (Detail::Known(record), Some(system_id));
        }
        let detail = match api.killmail(&kill).await {
            Ok(killmail) => Detail::Read(killmail),
            Err(FeedError::Throttled(_)) => Detail::Throttled,
            Err(_) => Detail::Skipped,
        };
        (detail, Some(system_id))
    }

    /// Asks the API about the system that has waited longest and stores the
    /// answer. The lock is not held while the request is made.
    ///
    /// With `details`, the newest killmails the API lists are queued to be
    /// read from ESI too (see [`Self::fetch_detail`]).
    pub(crate) async fn fetch_next<A: DayApi>(
        cache: &Mutex<Self>,
        api: &A,
        details: bool,
    ) -> Fetched {
        let next = {
            let mut cache = cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.queue.pop_front()
        };
        let Some(system_id) = next else {
            return Fetched::Idle;
        };
        let result = api.kills(system_id).await;
        let now = Instant::now();
        let mut cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &result {
            Ok(answer) => {
                cache.entries.insert(
                    system_id,
                    Entry::Ready {
                        count: answer.count,
                        at: now,
                    },
                );
                if details {
                    cache.drop_details(system_id);
                    cache
                        .details
                        .extend(answer.newest.iter().map(|kill| (system_id, kill.clone())));
                }
            }
            Err(_) => {
                cache.entries.insert(system_id, Entry::Failed { at: now });
            }
        }
        if matches!(result, Err(FeedError::Throttled(_))) {
            Fetched::Throttled
        } else {
            Fetched::Done
        }
    }
}

/// zKillboard's query API over HTTPS.
pub(crate) struct ZKillApi {
    client: Client,
    base: String,
}

impl ZKillApi {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            base: String::from(API_URL),
        })
    }

    /// The URL of `kill` in ESI (public: no token). Without an
    /// `X-Compatibility-Date` ESI answers as of its oldest date, and the
    /// killmail has not changed since.
    fn killmail_url(kill: &KillRef) -> String {
        format!("{ESI_URL}/killmails/{}/{}/", kill.killmail_id, kill.hash)
    }

    /// The URL of the killmails of `system_id` in the last day. The query is
    /// in the path, and the trailing slash is required.
    fn day_url(&self, system_id: usize) -> String {
        format!(
            "{}/solarSystemID/{system_id}/pastSeconds/{DAY_SECONDS}/",
            self.base
        )
    }
}

impl DayApi for ZKillApi {
    async fn kills(&self, system_id: usize) -> Result<DayAnswer, FeedError> {
        #[derive(Deserialize)]
        struct Item {
            killmail_id: u64,
            #[serde(default)]
            zkb: ItemZkb,
        }
        #[derive(Deserialize, Default)]
        struct ItemZkb {
            #[serde(default)]
            hash: String,
        }
        let response = self
            .client
            .get(self.day_url(system_id))
            .send()
            .await
            .map_err(|error| FeedError::Other(error.to_string()))?;
        match response.status() {
            status if status.is_success() => {}
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
                return Err(FeedError::Throttled(status));
            }
            status => return Err(FeedError::Other(format!("the API answered {status}"))),
        }
        let killmails: Vec<Item> = response
            .json()
            .await
            .map_err(|error| FeedError::Other(format!("unreadable answer: {error}")))?;
        Ok(DayAnswer {
            count: DayCount {
                kills: killmails.len(),
                more: killmails.len() >= PAGE_SIZE,
            },
            newest: killmails
                .into_iter()
                .filter(|item| !item.zkb.hash.is_empty())
                .take(MAX_DETAILS)
                .map(|item| KillRef {
                    killmail_id: item.killmail_id,
                    hash: item.zkb.hash,
                })
                .collect(),
        })
    }

    async fn killmail(&self, kill: &KillRef) -> Result<Killmail, FeedError> {
        /// The body of ESI's killmail: the feed's `esi` part plus the id.
        #[derive(Deserialize)]
        struct Body {
            killmail_id: u64,
            #[serde(flatten)]
            esi: EsiKillmail,
        }
        let response = self
            .client
            .get(Self::killmail_url(kill))
            .send()
            .await
            .map_err(|error| FeedError::Other(error.to_string()))?;
        match response.status() {
            status if status.is_success() => {}
            status @ (StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS) => {
                return Err(FeedError::Throttled(status));
            }
            status => return Err(FeedError::Other(format!("ESI answered {status}"))),
        }
        let body: Body = response
            .json()
            .await
            .map_err(|error| FeedError::Other(format!("unreadable killmail: {error}")))?;
        Ok(Killmail {
            killmail_id: body.killmail_id,
            sequence_id: 0,
            esi: body.esi,
            zkb: Zkb::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::intel::zkillboard::Victim;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// An API in memory: the same answer for every system.
    struct FakeApi {
        answer: Result<DayAnswer, FeedError>,
        killmail: Result<(), FeedError>,
        asked: AtomicUsize,
    }

    impl FakeApi {
        fn new(answer: Result<DayCount, FeedError>) -> Self {
            Self {
                answer: answer.map(|count| DayAnswer {
                    count,
                    newest: vec![kill_ref(11), kill_ref(10)],
                }),
                killmail: Ok(()),
                asked: AtomicUsize::new(0),
            }
        }
    }

    fn kill_ref(id: u64) -> KillRef {
        KillRef {
            killmail_id: id,
            hash: format!("hash{id}"),
        }
    }

    impl DayApi for FakeApi {
        async fn kills(&self, _system_id: usize) -> Result<DayAnswer, FeedError> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }

        async fn killmail(&self, kill: &KillRef) -> Result<Killmail, FeedError> {
            self.killmail.clone()?;
            Ok(Killmail {
                killmail_id: kill.killmail_id,
                sequence_id: 0,
                esi: EsiKillmail {
                    killmail_time: String::from("2026-10-29T04:41:23Z"),
                    solar_system_id: 1,
                    victim: Victim { ship_type_id: None },
                    attackers: Vec::new(),
                },
                zkb: Zkb::default(),
            })
        }
    }

    const COUNT: DayCount = DayCount {
        kills: 6,
        more: false,
    };

    #[test]
    fn a_new_system_is_queued_once() {
        let mut cache = DayCache::default();
        let now = Instant::now();
        assert_eq!(cache.look(1, now), (DayLine::Pending, true));
        assert_eq!(cache.look(1, now), (DayLine::Pending, true));
        assert_eq!(cache.queue.len(), 1);
    }

    #[tokio::test]
    async fn the_answer_is_kept_for_an_hour_and_then_asked_again() {
        let cache = Mutex::new(DayCache::default());
        let api = FakeApi::new(Ok(COUNT));
        let start = Instant::now();
        cache.lock().unwrap().look(1, start);
        assert_eq!(
            DayCache::fetch_next(&cache, &api, false).await,
            Fetched::Done
        );
        assert_eq!(
            DayCache::fetch_next(&cache, &api, false).await,
            Fetched::Idle
        );
        let (line, queued) = cache.lock().unwrap().look(1, Instant::now());
        assert_eq!((line, queued), (DayLine::Known(COUNT), false));
        // An hour later the old count shows while a new one is asked.
        let later = Instant::now() + TTL + Duration::from_secs(1);
        let (line, queued) = cache.lock().unwrap().look(1, later);
        assert_eq!((line, queued), (DayLine::Known(COUNT), true));
        assert_eq!(
            DayCache::fetch_next(&cache, &api, false).await,
            Fetched::Done
        );
        assert_eq!(api.asked.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_failure_is_not_retried_at_once() {
        let cache = Mutex::new(DayCache::default());
        let api = FakeApi::new(Err(FeedError::Other(String::from("down"))));
        cache.lock().unwrap().look(1, Instant::now());
        assert_eq!(
            DayCache::fetch_next(&cache, &api, false).await,
            Fetched::Done
        );
        let (line, queued) = cache.lock().unwrap().look(1, Instant::now());
        assert_eq!((line, queued), (DayLine::Unavailable, false));
        let later = Instant::now() + RETRY_AFTER + Duration::from_secs(1);
        let (line, queued) = cache.lock().unwrap().look(1, later);
        assert_eq!((line, queued), (DayLine::Pending, true));
    }

    #[tokio::test]
    async fn a_refusal_makes_the_fetcher_stay_away() {
        let cache = Mutex::new(DayCache::default());
        let api = FakeApi::new(Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS)));
        cache.lock().unwrap().look(1, Instant::now());
        assert_eq!(
            DayCache::fetch_next(&cache, &api, false).await,
            Fetched::Throttled
        );
    }

    #[test]
    fn the_queue_has_a_limit() {
        let mut cache = DayCache::default();
        let now = Instant::now();
        for system in 0..MAX_QUEUED {
            assert_eq!(cache.look(system, now).0, DayLine::Pending);
        }
        assert_eq!(cache.look(MAX_QUEUED, now).0, DayLine::Unavailable);
        assert_eq!(cache.queue.len(), MAX_QUEUED);
    }

    #[test]
    fn the_query_is_in_the_path_with_a_trailing_slash() {
        let api = ZKillApi::new().unwrap();
        assert_eq!(
            api.day_url(30002813),
            "https://zkillboard.com/api/solarSystemID/30002813/pastSeconds/86400/"
        );
        assert_eq!(
            ZKillApi::killmail_url(&kill_ref(11)),
            "https://esi.evetech.net/killmails/11/hash11/"
        );
    }

    #[tokio::test]
    async fn the_newest_killmails_are_read_one_by_one_only_when_asked_for() {
        let cache = Mutex::new(DayCache::default());
        let api = FakeApi::new(Ok(COUNT));
        cache.lock().unwrap().look(1, Instant::now());
        DayCache::fetch_next(&cache, &api, false).await;
        assert!(!cache.lock().unwrap().has_details(1));

        cache.lock().unwrap().look(2, Instant::now());
        DayCache::fetch_next(&cache, &api, true).await;
        assert!(cache.lock().unwrap().has_details(2));
        let (detail, system) = DayCache::fetch_detail(&cache, &api, |_| None).await;
        assert!(matches!(detail, Detail::Read(killmail) if killmail.killmail_id == 11));
        assert_eq!(system, Some(2));
        // The one left is given up when the next ones are too old.
        cache.lock().unwrap().drop_details(2);
        assert_eq!(
            DayCache::fetch_detail(&cache, &api, |_| None).await.0,
            Detail::Idle
        );
    }

    #[tokio::test]
    async fn a_killmail_that_cannot_be_read_is_skipped_and_a_refusal_stops() {
        let cache = Mutex::new(DayCache::default());
        let mut api = FakeApi::new(Ok(COUNT));
        cache.lock().unwrap().look(1, Instant::now());
        DayCache::fetch_next(&cache, &api, true).await;
        api.killmail = Err(FeedError::Other(String::from("gone")));
        assert_eq!(
            DayCache::fetch_detail(&cache, &api, |_| None).await.0,
            Detail::Skipped
        );
        api.killmail = Err(FeedError::Throttled(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(
            DayCache::fetch_detail(&cache, &api, |_| None).await.0,
            Detail::Throttled
        );
    }

    #[tokio::test]
    async fn a_killmail_already_kept_is_not_asked_of_esi_again() {
        let cache = Mutex::new(DayCache::default());
        let api = FakeApi::new(Ok(COUNT));
        cache.lock().unwrap().look(2, Instant::now());
        DayCache::fetch_next(&cache, &api, true).await;
        let kept = KillRecord {
            killmail_id: 11,
            time: chrono::Utc::now(),
            system_id: 2,
            victim: None,
            attacker_ships: Vec::new(),
            pilots: 1,
        };
        let known = |id: u64| (id == 11).then(|| kept.clone());
        let (detail, system) = DayCache::fetch_detail(&cache, &api, known).await;
        assert_eq!(detail, Detail::Known(kept.clone()));
        assert_eq!(system, Some(2));
    }

    #[test]
    fn the_counts_are_kept_and_taken_back_with_their_age() {
        let now = Instant::now();
        let mut cache = DayCache::default();
        cache.restore([(1, COUNT, TimeDelta::seconds(600))], now);
        assert_eq!(
            cache.look(1, now).0,
            DayLine::Known(COUNT),
            "a count asked 10 minutes ago is still good"
        );
        let snapshot = cache.snapshot(now);
        assert_eq!(snapshot, vec![(1, COUNT, TimeDelta::seconds(600))]);
        // What is already known is not replaced by what was kept.
        cache.restore(
            [(
                1,
                DayCount {
                    kills: 99,
                    more: true,
                },
                TimeDelta::seconds(1),
            )],
            now,
        );
        assert_eq!(cache.look(1, now).0, DayLine::Known(COUNT));
        // Nor does a count older than the TTL get a free pass.
        let mut stale = DayCache::default();
        let beyond = TimeDelta::from_std(TTL).unwrap() + TimeDelta::seconds(1);
        stale.restore([(2, COUNT, beyond)], now);
        let (line, queued) = stale.look(2, now);
        assert_eq!(line, DayLine::Known(COUNT));
        assert!(queued, "it is asked again, showing the old count meanwhile");
    }
}
