//! The client against the real EvE-Scout API.
//!
//! These tests need the network and the API to be up, so they are ignored by
//! default. Run them with:
//!
//! ```text
//! cargo test -p eve-scout --test live -- --ignored --nocapture
//! ```
//!
//! They are what shows that the requests this crate makes are the ones the API
//! accepts and that its real answers are read whole, which the tests with a
//! made-up server can't. An answer that is a `5xx` or a timeout is reported and
//! the test goes on: it says something about the API, not about this crate.
//! Anything else that goes wrong (an answer that can't be read, a `4xx`)
//! fails.
//!
//! With `SDE_DB` set to the path of a real `sde.db`, one more test checks the
//! wormhole classes of the API against that database.

use eve_scout::{
    Cached, Client, ClientConfig, Error, Hub, Period, RoutePreference, RouteRequest, Space,
    SystemClass, WormholeTypeFilter,
};
use std::time::Duration;

fn client() -> Client {
    Client::new(ClientConfig::default()).unwrap()
}

/// The answer, or `None` when the API had a bad moment.
fn answer<T>(what: &str, result: Result<Cached<T>, Error>) -> Option<Cached<T>> {
    match result {
        Ok(answer) => {
            eprintln!("{what}: ok, valid for {:?}", answer.max_age);
            Some(answer)
        }
        Err(error) if error.is_transient() => {
            eprintln!("{what}: the API had a bad moment, skipped: {error}");
            None
        }
        Err(error) => panic!("{what}: {error}"),
    }
}

#[tokio::test]
#[ignore = "needs the network"]
async fn every_get_endpoint_is_read_whole() {
    let api = client();

    if let Some(signatures) = answer("signatures", api.signatures(None).await) {
        assert!(signatures.max_age.is_some());
        for signature in &signatures.data {
            assert!(signature.id > 0);
            assert!(
                signature.hub().is_some(),
                "a signature in {}",
                signature.out_system_name
            );
        }
        for hub in Hub::ALL {
            if let Some(only) = answer("signatures of a hub", api.signatures(Some(hub)).await) {
                assert!(
                    only.data
                        .iter()
                        .all(|signature| signature.hub() == Some(hub))
                );
            }
        }
    }

    if let Some(observations) = answer("observations", api.observations().await) {
        assert!(
            observations
                .data
                .iter()
                .all(|observation| observation.system_id > 0)
        );
    }

    if let Some(types) = answer(
        "wormhole types",
        api.wormhole_types(&WormholeTypeFilter::default()).await,
    ) {
        assert!(types.data.len() > 50, "{} wormhole types", types.data.len());
    }
    if let Some(found) = answer(
        "wormhole types of a class",
        api.wormhole_types(&WormholeTypeFilter {
            limit: Some(3),
            target: Some(SystemClass::HighSec),
            ..WormholeTypeFilter::default()
        })
        .await,
    ) {
        assert!(found.data.len() <= 3);
        assert!(
            found
                .data
                .iter()
                .all(|wormhole| wormhole.target_system_class == SystemClass::HighSec)
        );
    }

    if let Some(found) = answer("systems", api.systems("Jit", Some(3), Some(Space::K)).await) {
        assert!(found.data.len() <= 3);
        assert!(
            found
                .data
                .iter()
                .all(|system| system.system_name.starts_with("Jit"))
        );
    }

    for period in [Period::Daily, Period::Weekly, Period::Monthly] {
        if let Some(scans) = answer("scan statistics", api.scan_statistics(period, None).await) {
            assert!(!scans.data.is_empty());
        }
    }
    if let Some(donations) = answer("donations", api.donation_statistics().await) {
        assert!(donations.data.daily_goals.is_some());
    }

    if let Some(routes) = answer(
        "route",
        api.route("Jita", "Amarr", Some(RoutePreference::Safer))
            .await,
    ) {
        let route = &routes.data[0];
        assert_eq!((route.from.as_str(), route.to.as_str()), ("Jita", "Amarr"));
        assert_eq!(route.route.len(), route.jumps as usize + 1);
    }
    if let Some(routes) = answer("routes to high-sec", api.highsec_routes("Thera").await) {
        assert!(routes.data.len() <= 5);
    }
    if let Some(routes) = answer(
        "routes to Jove Observatories",
        api.jove_observatory_routes("Jita", None).await,
    ) {
        assert!(routes.data.len() <= 5);
        assert!(routes.data.iter().all(|route| {
            route
                .route
                .last()
                .and_then(|system| system.jove_observatory)
                == Some(true)
        }));
    }
}

/// Asks again a few times while the API has a bad moment: the routing
/// endpoints were seen answering 502 and 503 one minute and 200 the next.
async fn answer_with_retries<T, F, Fut>(what: &str, mut ask: F) -> Option<Cached<T>>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Cached<T>, Error>>,
{
    for attempt in 1..=4 {
        match ask().await {
            Ok(answer) => {
                eprintln!("{what}: ok at attempt {attempt}");
                return Some(answer);
            }
            Err(error) if error.is_transient() => {
                eprintln!("{what}: attempt {attempt}: {error}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(error) => panic!("{what}: {error}"),
        }
    }
    eprintln!("{what}: the API did not answer in 4 attempts, skipped");
    None
}

#[tokio::test]
#[ignore = "needs the network"]
async fn routes_to_signatures_and_to_several_systems() {
    let api = client();
    for hub in Hub::ALL {
        if let Some(routes) = answer_with_retries("routes to signatures", || {
            api.signature_routes("Jita", hub, None)
        })
        .await
        {
            assert!(routes.data.iter().all(|route| route.signature_id.is_some()));
        }
    }
    let request = RouteRequest {
        from: String::from("Zoohen"),
        to: vec![
            String::from("Jita"),
            String::from("Amarr"),
            String::from("Thera"),
        ],
        preference: Some(RoutePreference::Shortest),
    };
    if let Some(routes) = answer_with_retries("several routes", || api.routes(&request)).await {
        assert!(!routes.data.is_empty());
        assert!(routes.data.iter().all(|route| route.from == "Zoohen"));
    }
}

#[tokio::test]
#[ignore = "needs the network"]
async fn an_unknown_system_is_an_error_of_the_api_that_is_not_transient() {
    let error = client()
        .route("No Such System", "Amarr", None)
        .await
        .expect_err("the API took a system that does not exist");
    let Error::Status { status, api } = &error else {
        panic!("not a status error: {error}");
    };
    assert_eq!(*status, 400);
    assert!(api.as_ref().unwrap().name.starts_with("ESAPI-"));
    assert!(!error.is_transient());
}

/// The class of a system in the SDE: the one of the system or, when it has
/// none, the one of its region.
fn sde_class_id(database: &rusqlite::Connection, system_id: i64) -> Option<Option<i64>> {
    database
        .query_row(
            "SELECT s.wormholeClassId, r.wormholeClassId FROM mapSolarSystems s \
             JOIN mapConstellations c ON c.constellationId = s.constellationId \
             JOIN mapRegions r ON r.regionId = c.regionId WHERE s.solarSystemId = ?1",
            [system_id],
            |row| {
                let own: Option<i64> = row.get(0)?;
                let region: Option<i64> = row.get(1)?;
                Ok(own.or(region))
            },
        )
        .ok()
}

#[tokio::test]
#[ignore = "needs the network and SDE_DB"]
async fn the_classes_of_the_api_are_the_ones_of_the_sde() {
    let Ok(path) = std::env::var("SDE_DB") else {
        eprintln!("SDE_DB is not set: nothing to compare with");
        return;
    };
    let database =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let Some(signatures) = answer("signatures", client().signatures(None).await) else {
        return;
    };
    let (mut compared, mut unknown) = (0, 0);
    for signature in &signatures.data {
        let (Some(system_id), Some(class)) = (signature.in_system_id, signature.in_system_class)
        else {
            continue;
        };
        let Some(Some(id)) = sde_class_id(&database, system_id) else {
            unknown += 1;
            continue;
        };
        assert_eq!(
            SystemClass::from_wormhole_class_id(id),
            Some(class),
            "{} (class id {id} in the SDE)",
            signature.in_system_name.as_deref().unwrap_or("?")
        );
        compared += 1;
    }
    eprintln!("{compared} systems compared, {unknown} not in the SDE");
    assert!(compared > 0, "nothing was compared");
}
