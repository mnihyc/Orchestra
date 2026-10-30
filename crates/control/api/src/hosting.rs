//! What the engine asks of this program when it makes a unit for an environment: the token its connector enrolls with, kept sealed in the system's store, the address and the authority it trusts, and the builds of the programs it runs.
//!
//! The engine decides when a unit is made and what it is made of; the certificate authority, the
//! address machines are told to dial and the master key that seals a secret are this program's. A
//! unit's connector enrolls with a token of its own purpose, single use, which becomes a managed
//! machine that belongs to the runtime it was made for ([`crate::certificates`]); the token is sealed
//! under the master key as soon as it is made and stored as a secret of the runtime, so that the
//! command that makes the unit refers to it and the link seals it again to the key of the host's node
//! when the command is written. The connector a unit runs is the one of the release the system runs
//! from ([`ReleaseBuilds`]): the file in the payload that the system also serves to anybody who asks.
//! The harnesses a unit is given are the builds the system lists for them: the default of each, for the
//! platform of the host, from the release's manifest and from what a system admin added.

use crate::certificates::{CertificateService, TokenFor, TokenRequest};
use crate::routes::admin::catalogue;
use crate::routes::downloads;
use crate::routes::resources::sealing::{binding, master_key};
use async_trait::async_trait;
use orch_api_contract::admin::{HarnessBuild, HarnessBuildStatus};
use orch_config::ServerConfig;
use orch_db::Conn;
use orch_db::queries::settings as stored;
use orch_engine::hosting::{
    Enrollment, EnrollmentAsk, HarnessRequest, HostServices, HostingError, Newest, ToolBuild,
};
use orch_types::Clock;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// How many bytes of a file are read at once when it is hashed: a connector is tens of megabytes, and this
/// keeps the work to a few hundred reads without holding the file.
const HASH_CHUNK_BYTES: usize = 64 * 1024;
/// How many hexadecimal digits of a build's hash are part of the version it is kept under in a host's tool
/// cache: enough that two builds of one release are two versions, so that a rebuilt file is not met by "another
/// build is held there under that name".
const VERSION_HASH_DIGITS: usize = 12;
/// The owner kind under which the secrets of an environment's unit are stored.
const OWNER_KIND: &str = "environment";
/// The name of the secret that holds a unit's enrollment token.
const TOKEN_SECRET: &str = "enroll_token";
/// The name of the connector's build in a host's tool cache; it comes first.
const CONNECTOR: &str = "orch-connector";

/// One build of a program a unit runs, with the platform it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// `linux`, `macos` or `windows`.
    pub os: String,
    /// `x86_64` or `aarch64`.
    pub arch: String,
    /// The program: its name, version, address and hash.
    pub tool: ToolBuild,
}

/// Where the builds of the programs a unit runs come from.
#[async_trait]
pub trait BuildCatalog: Send + Sync {
    /// The builds a unit made on a host of this platform runs, the connector first.
    async fn builds(&self, os: &str, arch: &str) -> Result<Vec<ToolBuild>, HostingError>;

    /// The default build of each of the harnesses `kinds` for a host of this platform, in the order of
    /// `kinds`: the builds the system lists, which a catalog that lists none gives none of.
    async fn harness_builds(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        kinds: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        let _ = (conn, os, arch, kinds);
        Ok(Vec::new())
    }

    /// Exact pins where listed; a catalogue with defaults alone refuses an unavailable pin.
    async fn requested_harness_builds(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        requests: &[HarnessRequest],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        let kinds: Vec<String> = requests
            .iter()
            .map(|request| request.kind.clone())
            .collect();
        let tools = self.harness_builds(conn, os, arch, &kinds).await?;
        for request in requests {
            if let Some(version) = &request.version
                && !tools
                    .iter()
                    .any(|tool| tool.name == request.kind && &tool.version == version)
            {
                return Err(HostingError::Failed(format!(
                    "no listed build of {} at {} for {os} {arch}",
                    request.kind, version
                )));
            }
        }
        Ok(tools)
    }
}

/// Builds listed in memory: what a program that knows them gives, and what the tests give.
#[derive(Debug, Clone, Default)]
pub struct ListedBuilds {
    builds: Vec<Build>,
}

impl ListedBuilds {
    /// A catalog of these builds.
    #[must_use]
    pub const fn new(builds: Vec<Build>) -> Self {
        Self { builds }
    }

    fn of(&self, os: &str, arch: &str) -> Result<Vec<ToolBuild>, HostingError> {
        let mut found: Vec<ToolBuild> = self
            .builds
            .iter()
            .filter(|build| build.os == os && build.arch == arch)
            .map(|build| build.tool.clone())
            .collect();
        found.sort_by_key(|tool| tool.name != CONNECTOR);
        if found.first().is_none_or(|tool| tool.name != CONNECTOR) {
            return Err(HostingError::Failed(format!(
                "no build of the connector is listed for {os} {arch}, so there is nothing to put in the unit"
            )));
        }
        Ok(found)
    }
}

#[async_trait]
impl BuildCatalog for ListedBuilds {
    async fn builds(&self, os: &str, arch: &str) -> Result<Vec<ToolBuild>, HostingError> {
        self.of(os, arch)
    }

    async fn requested_harness_builds(
        &self,
        _conn: &mut Conn,
        os: &str,
        arch: &str,
        requests: &[HarnessRequest],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        let listed = self.of(os, arch)?;
        let mut selected: std::collections::BTreeMap<String, ToolBuild> =
            std::collections::BTreeMap::new();
        for request in requests {
            let found = listed.iter().find(|tool| {
                tool.name == request.kind
                    && request
                        .version
                        .as_ref()
                        .is_none_or(|version| &tool.version == version)
            });
            let Some(tool) = found else {
                if let Some(version) = &request.version {
                    return Err(HostingError::Failed(format!(
                        "no listed build of {} at {} for {os} {arch}",
                        request.kind, version
                    )));
                }
                continue;
            };
            if selected
                .get(&tool.name)
                .is_some_and(|previous| previous.version != tool.version)
            {
                return Err(HostingError::Failed(format!(
                    "profiles request incompatible versions of {} in one unit",
                    tool.name
                )));
            }
            selected.insert(tool.name.clone(), tool.clone());
        }
        Ok(selected.into_values().collect())
    }
}

/// The builds of the release the system runs from: the connector in the `connectors` directory of its
/// payload, which is also the file the system serves under `/api/v1/downloads`.
///
/// Nothing is configured. A host's node downloads the very file anybody who asks the system for a
/// connector is given, from the address of its authenticated connection, and checks it against the SHA-256 worked out here from the bytes of the file as
/// they are when it is asked for, so that a file that was replaced is met by its new hash.
#[derive(Debug, Clone)]
pub struct ReleaseBuilds {
    payload: PathBuf,
}

impl ReleaseBuilds {
    /// The builds of the payload `config` names, reached at its public address.
    #[must_use]
    pub fn of(config: &ServerConfig) -> Self {
        Self::at(downloads::payload_dir(config), &config.system.public_url)
    }

    /// The builds in the payload directory `payload` (the one that holds the connectors), reached at
    /// the node's authenticated connection.
    #[must_use]
    pub fn at(payload: PathBuf, _public_url: &str) -> Self {
        Self { payload }
    }

    /// The release the payload says it is, if it says.
    async fn release(&self) -> Option<String> {
        let text = tokio::fs::read_to_string(self.payload.join(downloads::MANIFEST_FILE))
            .await
            .ok()?;
        let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
        manifest.get("version")?.as_str().map(str::to_owned)
    }
}

/// The SHA-256 of a file, in lower case hexadecimal, read in pieces.
async fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0_u8; HASH_CHUNK_BYTES];
    loop {
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&chunk[..read]);
    }
}

/// The word the API writes for a build's system or processor.
fn word_of(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The default build of each harness of `kinds` among `listed`, for a host of the platform `os` and
/// `arch`, as a host's tool cache is told of it: kept under the harness's name, at its version. A harness
/// of which none is listed for the platform is left out.
fn defaults_of(listed: &[HarnessBuild], os: &str, arch: &str, kinds: &[String]) -> Vec<ToolBuild> {
    kinds
        .iter()
        .filter_map(|kind| {
            let found = listed.iter().find(|build| {
                &build.kind == kind
                    && build.is_default
                    && word_of(&build.os) == os
                    && word_of(&build.arch) == arch
            });
            if found.is_none() {
                tracing::warn!(%kind, %os, %arch, "the system lists no default build of this harness for this platform: the unit will not have it");
            }
            found
        })
        .map(|build| ToolBuild {
            name: build.kind.clone(),
            version: build.version.clone(),
            url: build.url.clone(),
            sha256: build.sha256.clone(),
        })
        .collect()
}

/// Orders two versions by their numbers, part by part (`0.164.0` after `0.162.1`, `2.1.309` after `2.1.296`); a
/// part that is not a number counts as nothing. A release is after its own pre-releases (`0.2.1` after
/// `0.2.1-rc.2`), and pre-releases go in the order alpha, beta, rc by their numbers (`-rc.10` after `-rc.2`); what
/// follows `+` is build metadata and does not count.
pub(crate) fn version_order(one: &str, other: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn split(version: &str) -> (Vec<u64>, Option<Vec<u64>>) {
        let version = version.split('+').next().unwrap_or(version);
        let numbers = |text: &str| -> Vec<u64> {
            text.split('.')
                .map(|part| part.parse().unwrap_or(0))
                .collect()
        };
        match version.split_once('-') {
            None => (numbers(version), None),
            Some((release, marks)) => (
                numbers(release),
                Some(
                    marks
                        .split(['.', '-'])
                        .map(|part| match part {
                            "alpha" => 1,
                            "beta" => 2,
                            "rc" => 3,
                            other => other.parse().unwrap_or(0),
                        })
                        .collect(),
                ),
            ),
        }
    }
    let ((first, first_pre), (second, second_pre)) = (split(one), split(other));
    first
        .cmp(&second)
        .then_with(|| match (first_pre, second_pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(first), Some(second)) => first.cmp(&second),
        })
}

fn requested_of(
    listed: &[HarnessBuild],
    os: &str,
    arch: &str,
    requests: &[HarnessRequest],
) -> Result<Vec<ToolBuild>, HostingError> {
    let mut selected: std::collections::BTreeMap<String, ToolBuild> =
        std::collections::BTreeMap::new();
    for request in requests {
        if request
            .version
            .as_ref()
            .is_some_and(|version| version.trim().is_empty())
        {
            return Err(HostingError::Failed(format!(
                "an exact version is required for {}; choose the version before updating",
                request.kind
            )));
        }
        let platform = |build: &&HarnessBuild| {
            build.kind == request.kind && word_of(&build.os) == os && word_of(&build.arch) == arch
        };
        // The newest a place asks for, among the builds checked (or all of them); the default where none is.
        let newest = request.newest.and_then(|newest| {
            listed
                .iter()
                .filter(platform)
                .filter(|build| {
                    newest == Newest::Any || build.status == HarnessBuildStatus::Checked
                })
                .max_by(|one, other| version_order(&one.version, &other.version))
        });
        let found = newest.or_else(|| {
            listed.iter().filter(platform).find(|build| {
                request
                    .version
                    .as_ref()
                    .map_or(build.is_default, |version| {
                        orch_agent_programs::versions::same_release(&build.version, version)
                    })
            })
        });
        let Some(build) = found else {
            if let Some(version) = &request.version {
                return Err(HostingError::Failed(format!(
                    "no listed build of {} at {} for {os} {arch}",
                    request.kind, version
                )));
            }
            continue;
        };
        if let Some(reason) = build
            .check
            .as_ref()
            .and_then(|check| check.error.as_deref())
            .filter(|reason| reason.starts_with(crate::harness_checks::BUILD_REFUSAL_PREFIX))
        {
            return Err(HostingError::Failed(format!(
                "{} {}: {reason}",
                build.kind, build.version
            )));
        }
        if request.version.is_some() && build.status == HarnessBuildStatus::Checking {
            return Err(HostingError::Failed(format!(
                "{} {} is still being checked; wait for its result before updating",
                build.kind, build.version
            )));
        }
        if request.version.is_some()
            && build.source == orch_api_contract::admin::HarnessBuildSource::Channel
            && build.status == HarnessBuildStatus::Untested
            && build.check.is_none()
        {
            return Err(HostingError::Failed(format!(
                "{} {} has not been checked; fetch and check this version before updating",
                build.kind, build.version
            )));
        }
        let tool = ToolBuild {
            name: build.kind.clone(),
            version: build.version.clone(),
            url: build.url.clone(),
            sha256: build.sha256.clone(),
        };
        if selected
            .get(&tool.name)
            .is_some_and(|previous| previous.version != tool.version)
        {
            return Err(HostingError::Failed(format!(
                "profiles request incompatible versions of {} in one unit",
                tool.name
            )));
        }
        selected.insert(tool.name.clone(), tool);
    }
    Ok(selected.into_values().collect())
}

#[async_trait]
impl BuildCatalog for ReleaseBuilds {
    async fn requested_harness_builds(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        requests: &[HarnessRequest],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let listed = catalogue::builds_listed(&self.payload, conn)
            .await
            .map_err(|error| HostingError::Failed(error.to_string()))?;
        requested_of(&listed, os, arch, requests)
    }

    async fn harness_builds(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        kinds: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let listed = catalogue::builds_listed(&self.payload, conn)
            .await
            .map_err(|error| HostingError::Failed(error.to_string()))?;
        Ok(defaults_of(&listed, os, arch, kinds))
    }

    async fn builds(&self, os: &str, arch: &str) -> Result<Vec<ToolBuild>, HostingError> {
        if !self.payload.is_dir() {
            return Err(HostingError::Failed(
                "this system was not started from a release, so it has no connector to put in a unit"
                    .to_owned(),
            ));
        }
        let Some(file) = downloads::connector_names(os, arch)
            .into_iter()
            .map(|name| self.payload.join(name))
            .find(|path| path.is_file())
        else {
            return Err(HostingError::Failed(format!(
                "this release has no connector for {os} on {arch}, so there is nothing to put in the unit"
            )));
        };
        let sha256 = sha256_of(&file).await.map_err(|error| {
            HostingError::Failed(format!(
                "the connector {} cannot be read: {error}",
                file.display()
            ))
        })?;
        let release = self
            .release()
            .await
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        Ok(vec![ToolBuild {
            name: CONNECTOR.to_owned(),
            version: format!("{release}-{}", &sha256[..VERSION_HASH_DIGITS]),
            url: format!("/api/v1/downloads/connector/{os}/{arch}"),
            sha256,
        }])
    }
}

/// What the engine's jobs ask of this program.
pub struct ApiHostServices {
    certificates: CertificateService,
    clock: Arc<dyn Clock>,
    gateway_url: String,
    system: String,
    catalog: Arc<dyn BuildCatalog>,
    observed: std::sync::RwLock<std::collections::BTreeMap<ToolRequestKey, Vec<ToolBuild>>>,
}

type ToolRequestKey = (String, String, Vec<HarnessRequest>);

impl std::fmt::Debug for ApiHostServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiHostServices").finish_non_exhaustive()
    }
}

impl ApiHostServices {
    /// The services of a deployment with its configuration: the units it makes dial
    /// `[gateway] managed_endpoint`, trust its authority, and run the builds `catalog` lists.
    #[must_use]
    pub fn new(
        certificates: CertificateService,
        clock: Arc<dyn Clock>,
        config: &ServerConfig,
        catalog: Arc<dyn BuildCatalog>,
    ) -> Self {
        Self {
            certificates,
            clock,
            gateway_url: if config.gateway.managed_endpoint.is_empty() {
                config
                    .gateway
                    .endpoints
                    .first()
                    .map(|endpoint| endpoint.url.clone())
                    .unwrap_or_default()
            } else {
                config.gateway.managed_endpoint.clone()
            },
            system: config.system.name.clone(),
            catalog,
            observed: std::sync::RwLock::default(),
        }
    }
}

#[async_trait]
impl HostServices for ApiHostServices {
    fn observed_tools(
        &self,
        os: &str,
        arch: &str,
        requests: &[HarnessRequest],
    ) -> Option<Vec<ToolBuild>> {
        self.observed
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(os.to_owned(), arch.to_owned(), requests.to_vec()))
            .cloned()
    }
    async fn requested_tools(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        requests: &[HarnessRequest],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        let tools = self.catalog.builds(os, arch).await?;
        let harness = self
            .catalog
            .requested_harness_builds(conn, os, arch, requests)
            .await?;
        let tools: Vec<ToolBuild> = tools
            .into_iter()
            .filter(|tool| {
                tool.name == CONNECTOR || !requests.iter().any(|request| request.kind == tool.name)
            })
            .collect();
        let tools = joined(tools, harness);
        self.observed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (os.to_owned(), arch.to_owned(), requests.to_vec()),
                tools.clone(),
            );
        Ok(tools)
    }

    async fn enrollment(
        &self,
        conn: &mut Conn,
        ask: &EnrollmentAsk,
    ) -> Result<Enrollment, HostingError> {
        let gateway_url = if self.gateway_url.is_empty() {
            let node: Option<orch_types::Id> = sqlx::query_scalar(
                "SELECT h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.id=$1",
            )
            .bind(ask.runtime)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| HostingError::Failed(error.to_string()))?;
            let row = match node {
                Some(node) => stored::row(conn, None, &format!("machine_endpoints:{node}"))
                    .await
                    .map_err(|error| HostingError::Failed(error.to_string()))?,
                None => None,
            };
            row.and_then(|row| {
                serde_json::from_value::<Vec<orch_link_contract::enrollment::Endpoint>>(row.value)
                    .ok()
            })
            .and_then(|endpoints| endpoints.into_iter().next())
            .map(|endpoint| endpoint.url)
            .ok_or_else(|| {
                HostingError::Failed("the host has no authenticated connection address".into())
            })?
        } else {
            self.gateway_url.clone()
        };
        let issued = self
            .certificates
            .issue_token(&TokenRequest {
                created_by: ask.created_by,
                purpose: TokenFor::Runtime {
                    runtime: ask.runtime,
                },
                name_hint: Some(ask.name_hint.clone()),
            })
            .await
            .map_err(|fault| HostingError::Failed(fault.detail))?;
        let sealed = master_key()
            .map_err(|error| HostingError::Failed(error.to_string()))?
            .seal(
                issued.token.as_bytes(),
                &binding(OWNER_KIND, ask.runtime, TOKEN_SECRET),
            )
            .map_err(|_| {
                HostingError::Failed("the enrollment token could not be sealed".to_owned())
            })?;
        let secret = stored::put_secret(
            conn,
            OWNER_KIND,
            ask.runtime,
            TOKEN_SECRET,
            &sealed,
            None,
            self.clock.now(),
        )
        .await
        .map_err(|error| HostingError::Failed(error.to_string()))?;
        Ok(Enrollment {
            secret: secret.id,
            gateway_url,
            ca_cert: self.certificates.authority_certificate().pem(),
            system: self.system.clone(),
        })
    }

    async fn tools(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        harnesses: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError> {
        self.requested_tools(
            conn,
            os,
            arch,
            &harnesses
                .iter()
                .map(|kind| HarnessRequest {
                    kind: kind.clone(),
                    version: None,
                    newest: None,
                })
                .collect::<Vec<_>>(),
        )
        .await
    }
}

/// `tools` followed by the `harness` builds, a name kept once at its first place: the connector a unit
/// runs stays first and is never replaced by a harness build of the same name.
fn joined(mut tools: Vec<ToolBuild>, harness: Vec<ToolBuild>) -> Vec<ToolBuild> {
    for build in harness {
        if !tools.iter().any(|each| each.name == build.name) {
            tools.push(build);
        }
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_pin_selects_the_listed_nondefault_build_and_never_falls_back() {
        use orch_api_contract::admin::{HarnessArch, HarnessBuildSource};
        use orch_api_contract::resources::MachineOs;
        let listed = |version: &str, is_default| HarnessBuild {
            kind: "codex".to_owned(),
            version: version.to_owned(),
            os: MachineOs::Linux,
            arch: HarnessArch::X86_64,
            url: format!("https://builds.test/codex-{version}"),
            sha256: if is_default {
                "1".repeat(64)
            } else {
                "2".repeat(64)
            },
            is_default,
            source: HarnessBuildSource::Admin,
            status: orch_api_contract::admin::HarnessBuildStatus::Untested,
            check: None,
            carry: None,
        };
        let builds = vec![listed("0.160.1", true), listed("0.162.1", false)];
        let request = |version: Option<&str>| HarnessRequest {
            kind: "codex".to_owned(),
            version: version.map(str::to_owned),
            newest: None,
        };
        let pinned = requested_of(&builds, "linux", "x86_64", &[request(Some("0.162.1"))]).unwrap();
        assert!(
            requested_of(&builds, "linux", "x86_64", &[request(Some(""))]).is_err(),
            "an incomplete exact choice cannot select the default"
        );
        assert_eq!(pinned[0].version, "0.162.1");
        assert_eq!(pinned[0].sha256, "2".repeat(64));
        assert_eq!(
            requested_of(&builds, "linux", "x86_64", &[request(None)]).unwrap()[0].version,
            "0.160.1"
        );
        assert!(
            requested_of(&builds, "linux", "x86_64", &[request(Some("unlisted"))])
                .unwrap_err()
                .to_string()
                .contains("unlisted")
        );
        assert!(requested_of(&builds, "linux", "aarch64", &[request(Some("0.162.1"))]).is_err());
        assert!(
            requested_of(
                &builds,
                "linux",
                "x86_64",
                &[request(None), request(Some("0.162.1"))]
            )
            .is_err(),
            "one tool directory cannot silently satisfy conflicting profile versions"
        );
        let mut pending = listed("0.170.0", false);
        pending.source = orch_api_contract::admin::HarnessBuildSource::Channel;
        assert!(
            requested_of(
                std::slice::from_ref(&pending),
                "linux",
                "x86_64",
                &[request(Some("0.170.0"))]
            )
            .is_err(),
            "an exact vendor request waits for its first checksum and compatibility check"
        );
        pending.status = HarnessBuildStatus::Checking;
        assert!(
            requested_of(
                std::slice::from_ref(&pending),
                "linux",
                "x86_64",
                &[request(Some("0.170.0"))]
            )
            .is_err()
        );
        pending.status = HarnessBuildStatus::Failed;
        assert!(
            requested_of(
                std::slice::from_ref(&pending),
                "linux",
                "x86_64",
                &[request(Some("0.170.0"))]
            )
            .is_ok(),
            "a completed failed target remains an operator choice"
        );
        pending.is_default = true;
        pending.check = Some(
            serde_json::from_value(serde_json::json!({
                "at":"2026-10-26T00:00:00Z","machine":null,"missing":[],"off":[],
                "error":"refused: checksum mismatch","stage":"complete","cases":[]
            }))
            .unwrap(),
        );
        for asked in [
            request(Some("0.170.0")),
            request(None),
            HarnessRequest {
                kind: "codex".to_owned(),
                version: None,
                newest: Some(Newest::Any),
            },
        ] {
            let error = requested_of(std::slice::from_ref(&pending), "linux", "x86_64", &[asked])
                .unwrap_err();
            assert!(error.to_string().contains("checksum"), "{error}");
        }
    }

    #[test]
    fn a_place_asking_for_the_newest_gets_the_newest_checked_or_any_and_never_a_pre_release_order_by_text()
     {
        use orch_api_contract::admin::{HarnessArch, HarnessBuildSource, HarnessBuildStatus};
        use orch_api_contract::resources::MachineOs;
        let listed = |version: &str, is_default, status| HarnessBuild {
            kind: "codex".to_owned(),
            version: version.to_owned(),
            os: MachineOs::Linux,
            arch: HarnessArch::X86_64,
            url: format!("https://builds.test/codex-{version}"),
            sha256: "3".repeat(64),
            is_default,
            source: if is_default {
                HarnessBuildSource::Release
            } else {
                HarnessBuildSource::Channel
            },
            status,
            check: None,
            carry: None,
        };
        let builds = vec![
            listed("0.162.1", true, HarnessBuildStatus::Checked),
            listed("0.163.0", false, HarnessBuildStatus::Checked),
            listed("0.164.0", false, HarnessBuildStatus::Untested),
            listed("0.99.0", false, HarnessBuildStatus::Checked),
        ];
        let request = |newest| HarnessRequest {
            kind: "codex".to_owned(),
            version: None,
            newest,
        };
        let pick = |newest| {
            requested_of(&builds, "linux", "x86_64", &[request(newest)]).unwrap()[0]
                .version
                .clone()
        };
        assert_eq!(pick(None), "0.162.1", "shipped is the default");
        assert_eq!(
            pick(Some(Newest::Checked)),
            "0.163.0",
            "0.99.0 is older though its text sorts after"
        );
        assert_eq!(pick(Some(Newest::Any)), "0.164.0");
        assert_eq!(
            version_order("2.1.309", "2.1.296"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn a_release_is_after_its_pre_releases_and_pre_releases_go_by_kind_and_number() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(version_order("0.2.1", "0.2.1-rc.2"), Greater);
        assert_eq!(version_order("0.2.1-rc.2", "0.2.1"), Less);
        assert_eq!(version_order("0.2.1-rc.10", "0.2.1-rc.2"), Greater);
        assert_eq!(version_order("0.2.1-beta.9", "0.2.1-rc.1"), Less);
        assert_eq!(version_order("0.2.1-alpha.1", "0.2.1-beta.1"), Less);
        assert_eq!(version_order("0.2.2-rc.1", "0.2.1"), Greater);
        assert_eq!(version_order("1.0.0+post.1", "1.0.0"), Equal);
        assert_eq!(version_order("0.2.1-rc.2", "0.2.1-rc.2"), Equal);
    }

    fn build(os: &str, arch: &str, name: &str) -> Build {
        Build {
            os: os.to_owned(),
            arch: arch.to_owned(),
            tool: ToolBuild {
                name: name.to_owned(),
                version: "0.1.0".to_owned(),
                url: format!("https://builds.test/{name}"),
                sha256: "0".repeat(64),
            },
        }
    }

    #[test]
    fn the_builds_for_a_platform_are_those_listed_for_it_with_the_connector_first() {
        let listed = ListedBuilds::new(vec![
            build("linux", "x86_64", "codex"),
            build("linux", "aarch64", "orch-connector"),
            build("linux", "x86_64", "orch-connector"),
            build("macos", "aarch64", "orch-connector"),
        ]);
        let names: Vec<String> = listed
            .of("linux", "x86_64")
            .unwrap()
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["orch-connector", "codex"]);
    }

    #[test]
    fn the_builds_of_a_unit_are_the_connector_then_the_harnesses_each_name_once() {
        let named = |name: &str, version: &str| ToolBuild {
            version: version.to_owned(),
            ..build("linux", "x86_64", name).tool
        };
        let put = joined(
            vec![named("orch-connector", "1.2.3")],
            vec![
                named("codex", "0.162.1"),
                named("orch-connector", "9.9.9"),
                named("claude-code", "2.1.296"),
                named("codex", "0.1.0"),
            ],
        );
        let got: Vec<(&str, &str)> = put
            .iter()
            .map(|tool| (tool.name.as_str(), tool.version.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("orch-connector", "1.2.3"),
                ("codex", "0.162.1"),
                ("claude-code", "2.1.296"),
            ],
            "the connector stays first and is not replaced, and a name is put in the cache once"
        );
    }

    /// A payload as a release lays it out: the connector for Linux on x86_64 beside the manifest.
    fn a_release(connector: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orch-connector-linux-x86_64-musl"),
            connector,
        )
        .unwrap();
        std::fs::write(dir.path().join("version.json"), r#"{"version":"1.2.3"}"#).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_unit_runs_the_connector_of_the_release_at_the_address_anybody_downloads_it_from() {
        let dir = a_release(b"abc");
        let catalog = ReleaseBuilds::at(dir.path().to_owned(), "https://orch.example.com/");
        assert_eq!(
            catalog.builds("linux", "x86_64").await.unwrap(),
            [ToolBuild {
                name: "orch-connector".to_owned(),
                version: "1.2.3-ba7816bf8f01".to_owned(),
                url: "/api/v1/downloads/connector/linux/x86_64".to_owned(),
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                    .to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn a_connector_that_was_replaced_is_another_version_with_a_hash_of_its_own() {
        let dir = a_release(b"abc");
        let catalog = ReleaseBuilds::at(dir.path().to_owned(), "https://orch.example.com");
        let before = catalog.builds("linux", "x86_64").await.unwrap();
        std::fs::write(dir.path().join("orch-connector-linux-x86_64-musl"), b"abcd").unwrap();
        let after = catalog.builds("linux", "x86_64").await.unwrap();
        assert_ne!(before[0].sha256, after[0].sha256);
        assert_ne!(
            before[0].version, after[0].version,
            "a host keeps a version of a tool as it first got it"
        );
    }

    #[tokio::test]
    async fn a_system_that_was_not_started_from_a_release_has_no_connector_to_put_in_a_unit() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = ReleaseBuilds::at(dir.path().join("connectors"), "https://orch.example.com");
        let refused = catalog.builds("linux", "x86_64").await.unwrap_err();
        assert!(
            refused.to_string().contains("not started from a release"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_release_without_a_connector_for_the_platform_of_the_host_has_nothing_to_put_in_a_unit()
     {
        let dir = a_release(b"abc");
        let catalog = ReleaseBuilds::at(dir.path().to_owned(), "https://orch.example.com");
        let refused = catalog.builds("linux", "aarch64").await.unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("no connector for linux on aarch64"),
            "{refused}"
        );
    }

    #[test]
    fn a_platform_without_a_connector_build_has_nothing_to_put_in_a_unit() {
        let listed = ListedBuilds::new(vec![build("linux", "x86_64", "codex")]);
        let refused = listed.of("linux", "x86_64").unwrap_err();
        assert!(
            refused.to_string().contains("no build of the connector"),
            "{refused}"
        );
        assert!(ListedBuilds::default().of("linux", "x86_64").is_err());
    }
}
