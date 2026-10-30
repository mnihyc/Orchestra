//! The agent programs' release channels: which builds their vendors publish, with the vendors' own checksums.
//!
//! Orch reads only the channels requested by effective newer-build policies, or when a system admin asks, and keeps
//! every Linux build it finds as a build the tool cache can fetch, untested until its check passes. What a program's
//! channels are is data of its manifest: a GitHub repository whose releases carry assets, an npm package, a PyPI
//! project, or a manifest its vendor publishes. One generic reader for each kind of channel walks that data, and
//! nothing here names a vendor or a program. A build its vendor gives nothing to verify it by is never kept, and a
//! tool cache refuses a download whose checksum differs. Where this server cannot reach the vendors,
//! `agents.harness_mirror` names a server that serves the same files under its own address, with each origin's host
//! as the first part of the path.
//! A mirror transports bytes without publishing vendor checksum authority unless the administrator explicitly trusts it.

mod github;
mod npm;
mod pypi;
mod signed;

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use orch_agent_programs::{Arch, Channel, Programs};
use orch_api_contract::admin::{
    HarnessArch, HarnessBuild, HarnessBuildSource, HarnessBuildStatus, HarnessChannelSchedule,
    HarnessChannelsStatus,
};
use orch_api_contract::resources::MachineOs;
use orch_db::Conn;
use orch_db::queries::resources::management;
use orch_db::queries::settings as rows;
use orch_types::{Json, Time};
use serde::{Deserialize, Serialize};

/// How long one request to a vendor may take before the read is given up and tried at the next one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the download of a build may take while its digests are made. A build is as large as its vendor made it, so
/// this is far longer than a request for a document; it is a default, and the transfer is never held in memory.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// The most a channel's answer may be read as: a registry's document of a package with many versions is large, and a
/// release listing is small. A vendor that answers with more is not read.
const ANSWER_MOST_BYTES: usize = 64 * 1024 * 1024;
/// How often the schedule looks whether a read is due; the interval itself is the setting's.
const LOOK_EVERY: Duration = Duration::from_secs(15 * 60);
/// Internal read metadata lives beside other system metadata, outside the user-editable setting catalogue.
const READ_STATE_KEY: &str = "harness_channels:read";

/// What a vendor's address becomes under a mirror: the mirror's address, then the vendor's host and path.
#[must_use]
pub fn mirrored(url: &str, mirror: Option<&str>) -> String {
    match (
        mirror.map(|mirror| mirror.trim_end_matches('/')),
        url.split_once("://"),
    ) {
        (Some(mirror), Some((_, rest))) if !mirror.is_empty() => format!("{mirror}/{rest}"),
        _ => url.to_owned(),
    }
}

/// The processor of the API's builds, for a manifest's.
const fn arch_of(arch: Arch) -> HarnessArch {
    match arch {
        Arch::X86_64 => HarnessArch::X86_64,
        Arch::Aarch64 => HarnessArch::Aarch64,
    }
}

fn build(kind: &str, version: &str, arch: Arch, url: String, sha256: &str) -> HarnessBuild {
    HarnessBuild {
        kind: kind.to_owned(),
        version: version.to_owned(),
        os: MachineOs::Linux,
        arch: arch_of(arch),
        url,
        sha256: sha256.to_ascii_lowercase(),
        is_default: false,
        source: HarnessBuildSource::Channel,
        status: HarnessBuildStatus::Untested,
        check: None,
        carry: None,
    }
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|each| each.is_ascii_hexdigit())
}

/// Whether `text` matches `pattern`, in which `*` stands for any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let Some(first) = parts.next() else {
        return text.is_empty();
    };
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// A template with the places `pairs` name filled in: `{version}` and the like.
fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}

/// A read of one program's channels: who asks, which program, and the mirror to go through.
#[derive(Clone, Copy)]
struct Ask<'a> {
    client: &'a reqwest::Client,
    mirror: Option<&'a str>,
    kind: &'a str,
    /// The keys of the channel builds already kept: a reader that must download a build to know its checksum does not
    /// download one it has.
    kept: &'a BTreeSet<String>,
}

impl Ask<'_> {
    /// Whether the build of `version` for `arch` is kept already.
    fn has(&self, version: &str, arch: Arch) -> bool {
        self.kept.contains(&channel_key(&build(
            self.kind,
            version,
            arch,
            String::new(),
            "",
        )))
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, String> {
        let url = mirrored(url, self.mirror);
        self.client
            .get(&url)
            .header(reqwest::header::USER_AGENT, "orch")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("{url}: {error}"))
    }

    async fn bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let answer = self.get(url).await?;
        if answer
            .content_length()
            .is_some_and(|size| size > ANSWER_MOST_BYTES as u64)
        {
            return Err(format!("{url}: the answer is larger than a reader takes"));
        }
        let bytes = answer
            .bytes()
            .await
            .map_err(|error| format!("{url}: {error}"))?;
        if bytes.len() > ANSWER_MOST_BYTES {
            return Err(format!("{url}: the answer is larger than a reader takes"));
        }
        Ok(bytes.to_vec())
    }

    /// The integrity value of what `url` serves (`sha512-` and the base64 of its SHA-512, as a package registry
    /// writes it) and its SHA-256, made as it arrives: none of it is held, whatever its size.
    async fn digests(&self, url: &str) -> Result<(String, String), String> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256, Sha512};
        let url = mirrored(url, self.mirror);
        let mut answer = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, "orch")
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| format!("{url}: {error}"))?;
        let (mut long, mut short) = (Sha512::new(), Sha256::new());
        while let Some(piece) = answer
            .chunk()
            .await
            .map_err(|error| format!("{url}: {error}"))?
        {
            long.update(&piece);
            short.update(&piece);
        }
        Ok((
            format!(
                "sha512-{}",
                base64::engine::general_purpose::STANDARD.encode(long.finalize())
            ),
            hex::encode(short.finalize()),
        ))
    }

    async fn json(&self, url: &str) -> Result<Json, String> {
        let bytes = self.bytes(url).await?;
        serde_json::from_slice(&bytes).map_err(|error| format!("{url}: {error}"))
    }

    async fn text(&self, url: &str) -> Result<String, String> {
        let bytes = self.bytes(url).await?;
        String::from_utf8(bytes).map_err(|error| format!("{url}: {error}"))
    }
}

/// The kinds of the programs whose manifests name a channel to read.
#[must_use]
pub fn readable(programs: &Programs) -> BTreeSet<String> {
    programs
        .programs()
        .filter(|manifest| !manifest.builds.channels.is_empty())
        .map(|manifest| manifest.kind.clone())
        .collect()
}

/// Reads every program's channels. A vendor that cannot be read is said in the answer's second part, and the others'
/// builds are kept all the same.
pub async fn read(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    kept: &BTreeSet<String>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    read_for(client, programs, mirror, &readable(programs), kept).await
}

/// Reads only the named programs' channels; the explicit admin action calls [`read`] for all.
async fn read_channels(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    kinds: &BTreeSet<String>,
    kept: &BTreeSet<String>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    let mut builds = Vec::new();
    let mut failures = Vec::new();
    for manifest in programs
        .programs()
        .filter(|manifest| kinds.contains(&manifest.kind))
    {
        let ask = Ask {
            client,
            mirror,
            kind: &manifest.kind,
            kept,
        };
        for channel in &manifest.builds.channels {
            let (found, failed) = match channel {
                Channel::GithubRelease(channel) => github::read(ask, channel).await,
                Channel::Npm(channel) => npm::read(ask, channel).await,
                Channel::Pypi(channel) => pypi::read(ask, channel).await,
                Channel::SignedManifest(channel) => signed::read(ask, channel).await,
            };
            builds.extend(found);
            failures.extend(failed);
        }
    }
    (builds, failures)
}

/// Reads exactly the requested release, never substituting a channel's newest build.
pub async fn read_exact(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    kind: &str,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    read_exact_configured(client, programs, mirror, false, kind, version).await
}

/// Reads one exact version under the administrator's revocable publisher choice.
pub async fn read_exact_configured(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    trust_mirror: bool,
    kind: &str,
    version: &str,
) -> Result<Vec<HarnessBuild>, String> {
    let mirrored_publisher = mirror.is_some_and(|mirror| !mirror.trim().is_empty());
    if mirrored_publisher && !trust_mirror {
        return Err("a mirror transports builds but cannot publish vendor checksums; import a gathered vendor checksum list, or explicitly trust the mirror as a publisher".to_owned());
    }
    let Some(manifest) = programs
        .program(kind)
        .filter(|manifest| manifest.builds.exact_fetch())
    else {
        return Err(format!(
            "{kind} has no vendor channel; add a verified build instead"
        ));
    };
    // An exact release is fetched whether or not one is kept: it is what the person asked for.
    let none = BTreeSet::new();
    let ask = Ask {
        client,
        mirror,
        kind,
        kept: &none,
    };
    let mut builds = Vec::new();
    let mut reachable = false;
    for channel in &manifest.builds.channels {
        let answer = match channel {
            Channel::GithubRelease(channel) => github::read_exact(ask, channel, version).await,
            Channel::Npm(channel) => npm::read_exact(ask, channel, version).await,
            Channel::Pypi(channel) => pypi::read_exact(ask, channel, version).await,
            Channel::SignedManifest(channel) => signed::read_exact(ask, channel, version).await,
        };
        if let Ok(found) = answer {
            reachable = true;
            builds.extend(found.into_iter().filter(|build| {
                orch_agent_programs::versions::same_release(&build.version, version)
            }));
        }
    }
    if !reachable {
        return Err(format!(
            "{kind} {version} could not be fetched; check the release, mirror and connection"
        ));
    }
    if builds.is_empty() {
        return Err(format!(
            "{kind} {version} is unavailable: no release with a vendor checksum"
        ));
    }
    if mirrored_publisher {
        for build in &mut builds {
            build.source = HarnessBuildSource::Mirror;
        }
    }
    Ok(builds)
}

/// Whether the operator deliberately trusts the configured mirror as a checksum publisher.
pub async fn trusts_mirror(conn: &mut Conn) -> Result<bool, orch_db::DbError> {
    orch_settings::get::<bool>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_TRUST_MIRROR,
    )
    .await
    .map(|value| value.unwrap_or(false))
    .map_err(|error| orch_db::DbError::Other(sqlx::Error::Protocol(error.to_string())))
}

/// Whether a kept executable has independently established checksum authority.
#[must_use]
pub fn checksum_trusted(value: &Json, trust_mirror: bool) -> bool {
    match value.get("source").and_then(Json::as_str) {
        Some("admin") => matches!(
            value.get("checksum_origin").and_then(Json::as_str),
            Some("admin" | "vendor")
        ),
        Some("channel") => value.get("checksum_origin").and_then(Json::as_str) == Some("vendor"),
        Some("mirror") => {
            trust_mirror && value.get("checksum_origin").and_then(Json::as_str) == Some("mirror")
        }
        _ => false,
    }
}

/// Reads requested program channels without granting a mirror checksum authority.
pub async fn read_for(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    kinds: &BTreeSet<String>,
    kept: &BTreeSet<String>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    read_for_configured(client, programs, mirror, false, kinds, kept).await
}

/// Reads all declared channels under the administrator's revocable publisher choice.
pub async fn read_configured(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    trust_mirror: bool,
    kept: &BTreeSet<String>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    read_for_configured(
        client,
        programs,
        mirror,
        trust_mirror,
        &readable(programs),
        kept,
    )
    .await
}

/// Reads only requested programs and retains the checksum publisher's identity.
pub async fn read_for_configured(
    client: &reqwest::Client,
    programs: &Programs,
    mirror: Option<&str>,
    trust_mirror: bool,
    kinds: &BTreeSet<String>,
    kept: &BTreeSet<String>,
) -> (Vec<HarnessBuild>, Vec<String>) {
    let mirrored_publisher = mirror.is_some_and(|mirror| !mirror.trim().is_empty());
    if mirrored_publisher && !trust_mirror {
        return (Vec::new(), vec!["a mirror transports builds but cannot publish vendor checksums; import a gathered vendor checksum list, or explicitly trust the mirror as a publisher".to_owned()]);
    }
    let (mut builds, failures) = read_channels(client, programs, mirror, kinds, kept).await;
    if mirrored_publisher {
        for build in &mut builds {
            build.source = HarnessBuildSource::Mirror;
        }
    }
    (builds, failures)
}

/// Rewrites only a program's known vendor download origins through its configured transport mirror.
#[must_use]
pub fn download_url(programs: &Programs, build: &HarnessBuild, mirror: Option<&str>) -> String {
    let same_origin = |left: &str, right: &str| {
        reqwest::Url::parse(left)
            .ok()
            .zip(reqwest::Url::parse(right).ok())
            .is_some_and(|(left, right)| left.origin() == right.origin())
    };
    let known = programs.program(&build.kind).is_some_and(|manifest| {
        manifest
            .builds
            .channels
            .iter()
            .any(|channel| match channel {
                Channel::GithubRelease(channel) => {
                    build
                        .url
                        .starts_with(&format!("https://github.com/{}/", channel.repository))
                        || same_origin(&build.url, &channel.api)
                }
                Channel::SignedManifest(channel) => build
                    .url
                    .starts_with(&format!("{}/", channel.base.trim_end_matches('/'))),
                Channel::Npm(channel) => same_origin(&build.url, &channel.registry),
                Channel::Pypi(channel) => {
                    same_origin(&build.url, &channel.index)
                        || build.url.starts_with("https://files.pythonhosted.org/")
                }
            })
    });
    if known {
        mirrored(&build.url, mirror)
    } else {
        build.url.clone()
    }
}

/// The key a build read from a channel is kept under.
#[must_use]
pub fn channel_key(build: &HarnessBuild) -> String {
    let word = |value: Json| value.as_str().unwrap_or_default().to_owned();
    format!(
        "{}{}:{}:{}:{}",
        management::HARNESS_CHANNEL_PREFIX,
        build.kind,
        build.version,
        word(serde_json::json!(build.os)),
        word(serde_json::json!(build.arch)),
    )
}

/// Keeps the builds that were read; the answer is how many were new.
pub async fn keep(
    conn: &mut Conn,
    builds: &[HarnessBuild],
    at: DateTime<Utc>,
) -> Result<usize, orch_db::DbError> {
    let mut new = 0;
    for build in builds {
        let mut value = serde_json::to_value(build)
            .map_err(|error| orch_db::DbError::Other(sqlx::Error::Decode(error.into())))?;
        value["checksum_origin"] = Json::from(if build.source == HarnessBuildSource::Mirror {
            "mirror"
        } else {
            "vendor"
        });
        let key = channel_key(build);
        if let Some(old) = orch_db::queries::settings::row(conn, None, &key).await? {
            // Old mirrored metadata has no authority. A fresh vendor read can establish the anchor,
            // and vendor authority replaces a previously delegated mirror checksum.
            if old.value.get("checksum_origin").is_none()
                || (old.value["checksum_origin"] == "mirror"
                    && build.source == HarnessBuildSource::Channel)
            {
                let written = orch_db::queries::settings::put(
                    conn,
                    None,
                    &key,
                    &orch_db::queries::settings::Stored {
                        value: &value,
                        locked: false,
                        by: None,
                        at,
                    },
                    Some(old.revision),
                )
                .await?;
                if matches!(written, orch_db::queries::settings::Put::Written(_)) {
                    new += 1;
                }
            }
            continue;
        }
        if management::put_channel_build(conn, &key, &value, at).await? {
            new += 1;
        }
    }
    Ok(new)
}

/// The keys of the channel builds that are kept, for a read to leave alone what it has.
///
/// # Errors
/// The database failed.
pub async fn kept_keys(conn: &mut Conn) -> Result<BTreeSet<String>, orch_db::DbError> {
    Ok(management::stored_harness_builds(conn)
        .await?
        .into_iter()
        .map(|(key, _)| key)
        .filter(|key| key.starts_with(management::HARNESS_CHANNEL_PREFIX))
        .collect())
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ReadState {
    last_attempt_at: Option<Time>,
    last_read_at: Option<Time>,
}

fn decode_error(error: impl std::error::Error + Send + Sync + 'static) -> orch_db::DbError {
    orch_db::DbError::Other(sqlx::Error::Decode(Box::new(error)))
}

async fn read_state(conn: &mut Conn) -> Result<(ReadState, i64), orch_db::DbError> {
    match rows::row(conn, None, READ_STATE_KEY).await? {
        Some(row) => Ok((
            serde_json::from_value(row.value).map_err(decode_error)?,
            row.revision,
        )),
        None => Ok((ReadState::default(), 0)),
    }
}

fn newer_in(value: &Json) -> BTreeSet<String> {
    value
        .as_object()
        .into_iter()
        .flat_map(|entries| entries.iter())
        .filter(|(_, value)| {
            matches!(
                value.get("use").and_then(Json::as_str),
                Some("newest" | "newest_checked")
            )
        })
        .map(|(kind, _)| kind.clone())
        .collect()
}

fn has_pin(value: &Json) -> bool {
    value.as_object().is_some_and(|entries| {
        entries
            .values()
            .any(|choice| choice.get("use").and_then(Json::as_str) == Some("exact"))
    })
}

async fn requested(conn: &mut Conn) -> Result<(BTreeSet<String>, bool), orch_db::DbError> {
    let key = orch_settings_contract::registry::keys::AGENTS_HARNESS_VERSIONS;
    let system = orch_settings::get::<Json>(conn, orch_settings::Place::System, key)
        .await
        .map_err(decode_error)?
        .unwrap_or(Json::Null);
    let mut wanted = newer_in(&system);
    let mut pinned = has_pin(&system);
    // Only explicit possible opt-ins need resolution; inherited choices are already covered by their parent.
    // Resolving them honors locks and avoids treating an ineffective old override as consent to a request.
    for place in rows::harness_version_places(conn, key).await? {
        if let Some(value) =
            orch_settings::get::<Json>(conn, orch_settings::Place::Node(place), key)
                .await
                .map_err(decode_error)?
        {
            wanted.extend(newer_in(&value));
            pinned |= has_pin(&value);
        }
    }
    let environments: Vec<orch_types::Id> = sqlx::query_scalar("SELECT e.id FROM environments e JOIN nodes n ON n.id=e.home_node_id WHERE e.deleted_at IS NULL AND e.config ? $1 AND n.deleted_at IS NULL AND n.archived_at IS NULL AND NOT EXISTS (SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND (a.deleted_at IS NOT NULL OR a.archived_at IS NOT NULL))")
        .bind(key).fetch_all(&mut *conn).await?;
    for environment in environments {
        if let Some(value) =
            orch_settings::get::<Json>(conn, orch_settings::Place::Environment(environment), key)
                .await
                .map_err(decode_error)?
        {
            wanted.extend(newer_in(&value));
            pinned |= has_pin(&value);
        }
    }
    Ok((wanted, pinned))
}

/// Of the programs that are asked for, those whose manifests name a channel to read.
fn known(programs: &Programs, kinds: &BTreeSet<String>) -> BTreeSet<String> {
    let readable = readable(programs);
    kinds
        .iter()
        .filter(|kind| readable.contains(*kind))
        .cloned()
        .collect()
}

async fn policy(
    conn: &mut Conn,
) -> Result<(BTreeSet<String>, i64, HarnessChannelSchedule), orch_db::DbError> {
    let (wanted, pinned) = requested(conn).await?;
    let programs = orch_db::programs::registry(conn).await?;
    let key = orch_settings_contract::registry::keys::AGENTS_HARNESS_CHANNEL_HOURS;
    let hours = orch_settings::get::<i64>(conn, orch_settings::Place::System, key)
        .await
        .map_err(decode_error)?
        .or_else(|| {
            orch_settings_contract::registry::get(key)
                .and_then(|entry| entry.default_value())
                .and_then(|value| value.as_i64())
        })
        .ok_or_else(|| {
            orch_db::DbError::Other(sqlx::Error::Protocol(
                "no release interval default".to_owned(),
            ))
        })?;
    let reason = if hours <= 0 {
        HarnessChannelSchedule::Disabled
    } else if wanted.is_empty() {
        if pinned {
            HarnessChannelSchedule::Pinned
        } else {
            HarnessChannelSchedule::Shipped
        }
    } else if known(&programs, &wanted).is_empty() {
        HarnessChannelSchedule::NoChannel
    } else {
        HarnessChannelSchedule::Newer
    };
    Ok((wanted, hours, reason))
}

/// Reads channel metadata and effective policy without contacting a vendor.
///
/// # Errors
/// The settings or stored metadata could not be read.
pub async fn status(conn: &mut Conn) -> Result<HarnessChannelsStatus, orch_db::DbError> {
    let (wanted, interval_hours, reason) = policy(conn).await?;
    let (state, _) = read_state(conn).await?;
    Ok(HarnessChannelsStatus {
        last_read_at: state.last_read_at,
        reason,
        interval_hours,
        requested: wanted.into_iter().collect(),
    })
}

fn due(now: DateTime<Utc>, state: &ReadState, hours: i64) -> bool {
    hours > 0
        && state
            .last_attempt_at
            .as_ref()
            .is_none_or(|at| now.signed_duration_since(at.get()).num_hours() >= hours)
}

/// Keeps the latest attempted/successful read, including explicit reads while the schedule is off.
///
/// # Errors
/// Metadata could not be stored.
pub async fn record_read(
    conn: &mut Conn,
    at: DateTime<Utc>,
    succeeded: bool,
) -> Result<(), orch_db::DbError> {
    loop {
        let (mut state, revision) = read_state(conn).await?;
        let latest = state
            .last_attempt_at
            .as_ref()
            .map_or(at, |previous| previous.get().max(at));
        state.last_attempt_at = Some(Time::new(latest));
        if succeeded {
            state.last_read_at = Some(Time::new(
                state
                    .last_read_at
                    .as_ref()
                    .map_or(at, |previous| previous.get().max(at)),
            ));
        }
        let value = serde_json::to_value(&state).map_err(decode_error)?;
        match rows::put(
            conn,
            None,
            READ_STATE_KEY,
            &rows::Stored {
                value: &value,
                locked: false,
                by: None,
                at,
            },
            Some(revision),
        )
        .await?
        {
            rows::Put::Written(_) => return Ok(()),
            rows::Put::Stale => {}
        }
    }
}

/// Performs one scheduled look, making no channel request without effective demand and a due interval.
///
/// # Errors
/// Policy, build records or read metadata could not be read or stored.
pub async fn scheduled_look(
    app: &crate::state::App,
    client: &reqwest::Client,
) -> Result<bool, orch_db::DbError> {
    let now = app.clock.now();
    let mut conn = app.db.acquire().await?;
    let (wanted, hours, reason) = policy(&mut conn).await?;
    let programs = orch_db::programs::registry(&mut conn).await?;
    let kinds = known(&programs, &wanted);
    let kept = kept_keys(&mut conn).await?;
    let (mut state, revision) = read_state(&mut conn).await?;
    let mut read_now = reason == HarnessChannelSchedule::Newer && due(now, &state, hours);
    let mut mirror = None;
    let mut trust_mirror = false;
    if read_now {
        state.last_attempt_at = Some(Time::new(now));
        let value = serde_json::to_value(&state).map_err(decode_error)?;
        read_now = matches!(
            rows::put(
                &mut conn,
                None,
                READ_STATE_KEY,
                &rows::Stored {
                    value: &value,
                    locked: false,
                    by: None,
                    at: now
                },
                Some(revision)
            )
            .await?,
            rows::Put::Written(_)
        );
        if read_now {
            mirror = orch_settings::get::<String>(
                &mut conn,
                orch_settings::Place::System,
                orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
            )
            .await
            .map_err(decode_error)?;
            trust_mirror = trusts_mirror(&mut conn).await?;
        }
    }
    drop(conn);
    if read_now {
        let (builds, failures) = read_for_configured(
            client,
            &programs,
            mirror.as_deref(),
            trust_mirror,
            &kinds,
            &kept,
        )
        .await;
        for failure in &failures {
            tracing::warn!(%failure, "a requested release channel could not be read");
        }
        let mut conn = app.db.acquire().await?;
        keep(&mut conn, &builds, app.clock.now()).await?;
        record_read(
            &mut conn,
            app.clock.now(),
            !builds.is_empty() || failures.is_empty(),
        )
        .await?;
    }
    // Cache checks may fetch builds too: they follow current demand, even when the channel interval is zero.
    let current = {
        let mut conn = app.db.acquire().await?;
        let programs = orch_db::programs::registry(&mut conn).await?;
        known(&programs, &requested(&mut conn).await?.0)
    };
    if !current.is_empty() {
        crate::harness_checks::check_requested(app, &current).await;
    }
    Ok(read_now)
}

/// Watches local policy; only an effective request for newer builds permits scheduled vendor reads.
pub async fn keep_reading(app: crate::state::App, stop: tokio_util::sync::CancellationToken) {
    let client = reqwest::Client::new();
    let mut every = tokio::time::interval(LOOK_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    every.tick().await;
    loop {
        tokio::select! { () = stop.cancelled() => return, _ = every.tick() => {} }
        if let Err(error) = scheduled_look(&app, &client).await {
            tracing::warn!(%error, "the requested release channels could not be refreshed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CLAUDE_RELEASES: &str = "https://downloads.claude.ai/claude-code-releases";
    fn codex_builds(releases: &Json, mirror: Option<&str>) -> Vec<HarnessBuild> {
        let program = Programs::builtin().program("codex").unwrap();
        let Channel::GithubRelease(channel) = &program.builds.channels[0] else {
            panic!("the fixture declares its channel")
        };
        github::builds(channel, "codex", releases, &Default::default(), mirror)
    }
    fn claude_builds(manifest: &Json, mirror: Option<&str>) -> Vec<HarnessBuild> {
        let program = Programs::builtin().program("claude-code").unwrap();
        let Channel::SignedManifest(channel) = &program.builds.channels[0] else {
            panic!("the fixture declares its channel")
        };
        signed::builds(channel, "claude-code", manifest, mirror)
    }
    const SHA: &str = "3ec55053109608d5038f7e50750f7d25f768dfd740fef071aa710e2559b63aac";

    #[test]
    fn old_channel_rows_have_no_checksum_authority_and_mirror_trust_is_explicit() {
        assert!(!checksum_trusted(
            &json!({"source":"channel", "sha256":SHA}),
            false
        ));
        assert!(checksum_trusted(
            &json!({"source":"channel", "checksum_origin":"vendor"}),
            false
        ));
        assert!(!checksum_trusted(
            &json!({"source":"admin", "sha256":SHA}),
            false
        ));
        assert!(checksum_trusted(
            &json!({"source":"admin", "checksum_origin":"admin", "sha256":SHA}),
            false
        ));
        let mirrored = json!({"source":"mirror", "checksum_origin":"mirror"});
        assert!(!checksum_trusted(&mirrored, false));
        assert!(checksum_trusted(&mirrored, true));
    }

    #[test]
    fn codex_releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
        let releases = json!([
            { "tag_name": "rust-v0.165.0-alpha.1", "draft": false, "prerelease": true, "assets": [
                { "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.165.0-alpha.1/x" } ] },
            { "tag_name": "rust-v0.164.0", "draft": false, "prerelease": false, "assets": [
                { "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/codex-package-x86_64-unknown-linux-musl.tar.gz" },
                { "name": "codex-package-aarch64-unknown-linux-musl.tar.gz", "digest": null,
                  "browser_download_url": "https://github.com/openai/codex/releases/download/rust-v0.164.0/a" },
                { "name": "codex-package-x86_64-unknown-linux-musl-bazel.tar.gz", "digest": format!("sha256:{SHA}"),
                  "browser_download_url": "https://github.com/x" } ] }
        ]);
        let builds = codex_builds(&releases, None);
        assert_eq!(
            builds.len(),
            1,
            "a pre-release, an asset without a checksum and other packages are left out"
        );
        assert_eq!(
            (builds[0].version.as_str(), builds[0].arch),
            ("0.164.0", HarnessArch::X86_64)
        );
        assert_eq!(builds[0].sha256, SHA);
        assert_eq!(builds[0].source, HarnessBuildSource::Channel);
        assert_eq!(builds[0].status, HarnessBuildStatus::Untested);
        let under = codex_builds(&releases, Some("https://mirror.example.invalid/releases/"));
        assert_eq!(
            under[0].url,
            "https://mirror.example.invalid/releases/github.com/openai/codex/releases/download/rust-v0.164.0/codex-package-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn a_claude_code_manifest_gives_its_linux_programs_with_their_checksums() {
        let manifest = json!({ "version": "2.1.309", "platforms": {
            "linux-x64": { "binary": "claude", "checksum": SHA, "size": 1 },
            "linux-arm64": { "binary": "claude", "checksum": "not a checksum", "size": 1 },
            "darwin-arm64": { "binary": "claude", "checksum": SHA, "size": 1 } } });
        let builds = claude_builds(&manifest, None);
        assert_eq!(builds.len(), 1);
        assert_eq!(
            builds[0].url,
            format!("{CLAUDE_RELEASES}/2.1.309/linux-x64/claude")
        );
        assert_eq!(
            (builds[0].kind.as_str(), builds[0].version.as_str()),
            ("claude-code", "2.1.309")
        );
        assert!(claude_builds(&json!({}), None).is_empty());
    }

    #[test]
    fn a_mirror_serves_each_vendor_under_its_host() {
        let url = "https://api.github.com/repos/example/tool/releases?per_page=20";
        assert_eq!(mirrored(url, None), url);
        assert_eq!(mirrored(url, Some("")), url);
        assert_eq!(
            mirrored(
                "https://downloads.example.test/releases/latest",
                Some("http://10.0.0.5/m")
            ),
            "http://10.0.0.5/m/downloads.example.test/releases/latest"
        );
    }

    #[test]
    fn a_pattern_may_stand_for_what_varies_in_a_file_name() {
        assert!(wildcard(
            "*manylinux*_x86_64.whl",
            "p-1.0-py3-none-manylinux_2_28_x86_64.whl"
        ));
        assert!(!wildcard(
            "*manylinux*_x86_64.whl",
            "p-1.0-py3-none-manylinux_2_28_aarch64.whl"
        ));
        assert!(wildcard("exact.tar.gz", "exact.tar.gz"));
        assert!(!wildcard("exact.tar.gz", "exact.tar.gz.sig"));
        assert!(wildcard("*", "anything"));
    }

    #[test]
    fn a_build_that_is_kept_is_known_by_its_key_for_its_version_and_processor_alone() {
        let kept = BTreeSet::from([channel_key(&build(
            "tool",
            "1.0.0",
            Arch::X86_64,
            String::new(),
            "",
        ))]);
        let client = reqwest::Client::new();
        let ask = Ask {
            client: &client,
            mirror: None,
            kind: "tool",
            kept: &kept,
        };
        assert!(ask.has("1.0.0", Arch::X86_64));
        assert!(!ask.has("1.0.1", Arch::X86_64));
        assert!(!ask.has("1.0.0", Arch::Aarch64));
    }

    #[test]
    fn the_programs_orch_ships_name_a_channel_each_but_the_fake() {
        let kinds = readable(Programs::builtin());
        assert_eq!(
            kinds.into_iter().collect::<Vec<_>>(),
            ["claude-code", "codex", "dsh"]
        );
    }
}
