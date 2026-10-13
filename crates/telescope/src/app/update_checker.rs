//! Checks GitHub for a Telescope release newer than the running one, and
//! reports it to the UI.
//!
//! [`UpdateChecker::check_for_update`] is the network call and the version
//! comparison. [`UpdateChecker::announce`] turns it into the message for the
//! UI: a [`Message::NewVersionAvailable`] (which opens a dialog and is not
//! logged) only when a release newer than the running one, and not yet
//! announced, exists. The `housekeeping` thread runs it periodically, so the
//! egui UI thread never waits on the network.

use super::messages::Message;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

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

    /// Checks GitHub and decides what the UI should hear about it. Returns
    /// the message that opens the update dialog only when a release newer
    /// than the running one exists and `announced` (the last tag already
    /// shown) isn't it, so a periodic check never re-opens the dialog for a
    /// release the user has seen. Being up to date, or a release already
    /// announced, yields `None`.
    pub async fn announce(
        &self,
        current_version: &str,
        announced: &mut Option<String>,
    ) -> Result<Option<Message>, Box<dyn std::error::Error>> {
        let info = self.check_for_update(current_version).await?;
        Ok(Self::announcement(info, announced))
    }

    /// The dialog message for `info`, unless it is up to date or its release
    /// is the one in `announced`; records the release as announced.
    fn announcement(info: UpdateInfo, announced: &mut Option<String>) -> Option<Message> {
        if !info.needs_update || announced.as_deref() == Some(info.latest.as_str()) {
            return None;
        }
        *announced = Some(info.latest.clone());
        Some(Message::NewVersionAvailable(info))
    }

    /// The `repo` the running Telescope's releases are published in.
    pub fn for_telescope() -> Result<Self, reqwest::Error> {
        Self::new(REPO)
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateChecker, UpdateInfo};
    use crate::app::messages::Message;

    fn info(needs_update: bool, latest: &str) -> UpdateInfo {
        UpdateInfo {
            needs_update,
            current: String::from("1.0.0"),
            latest: latest.to_string(),
            url: String::new(),
        }
    }

    #[test]
    fn being_up_to_date_announces_nothing() {
        let mut announced = None;
        assert!(UpdateChecker::announcement(info(false, "1.0.0"), &mut announced).is_none());
        assert_eq!(announced, None);
    }

    #[test]
    fn a_new_release_is_announced_once() {
        let mut announced = None;
        let first = UpdateChecker::announcement(info(true, "1.1.0"), &mut announced);
        assert!(matches!(first, Some(Message::NewVersionAvailable(_))));
        assert_eq!(announced.as_deref(), Some("1.1.0"));
        // The next periodic check must not re-open the dialog.
        assert!(UpdateChecker::announcement(info(true, "1.1.0"), &mut announced).is_none());
        // A newer release after that is announced again.
        assert!(UpdateChecker::announcement(info(true, "1.2.0"), &mut announced).is_some());
    }

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
