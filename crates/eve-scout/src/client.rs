//! The client of the public endpoints of the EvE-Scout API
//! (`https://api.eve-scout.com/v2/public`), one method each.
//!
//! The API is free to use and says how long each answer stays valid in its
//! `Cache-Control` header (5 minutes for signatures and routes, an hour for
//! observations and statistics, a day for systems and wormhole types). The
//! client reports it with the data ([`Cached::max_age`]) and asking again
//! sooner buys nothing, but it does not keep to that pace by itself: whoever
//! calls it decides when to ask again.

use crate::guard::{Fresh, Guard, GuardConfig};
use crate::types::{
    DonationStatistics, Hub, Observation, Period, PublicSystem, Route, RoutePreference,
    RouteRequest, ScanStatistic, Signature, Space, WormholeType, WormholeTypeFilter,
};
use chrono::{DateTime, Utc};
use reqwest::header::{CACHE_CONTROL, HeaderMap, RETRY_AFTER};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How the client asks.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Where the public endpoints are served from, with no trailing slash.
    pub base_url: String,
    /// The `User-Agent` of every request: it says who is asking.
    pub user_agent: String,
    /// How long a whole request may take before it is given up as failed.
    pub request_timeout: Duration,
    /// What keeps the client from asking more than the API needs.
    pub guard: GuardConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            base_url: String::from("https://api.eve-scout.com/v2/public"),
            user_agent: String::from(concat!(
                "telescope/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/rafaga/telescope)"
            )),
            // The routes can take a while: 12 seconds for one answer were
            // seen, and 30 before a 503.
            request_timeout: Duration::from_secs(40),
            guard: GuardConfig::default(),
        }
    }
}

/// The error the API itself reports (`ESAPI-0019-0`...), when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// The code (`ESAPI-0019-0`).
    pub name: String,
    pub message: String,
}

/// Why a request failed.
#[derive(Debug)]
pub enum Error {
    /// The request could not be made or finished: no connection, a timeout.
    Http(reqwest::Error),
    /// The API answered with a status that is not a success.
    Status {
        status: u16,
        /// What the API said about it, when its answer had that shape.
        api: Option<ApiError>,
    },
    /// The answer is not what the API documents.
    Decode(String),
    /// The request was refused before it was made: it can't be valid.
    Invalid(String),
    /// The endpoint gave a bad answer a moment ago and is being left alone:
    /// nothing was asked. It can be asked again in `retry_in`.
    Backoff { retry_in: Duration },
}

impl Error {
    /// Whether asking again later may work: the API or the connection had a
    /// bad moment, as opposed to a request the API refuses for what it says.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Http(error) => error.is_timeout() || error.is_connect() || error.is_request(),
            Error::Status { status, .. } => *status == 429 || *status >= 500,
            Error::Backoff { .. } => true,
            Error::Decode(_) | Error::Invalid(_) => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(error) if error.is_timeout() => {
                write!(f, "EvE-Scout request failed (timed out): {error}")
            }
            Error::Http(error) => write!(f, "EvE-Scout request failed: {error}"),
            Error::Status {
                status,
                api: Some(api),
            } => write!(
                f,
                "EvE-Scout answered {status} ({}): {}",
                api.name, api.message
            ),
            Error::Status { status, api: None } => write!(f, "EvE-Scout answered {status}"),
            Error::Decode(message) => write!(f, "EvE-Scout answer not understood: {message}"),
            Error::Invalid(message) => write!(f, "invalid EvE-Scout request: {message}"),
            Error::Backoff { retry_in } => write!(
                f,
                "EvE-Scout is being left alone after a bad answer, for {} more seconds",
                retry_in.as_secs().max(1)
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Http(error)
    }
}

/// An answer, with what the API said about how long it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached<T> {
    pub data: T,
    /// How long, from now, the API says the answer stays valid
    /// (`Cache-Control: max-age`), if it says. For a copy the client held, it
    /// is what is left of it.
    pub max_age: Option<Duration>,
    /// Whether this is the copy the client held from an earlier request, with
    /// nothing asked of the API.
    pub from_cache: bool,
    /// The last time one of the scouts used the tool that keeps the
    /// signatures up to date. Only the signatures carry it, and only when a
    /// scout has used the tool since the API started.
    pub last_hub_interaction: Option<DateTime<Utc>>,
}

/// The `max-age` of a `Cache-Control` header.
fn max_age(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(CACHE_CONTROL)?.to_str().ok()?;
    value.split(',').find_map(|directive| {
        // The names of the directives do not depend on the case, and a value
        // may be quoted.
        let (name, seconds) = directive.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("max-age") {
            return None;
        }
        seconds
            .trim()
            .trim_matches('"')
            .parse()
            .ok()
            .map(Duration::from_secs)
    })
}

/// The header with the last scout interaction. The API sends the start of
/// the epoch when there was none.
fn last_hub_interaction(headers: &HeaderMap) -> Option<DateTime<Utc>> {
    let value = headers
        .get("x-last-signaleer-hub-interaction")?
        .to_str()
        .ok()?;
    let at = DateTime::parse_from_rfc3339(value.trim())
        .ok()?
        .with_timezone(&Utc);
    (at.timestamp() > 0).then_some(at)
}

/// How the API words an error.
#[derive(Deserialize)]
struct ErrorBody {
    error: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    name: Option<String>,
    message: Option<String>,
}

/// The wait a `Retry-After` header asks for, when it is a number of seconds
/// (the form with a date is not followed).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

/// An answer held by the guard, read as the type it was asked as.
fn from_held<T: DeserializeOwned>(held: Fresh) -> Result<Cached<T>, Error> {
    let data =
        serde_json::from_slice(&held.body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(Cached {
        data,
        max_age: Some(held.remaining),
        from_cache: true,
        last_hub_interaction: held.last_hub_interaction,
    })
}

/// The client. Cheap to clone: the connections and the guard are shared, so
/// every clone is held back by what the others were told.
///
/// What it asks of the API is kept in check by a guard (see
/// [`GuardConfig`]): an answer that is still valid is given again without
/// asking, an endpoint that gave a bad answer is left alone for a while
/// ([`Error::Backoff`]), and the same request made twice at once is made
/// once.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    guard: Arc<Guard>,
}

impl Client {
    /// A client that asks as `config` says.
    pub fn new(config: ClientConfig) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.as_str())
            .timeout(config.request_timeout)
            .build()?;
        Ok(Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            guard: Arc::new(Guard::new(config.guard)),
        })
    }

    /// Forgets the answers the client holds, so the next request goes to the
    /// API even if the last answer is still valid. An endpoint that is being
    /// left alone after a bad answer stays so.
    pub fn forget(&self) {
        self.guard.forget();
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Cached<T>, Error> {
        let key = format!(
            "GET {path}?{}",
            query
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("&")
        );
        let request = self
            .http
            .get(format!("{}{path}", self.base_url))
            .query(query);
        self.send(path, key, request).await
    }

    /// Makes `request` for the endpoint `path`, held in check by the guard.
    /// `key` says what is being asked, to tell it from other requests of the
    /// same endpoint.
    async fn send<T: DeserializeOwned>(
        &self,
        path: &str,
        key: String,
        request: reqwest::RequestBuilder,
    ) -> Result<Cached<T>, Error> {
        if let Some(held) = self.guard.fresh(&key, Instant::now()) {
            return from_held(held);
        }
        // One at a time for the same request: whoever comes second waits for
        // the first and takes what it got.
        let lane = self.guard.lane(&key);
        let turn = lane.lock().await;
        let result = self.send_in_turn(path, &key, request).await;
        drop(turn);
        self.guard.release_lane(&key, lane);
        result
    }

    async fn send_in_turn<T: DeserializeOwned>(
        &self,
        path: &str,
        key: &str,
        request: reqwest::RequestBuilder,
    ) -> Result<Cached<T>, Error> {
        if let Some(held) = self.guard.fresh(key, Instant::now()) {
            return from_held(held);
        }
        if let Some(retry_in) = self.guard.pause(path, Instant::now()) {
            return Err(Error::Backoff { retry_in });
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                self.guard.failed(path, None, Instant::now());
                return Err(error.into());
            }
        };
        let status = response.status();
        let max_age = max_age(response.headers());
        let last_hub_interaction = last_hub_interaction(response.headers());
        let retry = retry_after(response.headers());
        let body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => {
                self.guard.failed(path, None, Instant::now());
                return Err(error.into());
            }
        };
        if !status.is_success() {
            // Only what is the API's or the connection's bad moment holds
            // the endpoint back; a request it refuses for what it says does
            // not.
            if status.as_u16() == 429 || status.is_server_error() {
                self.guard.failed(path, retry, Instant::now());
            }
            let api = serde_json::from_slice::<ErrorBody>(&body)
                .ok()
                .and_then(|body| body.error)
                .map(|detail| ApiError {
                    name: detail.name.unwrap_or_default(),
                    message: detail.message.unwrap_or_default(),
                });
            return Err(Error::Status {
                status: status.as_u16(),
                api,
            });
        }
        self.guard.succeeded(path);
        let data =
            serde_json::from_slice(&body).map_err(|error| Error::Decode(error.to_string()))?;
        // Held only what was read well, for as long as the API says.
        if let Some(max_age) = max_age {
            self.guard.store(
                key.to_string(),
                body.to_vec(),
                max_age,
                last_hub_interaction,
                Instant::now(),
            );
        }
        Ok(Cached {
            data,
            max_age,
            from_cache: false,
            last_hub_interaction,
        })
    }

    /// What EvE-Scout tracks: the Pochven storms and Tom's Shuttle, with the
    /// latest record of each (`GET /observations`).
    pub async fn observations(&self) -> Result<Cached<Vec<Observation>>, Error> {
        self.get("/observations", &[]).await
    }

    /// The route from one system to another (`GET /routes`). Names are the
    /// systems' names (`Jita`).
    pub async fn route(
        &self,
        from: &str,
        to: &str,
        preference: Option<RoutePreference>,
    ) -> Result<Cached<Vec<Route>>, Error> {
        let mut query = vec![("from", from.to_string()), ("to", to.to_string())];
        push_preference(&mut query, preference);
        self.get("/routes", &query).await
    }

    /// The routes from one system to several (`POST /routes`): up to
    /// [`RouteRequest::MAX_TARGETS`] different ones. Wormhole shortcuts are
    /// used.
    pub async fn routes(&self, request: &RouteRequest) -> Result<Cached<Vec<Route>>, Error> {
        let mut targets: Vec<&str> = request.to.iter().map(String::as_str).collect();
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return Err(Error::Invalid(String::from("no target system")));
        }
        if targets.len() > RouteRequest::MAX_TARGETS {
            return Err(Error::Invalid(format!(
                "{} target systems, at most {} are taken",
                targets.len(),
                RouteRequest::MAX_TARGETS
            )));
        }
        // What is asked is the whole request, not just the endpoint.
        let key = format!(
            "POST /routes {}",
            serde_json::to_string(request).map_err(|error| Error::Invalid(error.to_string()))?
        );
        let builder = self
            .http
            .post(format!("{}/routes", self.base_url))
            .json(request);
        self.send("/routes", key, builder).await
    }

    /// Up to five routes from a system to the closest high-sec systems
    /// (`GET /routes/highsec`).
    pub async fn highsec_routes(&self, from: &str) -> Result<Cached<Vec<Route>>, Error> {
        self.get("/routes/highsec", &[("from", from.to_string())])
            .await
    }

    /// The five shortest routes from a system to Jove Observatories
    /// (`GET /routes/joveobservatories`).
    pub async fn jove_observatory_routes(
        &self,
        from: &str,
        preference: Option<RoutePreference>,
    ) -> Result<Cached<Vec<Route>>, Error> {
        let mut query = vec![("from", from.to_string())];
        push_preference(&mut query, preference);
        self.get("/routes/joveobservatories", &query).await
    }

    /// The routes from a system to every signature known in `hub`
    /// (`GET /routes/signatures`).
    pub async fn signature_routes(
        &self,
        from: &str,
        hub: Hub,
        preference: Option<RoutePreference>,
    ) -> Result<Cached<Vec<Route>>, Error> {
        let mut query = vec![
            ("from", from.to_string()),
            ("system_name", hub.query_name().to_string()),
        ];
        push_preference(&mut query, preference);
        self.get("/routes/signatures", &query).await
    }

    /// The wormhole signatures scanned in Thera and Turnur that have not
    /// expired, or only those of `hub` (`GET /signatures`).
    /// [`Cached::last_hub_interaction`] says when the scouts last updated
    /// them.
    pub async fn signatures(&self, hub: Option<Hub>) -> Result<Cached<Vec<Signature>>, Error> {
        let mut query = Vec::new();
        if let Some(hub) = hub {
            query.push(("system_name", hub.query_name().to_string()));
        }
        self.get("/signatures", &query).await
    }

    /// Who donated to EvE-Scout in the last 30 days and how the daily goal
    /// went (`GET /statistics/donations`).
    pub async fn donation_statistics(&self) -> Result<Cached<DonationStatistics>, Error> {
        self.get("/statistics/donations", &[]).await
    }

    /// How many signatures the scouts scanned, per day, week or month
    /// (`GET /statistics/scans`), in `hub` or in both.
    pub async fn scan_statistics(
        &self,
        period: Period,
        hub: Option<Hub>,
    ) -> Result<Cached<Vec<ScanStatistic>>, Error> {
        let mut query = vec![("period", period.query_name().to_string())];
        if let Some(hub) = hub {
            query.push(("system_name", hub.query_name().to_string()));
        }
        self.get("/statistics/scans", &query).await
    }

    /// The systems whose name starts with `query` (`GET /systems`). `limit`
    /// is at least 1 when given.
    pub async fn systems(
        &self,
        query: &str,
        limit: Option<u32>,
        space: Option<Space>,
    ) -> Result<Cached<Vec<PublicSystem>>, Error> {
        if query.trim().is_empty() {
            return Err(Error::Invalid(String::from("the system search is empty")));
        }
        if limit == Some(0) {
            return Err(Error::Invalid(String::from("the limit must be at least 1")));
        }
        let mut parameters = vec![("query", query.to_string())];
        if let Some(limit) = limit {
            parameters.push(("limit", limit.to_string()));
        }
        if let Some(space) = space {
            parameters.push(("space", space.query_name().to_string()));
        }
        self.get("/systems", &parameters).await
    }

    /// The wormhole types that match `filter` (`GET /wormholetypes`).
    pub async fn wormhole_types(
        &self,
        filter: &WormholeTypeFilter,
    ) -> Result<Cached<Vec<WormholeType>>, Error> {
        if filter.limit == Some(0) {
            return Err(Error::Invalid(String::from("the limit must be at least 1")));
        }
        let mut query = Vec::new();
        if let Some(limit) = filter.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(identifier) = &filter.identifier {
            query.push(("identifier", identifier.clone()));
        }
        for (name, class) in [
            ("source", filter.source),
            ("target", filter.target),
            ("edge", filter.edge),
        ] {
            if let Some(class) = class {
                query.push((name, class_name(class)?));
            }
        }
        self.get("/wormholetypes", &query).await
    }
}

fn push_preference(query: &mut Vec<(&'static str, String)>, preference: Option<RoutePreference>) {
    if let Some(preference) = preference {
        let name = match preference {
            RoutePreference::Safer => "safer",
            RoutePreference::Shortest => "shortest",
            RoutePreference::ShortestGates => "shortest-gates",
        };
        query.push(("preference", name.to_string()));
    }
}

/// The name the API gives a system class, which is how it serializes.
fn class_name(class: crate::types::SystemClass) -> Result<String, Error> {
    match serde_json::to_value(class) {
        Ok(serde_json::Value::String(name)) => Ok(name),
        _ => Err(Error::Invalid(String::from(
            "a system class without a name",
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Mock, Reply};
    use crate::types::SystemClass;

    const SYSTEM: &str = r#"{"system_id":30000142,"system_name":"Jita","region_id":10000002,"region_name":"The Forge","system_class":"hs","security_status":0.9}"#;

    fn client(mock: &Mock) -> Client {
        Client::new(ClientConfig {
            base_url: mock.base_url(),
            ..ClientConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn the_max_age_is_read_from_the_cache_control_header() {
        let mut headers = HeaderMap::new();
        headers.insert(CACHE_CONTROL, "public, max-age=300".parse().unwrap());
        assert_eq!(max_age(&headers), Some(Duration::from_secs(300)));
        headers.insert(CACHE_CONTROL, "no-store".parse().unwrap());
        assert_eq!(max_age(&headers), None);
        assert_eq!(max_age(&HeaderMap::new()), None);
    }

    #[test]
    fn the_max_age_is_found_among_other_directives_whatever_their_case() {
        for (value, expected) in [
            ("public, s-maxage=60, max-age=300", Some(300)),
            ("Max-Age=120", Some(120)),
            ("public,max-age = 45", Some(45)),
            ("max-age=\"90\"", Some(90)),
            ("max-age=abc", None),
            ("max-age=", None),
            ("s-maxage=60", None),
            ("public", None),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(CACHE_CONTROL, value.parse().unwrap());
            assert_eq!(
                max_age(&headers),
                expected.map(Duration::from_secs),
                "{value}"
            );
        }
    }

    #[tokio::test]
    async fn a_refused_connection_and_a_timeout_are_transient_errors() {
        let refused = Client::new(ClientConfig {
            base_url: Mock::refused_url(),
            ..ClientConfig::default()
        })
        .unwrap();
        let error = refused.observations().await.unwrap_err();
        assert!(matches!(error, Error::Http(_)), "{error}");
        assert!(error.is_transient());

        let slow = Client::new(ClientConfig {
            base_url: Mock::hanging_url(),
            request_timeout: Duration::from_millis(150),
            ..ClientConfig::default()
        })
        .unwrap();
        let error = slow.observations().await.unwrap_err();
        assert!(
            matches!(&error, Error::Http(inner) if inner.is_timeout()),
            "{error}"
        );
        assert!(error.is_transient());
        assert!(error.to_string().starts_with("EvE-Scout request failed"));
        assert!(std::error::Error::source(&error).is_some());
    }

    // ---------- the guards ----------

    const OBSERVATION: &str = r#"[{"id":"1","observation_type":"electric_a","observation_category":"storm","system_id":30000512,"system_name":"O5Q7-U"}]"#;

    fn client_with(mock: &Mock, guard: GuardConfig) -> Client {
        Client::new(ClientConfig {
            base_url: mock.base_url(),
            guard,
            ..ClientConfig::default()
        })
        .unwrap()
    }

    fn valid_for(seconds: u64) -> Reply {
        Reply::json(200, OBSERVATION).with_header("Cache-Control", &format!("max-age={seconds}"))
    }

    #[tokio::test]
    async fn an_answer_that_is_still_valid_is_given_again_without_asking() {
        let mock = Mock::start(vec![("/observations", valid_for(300))]);
        let api = client(&mock);

        let first = api.observations().await.unwrap();
        let second = api.observations().await.unwrap();

        assert_eq!(mock.requests().len(), 1);
        assert!(!first.from_cache && second.from_cache);
        assert_eq!(first.data, second.data);
        // What is left of what the API said, not the whole of it again.
        let left = second.max_age.unwrap();
        assert!(left > Duration::from_secs(290) && left <= Duration::from_secs(300));
        // A clone shares what is held.
        assert!(api.clone().observations().await.unwrap().from_cache);
        assert_eq!(mock.requests().len(), 1);
    }

    #[tokio::test]
    async fn an_answer_that_says_nothing_about_how_long_it_holds_is_not_held() {
        let mock = Mock::start(vec![("/observations", Reply::json(200, OBSERVATION))]);
        let api = client(&mock);
        api.observations().await.unwrap();
        let second = api.observations().await.unwrap();
        assert!(!second.from_cache);
        assert_eq!(mock.requests().len(), 2);
    }

    #[tokio::test]
    async fn an_answer_that_ran_out_is_asked_for_again() {
        let mock = Mock::start(vec![("/observations", valid_for(1))]);
        let api = client(&mock);
        api.observations().await.unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let again = api.observations().await.unwrap();
        assert!(!again.from_cache);
        assert_eq!(mock.requests().len(), 2);
    }

    #[tokio::test]
    async fn the_same_request_made_twice_at_once_is_made_once() {
        let mock = Mock::start(vec![("/observations", valid_for(300))]);
        let api = client(&mock);

        let (first, second) = tokio::join!(api.observations(), api.observations());

        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(mock.requests().len(), 1);
        assert_eq!(first.data, second.data);
        assert!(
            first.from_cache != second.from_cache,
            "one asked, one waited"
        );
    }

    #[tokio::test]
    async fn requests_that_differ_are_not_taken_for_each_other() {
        let mock = Mock::start(vec![("/routes", valid_for(300).with_body("[]"))]);
        let api = client(&mock);
        api.route("Jita", "Amarr", None).await.unwrap();
        api.route("Jita", "Dodixie", None).await.unwrap();
        assert_eq!(mock.requests().len(), 2);
        assert!(api.route("Jita", "Amarr", None).await.unwrap().from_cache);
        assert_eq!(mock.requests().len(), 2, "the first one was held");
    }

    #[tokio::test]
    async fn the_post_is_told_apart_by_what_it_carries() {
        let mock = Mock::start(vec![("/routes", valid_for(300).with_body("[]"))]);
        let api = client(&mock);
        let request = |to: &str| RouteRequest {
            from: String::from("Zoohen"),
            to: vec![to.to_string()],
            preference: None,
        };
        api.routes(&request("Jita")).await.unwrap();
        api.routes(&request("Amarr")).await.unwrap();
        assert_eq!(mock.requests().len(), 2);
        assert!(api.routes(&request("Jita")).await.unwrap().from_cache);
        assert_eq!(mock.requests().len(), 2);
    }

    #[tokio::test]
    async fn a_bad_answer_leaves_the_endpoint_alone_for_what_retry_after_says() {
        let mock = Mock::start(vec![
            (
                "/observations",
                Reply::json(503, "").with_header("Retry-After", "30"),
            ),
            ("/signatures", Reply::json(200, "[]")),
        ]);
        let api = client(&mock);

        let first = api.observations().await.unwrap_err();
        assert!(matches!(first, Error::Status { status: 503, .. }));
        let second = api.observations().await.unwrap_err();
        let Error::Backoff { retry_in } = second else {
            panic!("not left alone: {second}");
        };
        assert!(retry_in > Duration::from_secs(25) && retry_in <= Duration::from_secs(30));
        assert!(second.is_transient());
        // Nothing was asked the second time.
        assert_eq!(mock.requests().len(), 1);
        // Another endpoint is not held back.
        api.signatures(None).await.unwrap();
        assert_eq!(mock.requests().len(), 2);
    }

    #[tokio::test]
    async fn too_many_requests_holds_it_back_too_but_a_refusal_does_not() {
        let mock = Mock::start(vec![
            ("/observations", Reply::json(429, "")),
            (
                "/signatures",
                Reply::json(400, r#"{"error":{"name":"ESAPI-0010-0","message":"x"}}"#),
            ),
        ]);
        let api = client(&mock);

        api.observations().await.unwrap_err();
        assert!(matches!(
            api.observations().await.unwrap_err(),
            Error::Backoff { .. }
        ));

        // The API refused what was asked, which is not its bad moment: asked
        // again as soon as the caller wants.
        api.signatures(None).await.unwrap_err();
        let again = api.signatures(None).await.unwrap_err();
        assert!(matches!(again, Error::Status { status: 400, .. }));
        assert_eq!(mock.requests().len(), 3);
    }

    #[tokio::test]
    async fn without_retry_after_the_wait_doubles_and_a_good_answer_ends_it() {
        let mock = Mock::start(vec![
            ("/observations", Reply::json(503, "")),
            ("/observations", Reply::json(503, "")),
            ("/observations", Reply::json(200, OBSERVATION)),
        ]);
        let api = client_with(
            &mock,
            GuardConfig {
                backoff_base: Duration::from_millis(60),
                ..GuardConfig::default()
            },
        );

        api.observations().await.unwrap_err();
        let Error::Backoff { retry_in } = api.observations().await.unwrap_err() else {
            panic!("not left alone");
        };
        assert!(retry_in <= Duration::from_millis(60));

        tokio::time::sleep(Duration::from_millis(70)).await;
        api.observations().await.unwrap_err();
        let Error::Backoff { retry_in } = api.observations().await.unwrap_err() else {
            panic!("not left alone");
        };
        // The second failure in a row waits for twice as long.
        assert!(retry_in > Duration::from_millis(60), "{retry_in:?}");

        tokio::time::sleep(Duration::from_millis(130)).await;
        api.observations().await.unwrap();
        // It is over: the next one goes to the API again (nothing was held,
        // the answer said nothing about it).
        api.observations().await.unwrap();
        assert_eq!(mock.requests().len(), 4);
    }

    #[tokio::test]
    async fn a_connection_that_failed_leaves_the_endpoint_alone_too() {
        let api = Client::new(ClientConfig {
            base_url: Mock::refused_url(),
            ..ClientConfig::default()
        })
        .unwrap();
        assert!(matches!(
            api.observations().await.unwrap_err(),
            Error::Http(_)
        ));
        assert!(matches!(
            api.observations().await.unwrap_err(),
            Error::Backoff { .. }
        ));
    }

    #[tokio::test]
    async fn forgetting_asks_again_but_does_not_end_a_wait() {
        let mock = Mock::start(vec![
            ("/observations", valid_for(300)),
            ("/signatures", Reply::json(503, "")),
        ]);
        let api = client(&mock);
        api.observations().await.unwrap();
        api.forget();
        assert!(!api.observations().await.unwrap().from_cache);
        assert_eq!(mock.requests().len(), 2);

        api.signatures(None).await.unwrap_err();
        api.forget();
        assert!(matches!(
            api.signatures(None).await.unwrap_err(),
            Error::Backoff { .. }
        ));
    }

    #[test]
    fn the_start_of_the_epoch_means_no_scout_has_used_the_tool() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-last-signaleer-hub-interaction",
            "1970-01-01T00:00:00.000Z".parse().unwrap(),
        );
        assert_eq!(last_hub_interaction(&headers), None);
        headers.insert(
            "x-last-signaleer-hub-interaction",
            "2026-11-05T16:38:29.016Z".parse().unwrap(),
        );
        assert_eq!(
            last_hub_interaction(&headers).unwrap().to_rfc3339(),
            "2026-11-05T16:38:29.016+00:00"
        );
    }

    #[tokio::test]
    async fn a_route_is_asked_with_its_names_and_preference_and_comes_with_its_ttl() {
        let body = format!(r#"[{{"from":"Jita","to":"Amarr","jumps":1,"route":[{SYSTEM}]}}]"#);
        let mock = Mock::start(vec![(
            "/routes",
            Reply::json(200, &body).with_header("Cache-Control", "public, max-age=300"),
        )]);
        let answer = client(&mock)
            .route("Jita", "Amarr", Some(RoutePreference::ShortestGates))
            .await
            .unwrap();
        assert_eq!(answer.max_age, Some(Duration::from_secs(300)));
        assert_eq!(answer.data[0].route[0].system_name, "Jita");
        let requests = mock.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(
            requests[0].target,
            "/routes?from=Jita&to=Amarr&preference=shortest-gates"
        );
        // The User-Agent says who is asking.
        assert!(
            requests[0]
                .header("user-agent")
                .unwrap()
                .starts_with("telescope/")
        );
    }

    #[tokio::test]
    async fn signatures_carry_the_last_interaction_of_the_scouts() {
        let mock = Mock::start(vec![(
            "/signatures",
            Reply::json(200, "[]").with_header(
                "X-Last-Signaleer-Hub-Interaction",
                "2026-11-05T16:38:29.016Z",
            ),
        )]);
        let answer = client(&mock).signatures(Some(Hub::Thera)).await.unwrap();
        assert!(answer.data.is_empty());
        assert!(answer.last_hub_interaction.is_some());
        assert_eq!(mock.requests()[0].target, "/signatures?system_name=thera");
        // Without a hub there is no filter.
        client(&mock).signatures(None).await.unwrap();
        assert_eq!(mock.requests()[1].target, "/signatures");
    }

    #[tokio::test]
    async fn several_routes_are_asked_in_one_post_with_a_json_body() {
        let mock = Mock::start(vec![("/routes", Reply::json(200, "[]"))]);
        let request = RouteRequest {
            from: String::from("Zoohen"),
            to: vec![
                String::from("Jita"),
                String::from("Thera"),
                String::from("Jita"),
            ],
            preference: Some(RoutePreference::Safer),
        };
        client(&mock).routes(&request).await.unwrap();
        let sent = &mock.requests()[0];
        assert_eq!(sent.method, "POST");
        let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(body["from"], "Zoohen");
        assert_eq!(body["preference"], "safer");
        assert_eq!(body["to"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_request_the_api_can_not_take_is_refused_before_it_is_made() {
        let mock = Mock::start(vec![("/routes", Reply::json(200, "[]"))]);
        let api = client(&mock);
        let none = RouteRequest {
            from: String::from("Zoohen"),
            to: Vec::new(),
            preference: None,
        };
        assert!(matches!(api.routes(&none).await, Err(Error::Invalid(_))));
        let many = RouteRequest {
            from: String::from("Zoohen"),
            to: (0..=RouteRequest::MAX_TARGETS)
                .map(|n| format!("S{n}"))
                .collect(),
            preference: None,
        };
        assert!(matches!(api.routes(&many).await, Err(Error::Invalid(_))));
        assert!(matches!(
            api.systems("  ", None, None).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            api.systems("Ji", Some(0), None).await,
            Err(Error::Invalid(_))
        ));
        // None of them reached the server.
        assert!(mock.requests().is_empty());
        // Duplicates do not count: the API removes them.
        let repeated = RouteRequest {
            from: String::from("Zoohen"),
            to: vec![String::from("Jita"); 300],
            preference: None,
        };
        assert!(api.routes(&repeated).await.is_ok());
    }

    #[tokio::test]
    async fn the_searches_and_filters_become_query_parameters() {
        let mock = Mock::start(vec![
            ("/systems", Reply::json(200, &format!("[{SYSTEM}]"))),
            ("/wormholetypes", Reply::json(200, "[]")),
            ("/statistics/scans", Reply::json(200, "[]")),
        ]);
        let client = client(&mock);
        let found = client.systems("Ji", Some(5), Some(Space::K)).await.unwrap();
        assert_eq!(found.data[0].system_class, SystemClass::HighSec);
        client
            .wormhole_types(&WormholeTypeFilter {
                limit: Some(2),
                identifier: Some(String::from("z")),
                source: Some(SystemClass::C2),
                target: Some(SystemClass::LowSec),
                edge: None,
            })
            .await
            .unwrap();
        client
            .scan_statistics(Period::Weekly, Some(Hub::Turnur))
            .await
            .unwrap();
        let targets: Vec<_> = mock.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            [
                "/systems?query=Ji&limit=5&space=k-space",
                "/wormholetypes?limit=2&identifier=z&source=c2&target=ls",
                "/statistics/scans?period=weekly&system_name=turnur",
            ]
        );
    }

    #[tokio::test]
    async fn an_error_of_the_api_says_what_it_said() {
        let mock = Mock::start(vec![(
            "/routes",
            Reply::json(
                400,
                r#"{"error":{"name":"ESAPI-0019-0","message":"invalid or missing value for 'from'","details":{"from":"Nowhere"}}}"#,
            ),
        )]);
        let error = client(&mock)
            .route("Nowhere", "Amarr", None)
            .await
            .unwrap_err();
        let Error::Status { status, api } = &error else {
            panic!("not a status error: {error}");
        };
        assert_eq!(*status, 400);
        assert_eq!(api.as_ref().unwrap().name, "ESAPI-0019-0");
        assert!(!error.is_transient());
        assert!(error.to_string().contains("ESAPI-0019-0"));
    }

    #[tokio::test]
    async fn a_bad_gateway_is_transient_and_a_broken_body_is_not() {
        let mock = Mock::start(vec![
            ("/observations", Reply::json(502, "")),
            ("/signatures", Reply::json(200, "not json")),
        ]);
        let client = client(&mock);
        let bad_gateway = client.observations().await.unwrap_err();
        assert!(matches!(
            bad_gateway,
            Error::Status {
                status: 502,
                api: None
            }
        ));
        assert!(bad_gateway.is_transient());
        let broken = client.signatures(None).await.unwrap_err();
        assert!(matches!(broken, Error::Decode(_)));
        assert!(!broken.is_transient());
    }
}
