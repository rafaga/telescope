//! Checks GitHub for a Telescope release newer than the running one, and
//! reports it to the UI.
//!
//! [`UpdateChecker::check_for_update`] is the network call and the version
//! comparison. [`UpdateChecker::spawn`] runs it on its own thread, with its
//! own single-threaded tokio runtime (as
//! `database_updater::DatabaseUpdater::spawn` and `messages::AuthSpawner`
//! do), so the egui UI thread never waits on the network. The result reaches
//! the UI as a [`Message::NewVersionAvailable`], which opens a dialog and is
//! not logged; nothing is sent when the app is up to date, and a failed check
//! is a warning in the log.

use super::messages::{Message, Type, send_app_message};
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

/// The GitHub repository whose releases are checked (`owner/name`).
const REPO: &str = "rafaga/telescope";

/// How long the whole request may take before it is given up as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The two fields of GitHub's "latest release" payload this check needs.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// Outcome of comparing the running version against the latest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub needs_update: bool,
    pub current: String,
    pub latest: String,
    /// The release's page on GitHub.
    pub url: String,
}

pub struct UpdateChecker {
    client: Client,
    repo: String,
}

impl UpdateChecker {
    /// `repo` is `owner/name`, e.g. `rafaga/telescope`.
    pub fn new(repo: &str) -> Result<Self, reqwest::Error> {
        // GitHub rejects requests without a User-Agent.
        let client = Client::builder()
            .user_agent(concat!("telescope/", env!("CARGO_PKG_VERSION")))
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            repo: repo.to_string(),
        })
    }

    /// Fetches the latest non-prerelease release and compares its tag with
    /// `current_version` (a semver string such as `CARGO_PKG_VERSION`).
    pub async fn check_for_update(
        &self,
        current_version: &str,
    ) -> Result<UpdateInfo, Box<dyn std::error::Error>> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", self.repo);
        let release: Release = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            // A 404 (no release yet) or 403 (rate limit) would otherwise
            // surface as a confusing JSON decoding error.
            .error_for_status()?
            .json()
            .await?;

        Ok(UpdateInfo {
            needs_update: Self::is_newer(current_version, &release.tag_name)?,
            current: current_version.to_string(),
            latest: release.tag_name,
            url: release.html_url,
        })
    }

    /// Whether the release tagged `tag` (with or without a leading `v`) is
    /// newer than `current`.
    fn is_newer(current: &str, tag: &str) -> Result<bool, semver::Error> {
        let current = Version::parse(current)?;
        let remote = Version::parse(tag.trim_start_matches('v'))?;
        Ok(remote > current)
    }

    /// Checks GitHub in the background and, if a newer release exists, tells
    /// the UI through `app_msg`. A failed check (offline, rate limited) is
    /// reported as a warning in the log; being up to date is silent.
    pub fn spawn(app_msg: Arc<Sender<Message>>) {
        let spawned = std::thread::Builder::new()
            .name(String::from("update-checker"))
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!(%error, "failed to build the update-checker runtime");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let _span = tracing::info_span!("update checker").entered();
                    let outcome = match Self::new(REPO) {
                        Ok(checker) => checker
                            .check_for_update(env!("CARGO_PKG_VERSION"))
                            .await
                            .map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    };
                    let message = match outcome {
                        Ok(info) if info.needs_update => Message::NewVersionAvailable(info),
                        Ok(_) => {
                            tracing::debug!("Telescope is up to date");
                            return;
                        }
                        Err(error) => Message::GenericNotification((
                            Type::Warning,
                            String::from("UpdateChecker"),
                            String::from("spawn"),
                            format!("Could not check for a new Telescope release: {error}"),
                        )),
                    };
                    let _ = send_app_message(&app_msg, message).await;
                });
            });
        if let Err(error) = spawned {
            tracing::error!(%error, "failed to start the update-checker thread");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::UpdateChecker;

    #[test]
    fn a_higher_release_is_an_update() {
        assert!(UpdateChecker::is_newer("1.0.0", "1.0.1").unwrap());
        assert!(UpdateChecker::is_newer("1.0.0", "v1.1.0").unwrap());
    }

    #[test]
    fn the_same_or_an_older_release_is_not() {
        assert!(!UpdateChecker::is_newer("1.0.1", "1.0.1").unwrap());
        assert!(!UpdateChecker::is_newer("1.0.1", "v1.0.0").unwrap());
    }

    #[test]
    fn a_release_candidate_is_older_than_its_final_release() {
        assert!(UpdateChecker::is_newer("1.0.0-rc03", "1.0.0").unwrap());
        assert!(!UpdateChecker::is_newer("1.0.0", "1.0.0-rc03").unwrap());
    }

    #[test]
    fn an_unparsable_tag_is_an_error() {
        assert!(UpdateChecker::is_newer("1.0.0", "nightly").is_err());
    }
}
