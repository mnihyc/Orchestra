//! Optional read-only version checks at the updater's configured source, with safe retained facts and one notice per offer.

use orch_api_contract::admin::ReleaseCheck;
use orch_build_info::release::Manifest;
use orch_config::server::Release;
use orch_db::queries::installation;
use orch_db::queries::notifications::people::{self, NewNotification};
use orch_db::{Db, DbError};
use orch_settings::Place;
use orch_types::{Clock, Time};
use serde::{Deserialize, Serialize};
use std::time::Duration as Wait;

/// Which request wants a lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// An administrator explicitly asked, even with scheduled checks off.
    Manual,
    /// Only after the enabled interval has passed.
    WhenDue,
    /// Management's System page reads the same hourly cache even when notices are disabled.
    SystemPage,
}

/// Database and configuration failures prevent even retaining a lookup's result.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Persistence failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The interval could not be resolved.
    #[error(transparent)]
    Setting(#[from] orch_settings::Error),
    /// Stored metadata could not be read or written.
    #[error("installation metadata does not read: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct State {
    view: ReleaseCheck,
    attempted_at: Time,
}

/// The last visible facts, without causing a request to the release site.
pub async fn read(conn: &mut orch_db::Conn) -> Result<Option<ReleaseCheck>, Error> {
    installation::get(conn, "release_check")
        .await?
        .map(serde_json::from_value::<State>)
        .transpose()
        .map(|state| state.map(|state| state.view))
        .map_err(Into::into)
}

/// Whether the optional check is enabled and due since the last attempt (successful or failed).
#[must_use]
pub fn due(now: Time, days: i64, attempted_at: Option<Time>) -> bool {
    days > 0
        && attempted_at.is_none_or(|at| now.get() - at.get() >= crate::spans::days(days as u64))
}

fn release_source(config: &Release) -> String {
    config.source()
}

fn address(config: &Release) -> Result<reqwest::Url, &'static str> {
    let base = release_source(config);
    let source = reqwest::Url::parse(&base)
        .map_err(|_| "the configured release source is not an address")?;
    if !matches!(source.scheme(), "https" | "http")
        || source.host_str().is_none()
        || !source.username().is_empty()
        || source.password().is_some()
        || source.query().is_some()
        || source.fragment().is_some()
    {
        return Err(
            "the configured release source must be an HTTP(S) address without credentials, query or fragment",
        );
    }
    let path = source.path().trim_matches('/');
    let url = if source.host_str() == Some("github.com") && path.ends_with("/releases") {
        let repository = path.trim_end_matches("/releases");
        reqwest::Url::parse(&format!(
            "https://api.github.com/repos/{repository}/releases/latest"
        ))
    } else {
        reqwest::Url::parse(&format!("{base}/latest/download/version.json"))
    }
    .map_err(|_| "the configured release source is not an address")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "the configured release source must be an HTTP(S) address without credentials, query or fragment",
        );
    }
    Ok(url)
}

async fn fetch(config: &Release) -> Result<(String, String), &'static str> {
    let url = address(config)?;
    let github = url.host_str() == Some("api.github.com");
    let client = reqwest::Client::builder()
        .user_agent("Orch-release-check")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Wait::from_secs(config.request_timeout_secs.max(1)))
        .build()
        .map_err(|_| "the release lookup could not be prepared")?;
    // No cookie jar, authorization, machine identity or installation statistics is attached.
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "the release source could not be reached within its time bound")?;
    if !response.status().is_success() {
        return Err("the release source did not return a successful response");
    }
    if response
        .content_length()
        .is_some_and(|length| length > config.manifest_max_bytes as u64)
    {
        return Err("the release manifest exceeds its size bound");
    }
    let mut bytes = Vec::new();
    while let Some(part) = response
        .chunk()
        .await
        .map_err(|_| "the release manifest could not be read within its time bound")?
    {
        if part.len() > config.manifest_max_bytes.saturating_sub(bytes.len()) {
            return Err("the release manifest exceeds its size bound");
        }
        bytes.extend_from_slice(&part);
    }
    if github {
        #[derive(Deserialize)]
        struct GitHubRelease {
            tag_name: String,
            html_url: String,
            draft: bool,
            prerelease: bool,
        }
        let release: GitHubRelease =
            serde_json::from_slice(&bytes).map_err(|_| "GitHub did not return a release")?;
        if release.draft || release.prerelease {
            return Err("the release source did not name a stable published release");
        }
        let version = release.tag_name.trim_start_matches('v').to_owned();
        if !orch_build_info::release::is_release_version(&version) {
            return Err("the release source names an invalid version");
        }
        let notes = reqwest::Url::parse(&release.html_url)
            .map_err(|_| "the release notes address is invalid")?;
        if notes.scheme() != "https" || notes.host_str() != Some("github.com") {
            return Err("the release notes address is invalid");
        }
        return Ok((version, release.html_url));
    }
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| "the response is not a release manifest")?;
    if !manifest.problems().is_empty() {
        return Err("the response is not a valid whole release manifest");
    }
    let notes = format!("{}/tag/{}", release_source(config), manifest.tag);
    Ok((manifest.version, notes))
}

/// How long a check's lease holds: five minutes, far more than the bounded request of a check takes.
const CHECK_LEASE: chrono::Duration = chrono::Duration::minutes(5);

/// Fetches only when requested or enabled and due. A failure preserves successful facts and their time.
pub async fn check(
    db: &Db,
    clock: &dyn Clock,
    config: &Release,
    installed: &str,
    wanted: Wanted,
) -> Result<Option<ReleaseCheck>, Error> {
    let now = clock.time();
    // The same lease serves manual calls and every worker, so two due workers cannot race a notice. It is a
    // lease and not a lock in a transaction: the request to the release source waits on the network, and no
    // transaction is held open across that.
    let lease = orch_db::queries::job_leases::Lease::for_run("release_check", CHECK_LEASE);
    if !lease.hold(db, now.get()).await? {
        let mut conn = db.acquire().await?;
        return read(&mut conn).await;
    }
    let checked = check_held(db, now, config, installed, wanted).await;
    lease.release(db).await?;
    checked
}

/// The check, once its lease is held: what was kept is read in one short statement or two, the release
/// source asked with no transaction open, and the outcome written in one transaction.
async fn check_held(
    db: &Db,
    now: Time,
    config: &Release,
    installed: &str,
    wanted: Wanted,
) -> Result<Option<ReleaseCheck>, Error> {
    let (held, days) = {
        let mut conn = db.acquire().await?;
        let held = installation::get(&mut conn, "release_check")
            .await?
            .map(serde_json::from_value::<State>)
            .transpose()?;
        let days = orch_settings::get::<i64>(&mut conn, Place::System, "release.check_days")
            .await?
            .unwrap_or(0);
        (held, days)
    };
    // Successes and failures share the same bound, across all API processes, manual reads and
    // worker rounds. Repeated page opens cannot turn a failing site into a request storm.
    if let Some(state) = &held
        && now.get() - state.attempted_at.get() < chrono::Duration::hours(1)
    {
        let mut view = state.view.clone();
        view.installed_version = installed.to_owned();
        view.newer = view.offered_version.as_ref().and_then(|version| {
            let offered = semver::Version::parse(version).ok()?;
            let installed = semver::Version::parse(installed).ok()?;
            view.command = (offered >= installed).then(|| format!("./update.sh v{version}"));
            Some(offered > installed)
        });
        return Ok(Some(view));
    }
    if wanted == Wanted::WhenDue && !due(now, days, held.as_ref().map(|state| state.attempted_at)) {
        return Ok(None);
    }
    let mut view = held.map_or_else(
        || ReleaseCheck {
            notes_url: None,
            checked_at: now,
            installed_version: installed.to_owned(),
            offered_version: None,
            newer: None,
            source: String::new(),
            error: None,
            command: None,
        },
        |state| state.view,
    );
    view.installed_version = installed.to_owned();
    // Invalid configuration must not echo a possible embedded credential back to a browser or log.
    let source = address(config).map_or_else(|_| String::new(), |url| url.to_string());
    if view.offered_version.is_none() {
        view.source.clone_from(&source);
    }
    let answer = fetch(config).await.and_then(|(version, notes)| {
        let offered = semver::Version::parse(&version).map_err(|_| "the offered version is invalid")?;
        let current = semver::Version::parse(installed).map_err(|_| "the installed version is invalid")?;
        let previous = view.offered_version.as_deref().and_then(|value| semver::Version::parse(value).ok());
        if offered < current || previous.is_some_and(|previous| offered < previous) {
            return Err("the release source names an older version; the previous successful facts are retained");
        }
        Ok((version, offered > current, notes))
    });
    let mut tx = db.begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    match answer {
        Ok((offered, newer, notes)) => {
            view.source = source;
            view.checked_at = now;
            view.offered_version = Some(offered.clone());
            view.newer = Some(newer);
            view.notes_url = Some(notes);
            view.error = None;
            view.command = Some(format!("./update.sh v{offered}"));
            if newer {
                notice(&mut tx, &offered, now).await?;
            }
        }
        Err(reason) => view.error = Some(reason.to_owned()),
    }
    installation::put(
        &mut tx,
        "release_check",
        &serde_json::to_value(State {
            view: view.clone(),
            attempted_at: now,
        })?,
        now.get(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Some(view))
}

async fn notice(conn: &mut orch_db::Conn, offered: &str, now: Time) -> Result<(), DbError> {
    for (user_id, locale) in installation::administrators(conn).await? {
        let (title, body) = if locale == "zh-CN" {
            (
                format!("Orch {offered} 已发布"),
                "管理员可以在系统主机上运行更新命令。系统未安装任何更新。".to_owned(),
            )
        } else {
            (format!("Orch {offered} is available"), "An operator can run the update command on the system's host. Nothing was installed.".to_owned())
        };
        let data =
            serde_json::json!({"reason":"release_available", "version":offered, "path":"/admin"});
        people::announce(
            conn,
            &NewNotification {
                user_id,
                kind: "announcement",
                node_id: None,
                thread_id: None,
                approval_id: None,
                chat_id: None,
                title: &title,
                body: &body,
                data: &data,
                dedupe_key: &format!("release.available:{offered}"),
                at: now.get(),
            },
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_empty_sources_use_the_repositorys_github_release_api_without_a_browser_request()
    {
        let config = Release::default();
        let address = address(&config).unwrap();
        assert_eq!(address.host_str(), Some("api.github.com"));
        assert!(address.path().ends_with("/releases/latest"));
        assert_eq!(
            super::address(&Release {
                url: String::new(),
                ..config
            })
            .unwrap(),
            address
        );
    }

    #[test]
    fn an_operator_source_uses_the_updaters_manifest_layout_and_refuses_embedded_identity() {
        let config = Release {
            url: "https://releases.example.test/orch".into(),
            ..Release::default()
        };
        assert_eq!(
            address(&config).unwrap().as_str(),
            "https://releases.example.test/orch/latest/download/version.json"
        );
        for url in [
            "https://secret@github.com/example/repo/releases",
            "https://releases.example.test/orch?token=secret",
            "file:///tmp/version.json",
        ] {
            assert!(
                address(&Release {
                    url: url.into(),
                    ..config.clone()
                })
                .is_err()
            );
        }
    }
}
