//! What making a unit asks of the rest of the system: the token its connector enrolls with, the address and the authority it trusts, and where the programs it runs come from.
//!
//! The engine decides when a unit is made and what it is made of; it does not hold the system's
//! certificate authority, the address machines are told to dial, or the master key that seals a
//! token into a secret. Those belong to the program the engine lives in, which answers through
//! [`HostServices`] and is attached to the engine with [`crate::Engine::attach_host_services`].
//! Until it is, a job that needs it fails with a plain sentence and nothing is made.

use async_trait::async_trait;
use orch_config::ServerConfig;
use orch_db::Conn;
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long a runner's lease on a job holds before another process may take the job over: long enough
/// that a slow transaction does not lose it, short enough that a job whose process died is going
/// again within half a minute.
pub const JOB_LEASE: Duration = Duration::from_secs(30);
/// How long a stop request waits for the host to stop a unit before it answers with what it has:
/// the grace the node gives a unit (ten seconds) and the time a slow host needs to answer.
pub const STOP_WAIT: Duration = Duration::from_secs(45);
/// How often a runner looks at its job's row for a request to cancel, and a waiting phase looks at
/// what it waits for: often enough that a cancel is felt at once, rarely enough to cost nothing.
pub const WATCH_EVERY: Duration = Duration::from_millis(250);
/// The shortest pause between two attempts to remove what will not go: the pauses grow from here,
/// doubling, up to `reaper_retry_max_secs`.
pub const REMOVAL_FIRST_PAUSE: Duration = Duration::from_secs(30);
/// How long the engine waits for a connector to answer one command of an attach (`env.attach` is
/// given a minute, `exec.start` two): the longest of them and a margin for the answer to come back.
pub const ATTACH_WAIT: Duration = Duration::from_secs(150);
/// How long an environment on a person's machine whose attach was refused is left alone before the
/// engine tries it again: long enough not to press a machine whose policy says no, short enough that a
/// person who made the directory or changed the policy sees the environment come up within half a minute.
pub const ATTACH_RETRY: Duration = Duration::from_secs(30);

/// The waits of the part that makes, stops and removes environments, from the deployment's
/// configuration where it has a value for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostingConfig {
    /// How long a runner's lease on a job holds.
    pub job_lease: Duration,
    /// How long the connector of a new unit has to enroll (`[node] enroll_timeout_secs`).
    pub enroll_timeout: Duration,
    /// How often the schedule looks for what is due for removal (`[engine] reaper_secs`).
    pub reaper_every: Duration,
    /// The pause after the first removal that failed, which doubles with each further failure.
    pub removal_first_pause: Duration,
    /// The longest pause between two attempts to remove something (`[engine] reaper_retry_max_secs`).
    pub reaper_retry_max: Duration,
    /// How long before an unused environment is removed its owner is told (`[engine] removal_notice_hours`).
    pub removal_notice: Duration,
    /// How long a request to stop waits for the host.
    pub stop_wait: Duration,
    /// How long the engine waits for a connector to answer one command of an attach.
    pub attach_wait: Duration,
    /// How long an environment whose attach was refused is left alone before it is tried again.
    pub attach_retry: Duration,
    // A managed provider's runtime and the sign-in that waits for it (the part that keeps providers).
    /// How long a sign-in may wait for the runtime that is being made before it is forgotten with the
    /// secrets in it (`[engine] login_pending_secs`).
    pub login_pending: Duration,
}

impl Default for HostingConfig {
    fn default() -> Self {
        Self::from_server(&ServerConfig::default())
    }
}

impl HostingConfig {
    /// The waits a deployment's configuration gives.
    #[must_use]
    pub fn from_server(config: &ServerConfig) -> Self {
        Self {
            job_lease: JOB_LEASE,
            enroll_timeout: Duration::from_secs(config.node.enroll_timeout_secs),
            reaper_every: Duration::from_secs(config.engine.reaper_secs),
            removal_first_pause: REMOVAL_FIRST_PAUSE,
            reaper_retry_max: Duration::from_secs(config.engine.reaper_retry_max_secs),
            removal_notice: Duration::from_secs(
                config
                    .engine
                    .removal_notice_hours
                    .saturating_mul(SECONDS_PER_HOUR),
            ),
            stop_wait: STOP_WAIT,
            attach_wait: ATTACH_WAIT,
            attach_retry: ATTACH_RETRY,
            login_pending: Duration::from_secs(config.engine.login_pending_secs),
        }
    }
}

/// Seconds in an hour, for the notice before an unused environment goes.
const SECONDS_PER_HOUR: u64 = 3_600;

/// Why the rest of the system could not give what a unit needs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostingError {
    /// Nothing is attached that could answer.
    #[error("the system cannot make an enrollment token for a unit: nothing is attached to do it")]
    NotAttached,
    /// The answer could not be made.
    #[error("{0}")]
    Failed(String),
}

/// What the connector of a new unit enrolls with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentAsk {
    /// The runtime the unit is for; the machine that enrolls becomes its machine.
    pub runtime: Id,
    /// The host the unit is made on.
    pub host: Id,
    /// Who the machine is made on behalf of: the environment's owner.
    pub created_by: Id,
    /// What the token is called, by which it is found again.
    pub name_hint: String,
}

/// What a unit needs to come online by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrollment {
    /// The secret that holds the token, sealed; the command that makes the unit refers to it and
    /// the link seals it to the host's key when the command is written.
    pub secret: Id,
    /// Where the connector in the unit dials.
    pub gateway_url: String,
    /// The authority it trusts, as PEM.
    pub ca_cert: String,
    /// What the system is called, which every unit it makes carries as a label.
    pub system: String,
}

/// A program a host must hold before a unit of the system runs there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBuild {
    /// Its name in the host's tool cache.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Where the host downloads it from.
    pub url: String,
    /// The SHA-256 the download must have, in hexadecimal.
    pub sha256: String,
}

/// The exact listed build a profile needs; no version means the platform's system default.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HarnessRequest {
    /// The harness kind.
    pub kind: String,
    /// The profile's pinned version, or the exact version the place chose.
    pub version: Option<String>,
    /// Where no version is pinned: the newest build of the kind instead of its default, as the place chose.
    pub newest: Option<Newest>,
}

/// Which newest build a place chose for a harness kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Newest {
    /// The newest build whose check passed (the build a release ships with counts as checked).
    Checked,
    /// The newest build, checked or not.
    Any,
}

/// Applies a place's `agents.harness_versions` to the requests that pin no version: shipped leaves the default,
/// exact names a version, and newest asks for the newest build (checked, or any). A profile's pin always wins.
#[must_use]
pub fn with_place_versions(
    requests: std::collections::BTreeSet<HarnessRequest>,
    wanted: Option<&orch_types::Json>,
) -> std::collections::BTreeSet<HarnessRequest> {
    requests
        .into_iter()
        .map(|mut request| {
            let choice = wanted.and_then(|wanted| wanted.get(&request.kind));
            if request.version.is_none()
                && request.newest.is_none()
                && let Some(choice) = choice
            {
                match choice.get("use").and_then(orch_types::Json::as_str) {
                    Some("exact") => {
                        // Retain an unsatisfied exact request even for old malformed rows; never default it.
                        request.version = Some(
                            choice
                                .get("version")
                                .and_then(orch_types::Json::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        );
                    }
                    Some("newest_checked") => request.newest = Some(Newest::Checked),
                    Some("newest") => request.newest = Some(Newest::Any),
                    _ => {}
                }
            }
            request
        })
        .collect()
}

/// Profile requirements of a unit, without replacing an unavailable pin with a default.
pub async fn harness_requests(
    conn: &mut Conn,
    target: crate::updates::Target,
    wanted: &[String],
) -> Result<Vec<HarnessRequest>, crate::EngineError> {
    let (environment, provider) = match target {
        crate::updates::Target::Environment(id) => (Some(id), None),
        crate::updates::Target::Provider(id) => (None, Some(id)),
    };
    let default_agent = match environment {
        Some(id) => {
            orch_settings::get::<Id>(
                conn,
                orch_settings::Place::Environment(id),
                orch_settings_contract::registry::keys::AGENT_DEFAULT_AGENT_ID,
            )
            .await?
        }
        None => None,
    };
    let profiled: Vec<(String, Option<String>)> = sqlx::query_as("SELECT DISTINCT a.harness_kind,a.profile->>'harness_version'
        FROM agents a WHERE a.deleted_at IS NULL AND NOT a.disabled AND a.harness_kind=ANY($3)
          AND (a.id=$4 OR EXISTS(SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id
                WHERE t.agent_id=a.id AND (t.environment_id=$1 OR t.provider_id=$2) AND n.deleted_at IS NULL)
               OR EXISTS(SELECT 1 FROM providers p WHERE p.id=$2 AND p.agent_id=a.id)
               OR (a.home_id IS NULL AND EXISTS(SELECT 1 FROM providers p WHERE p.id=$2 AND p.kind='api')))")
        .bind(environment).bind(provider).bind(wanted).bind(default_agent).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let mut requests = std::collections::BTreeSet::new();
    for kind in wanted {
        let actual: Vec<_> = profiled.iter().filter(|(name, _)| name == kind).collect();
        if actual.is_empty() {
            requests.insert(HarnessRequest {
                kind: kind.clone(),
                version: None,
                newest: None,
            });
        } else {
            for (kind, version) in actual {
                requests.insert(HarnessRequest {
                    kind: kind.clone(),
                    version: version.clone(),
                    newest: None,
                });
            }
        }
    }
    let place = match target {
        crate::updates::Target::Environment(id) => orch_settings::Place::Environment(id),
        crate::updates::Target::Provider(id) => orch_settings::Place::Provider(id),
    };
    let wanted = Box::pin(orch_settings::get::<orch_types::Json>(
        conn,
        place,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_VERSIONS,
    ))
    .await?;
    Ok(with_place_versions(requests, wanted.as_ref())
        .into_iter()
        .collect())
}

/// What the program the engine lives in answers.
#[async_trait]
pub trait HostServices: Send + Sync {
    /// An already observed catalogue, for read projections. This must never fetch, hash a file,
    /// contact a machine or wait for a catalogue refresh. None means it has not been observed.
    fn observed_tools(
        &self,
        _os: &str,
        _arch: &str,
        _requests: &[HarnessRequest],
    ) -> Option<Vec<ToolBuild>> {
        None
    }
    /// Makes the single-use token with which the connector of a unit enrolls, and stores it sealed as
    /// a secret of the runtime. The token makes a machine that is managed, on the runtime's host, and
    /// that the runtime knows once it has enrolled.
    async fn enrollment(
        &self,
        conn: &mut Conn,
        ask: &EnrollmentAsk,
    ) -> Result<Enrollment, HostingError>;

    /// The programs a unit made on a host of this platform needs, in the order they are put in the
    /// cache: first the connector, which the unit runs, then the default build of each of the
    /// `harnesses` (kinds of harness) that the system lists for the platform. A harness it lists no
    /// build of for this platform is left out, and the unit has none of it.
    async fn tools(
        &self,
        conn: &mut Conn,
        os: &str,
        arch: &str,
        harnesses: &[String],
    ) -> Result<Vec<ToolBuild>, HostingError>;

    /// Resolves profiles' requested builds. Implementations that expose only defaults refuse a different pin.
    async fn requested_tools(
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
        let tools = self.tools(conn, os, arch, &kinds).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn request(kind: &str, version: Option<&str>) -> HarnessRequest {
        HarnessRequest {
            kind: kind.to_owned(),
            version: version.map(str::to_owned),
            newest: None,
        }
    }

    #[test]
    fn a_place_chooses_the_build_of_each_kind_and_a_profiles_pin_still_wins() {
        let wanted = json!({
            "codex": { "use": "newest_checked" },
            "claude-code": { "use": "exact", "version": "2.1.309" },
            "pi": { "use": "newest" },
        });
        let requests: BTreeSet<_> = [
            request("codex", None),
            request("claude-code", None),
            request("pi", None),
            request("fake", None),
            request("codex", Some("0.162.1")),
        ]
        .into_iter()
        .collect();
        let applied: Vec<_> = with_place_versions(requests, Some(&wanted))
            .into_iter()
            .collect();
        let of = |kind: &str, pinned: Option<&str>| {
            applied
                .iter()
                .find(|request| {
                    request.kind == kind
                        && (pinned.is_none() || request.version.as_deref() == pinned)
                })
                .cloned()
                .unwrap()
        };
        assert_eq!(
            of("codex", Some("0.162.1")).newest,
            None,
            "a profile's pin wins"
        );
        assert!(
            applied
                .iter()
                .any(|request| request.kind == "codex" && request.newest == Some(Newest::Checked))
        );
        assert_eq!(of("claude-code", None).version.as_deref(), Some("2.1.309"));
        assert_eq!(of("pi", None).newest, Some(Newest::Any));
        assert_eq!(
            of("fake", None),
            request("fake", None),
            "a kind the place says nothing of is shipped"
        );
        let none: BTreeSet<_> = [request("codex", None)].into_iter().collect();
        assert_eq!(with_place_versions(none.clone(), None), none);
        let incomplete = with_place_versions(none, Some(&json!({"codex":{"use":"exact"}})));
        assert!(
            incomplete.iter().all(|request| request.version.is_some()),
            "an incomplete stored Exact policy remains unsatisfied rather than becoming Shipped"
        );
    }
}

/// A harness's descriptor constrained by every kept build's evidence at that kind and version, across systems
/// and processors. Named failed capabilities stay off. Codex standalone native import is enabled only after all
/// those builds prove that case in a complete native report; an unproved or legacy declaration cannot offer it.
///
/// # Errors
/// The database failed.
pub async fn with_build_limits(
    conn: &mut Conn,
    descriptor: orch_types::Json,
) -> Result<orch_types::Json, crate::EngineError> {
    Ok(build_limits(conn, descriptor).await?.0)
}

/// Applies the build's current limits and returns their names for a plan's point-of-use facts.
///
/// # Errors
/// The database failed.
pub async fn build_limits(
    conn: &mut Conn,
    mut descriptor: orch_types::Json,
) -> Result<(orch_types::Json, Vec<String>), crate::EngineError> {
    let (Some(kind), Some(version)) = (
        descriptor
            .get("kind")
            .and_then(orch_types::Json::as_str)
            .map(str::to_owned),
        descriptor
            .pointer("/harness/version")
            .and_then(orch_types::Json::as_str)
            .map(str::to_owned),
    ) else {
        return Ok((descriptor, Vec::new()));
    };
    let kept: Vec<(String, orch_types::Json)> =
        orch_db::queries::resources::management::stored_harness_builds(conn)
            .await?
            .into_iter()
            .filter(|(_, build)| {
                build.get("kind").and_then(orch_types::Json::as_str) == Some(kind.as_str())
                    && build.get("version").and_then(orch_types::Json::as_str)
                        == Some(version.as_str())
            })
            .collect();
    // A feature is proven for a build when a case of its check that stands for it passed.
    let proven: Vec<String> = kept
        .iter()
        .flat_map(|(_, build)| {
            build
                .pointer("/check/cases")
                .and_then(orch_types::Json::as_array)
                .into_iter()
                .flatten()
                .filter(|case| case.get("passed").and_then(orch_types::Json::as_bool) == Some(true))
                .filter_map(|case| case.get("feature").and_then(orch_types::Json::as_str))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    let programs = orch_db::programs::registry(conn).await?;
    let expected: Vec<String> = programs
        .program(&kind)
        .map(|manifest| {
            manifest
                .checks
                .cases()
                .map(|(name, _)| name.to_owned())
                .collect()
        })
        .unwrap_or_default();
    if expected.iter().any(|name| name == "history.native_import")
        && let Some(held) = descriptor.pointer_mut("/capabilities/import_native")
    {
        *held = orch_types::Json::Bool(
            !kept.is_empty()
                && kept
                    .iter()
                    .all(|(_, build)| proves_native_import(build, &expected)),
        );
    }
    let off: Vec<String> = kept
        .into_iter()
        .flat_map(|(_, build)| {
            build
                .pointer("/check/off")
                .and_then(orch_types::Json::as_array)
                .map(|off| {
                    off.iter()
                        .filter_map(|capability| capability.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();
    switch_off(&mut descriptor, &off);
    keep_auto_only_where_proven(&mut descriptor, &proven);
    Ok((descriptor, off))
}

fn proves_native_import(build: &orch_types::Json, expected: &[String]) -> bool {
    matches!(build["status"].as_str(), Some("checked" | "failed"))
        && build
            .pointer("/check/stage")
            .and_then(orch_types::Json::as_str)
            == Some("complete")
        && build
            .pointer("/check/cases")
            .and_then(orch_types::Json::as_array)
            .is_some_and(|cases| {
                cases.len() == expected.len()
                    && cases.iter().all(|case| {
                        case["passed"]
                            .as_bool()
                            .is_some_and(|passed| passed == case["error"].is_null())
                            && case["elapsed_ms"]
                                .as_i64()
                                .is_some_and(|elapsed| elapsed >= 0)
                    })
                    && expected.iter().all(|name| {
                        cases
                            .iter()
                            .filter(|case| case["name"] == name.as_str())
                            .count()
                            == 1
                    })
                    && cases.iter().any(|case| {
                        case["name"] == "history.native_import"
                            && case["passed"] == true
                            && case["unsupported"].is_null()
                    })
            })
}

/// The feature whose passing case proves Auto's reviewer for a build.
const AUTO_FEATURE: &str = "approval_modes.auto";

/// Auto is a mode of a program only for a build whose check passed the case that proves it: the adapter's claim
/// alone does not make it one, and no override can.
fn keep_auto_only_where_proven(descriptor: &mut orch_types::Json, proven: &[String]) {
    if proven.iter().any(|feature| feature == AUTO_FEATURE) {
        return;
    }
    if let Some(modes) = descriptor
        .get_mut("approval_modes")
        .and_then(orch_types::Json::as_array_mut)
    {
        modes.retain(|mode| mode.as_str() != Some("auto"));
    }
}

/// Switches off the named capabilities of a descriptor that are switches; the others stay as declared.
fn switch_off(descriptor: &mut orch_types::Json, off: &[String]) {
    // A mode a failed case stood for is no mode of the build.
    for capability in off {
        if let Some(mode) = capability.strip_prefix("approval_modes.")
            && let Some(modes) = descriptor
                .get_mut("approval_modes")
                .and_then(orch_types::Json::as_array_mut)
        {
            modes.retain(|each| each.as_str() != Some(mode));
        }
    }
    if let Some(capabilities) = descriptor
        .get_mut("capabilities")
        .and_then(orch_types::Json::as_object_mut)
    {
        for capability in off {
            if capability.starts_with("approval_modes.") {
                continue;
            }
            if let Some(kind) = capability.strip_prefix("approvals.") {
                if let Some(approvals) = capabilities
                    .get_mut("approvals")
                    .and_then(orch_types::Json::as_array_mut)
                {
                    approvals.retain(|approval| approval.as_str() != Some(kind));
                }
                continue;
            }
            if capability == "orch_tools" {
                capabilities.insert(
                    capability.clone(),
                    orch_types::Json::String("none".to_owned()),
                );
                continue;
            }
            if let Some(held) = capabilities.get_mut(capability)
                && held.is_boolean()
            {
                *held = orch_types::Json::Bool(false);
            }
            if capability == "subagents"
                && let Some(limits) = capabilities.get_mut("subagent_limits")
            {
                *limits = orch_types::Json::Array(Vec::new());
            }
        }
    }
}

#[cfg(test)]
mod build_limit_tests {
    #[test]
    fn an_unavailable_child_stream_removes_child_controls_and_keeps_other_features() {
        let mut descriptor = serde_json::json!({"capabilities":{"subagents":true,"subagent_limits":["at_once","depth"],"steer":true,"approvals":["command"]}});
        super::switch_off(&mut descriptor, &["subagents".to_owned()]);
        assert_eq!(descriptor["capabilities"]["subagents"], false);
        assert_eq!(
            descriptor["capabilities"]["subagent_limits"],
            serde_json::json!([])
        );
        assert_eq!(descriptor["capabilities"]["steer"], true);
        assert_eq!(
            descriptor["capabilities"]["approvals"],
            serde_json::json!(["command"])
        );
    }
    #[test]
    fn a_failed_native_question_keeps_commands_and_every_unrelated_capability_available() {
        let mut descriptor = serde_json::json!({ "capabilities": { "steer": true, "subagents": true, "orch_tools": "mcp", "approvals": ["command", "user_input", "mcp_elicitation"] } });
        super::switch_off(&mut descriptor, &["approvals.user_input".to_owned()]);
        assert_eq!(
            descriptor["capabilities"]["approvals"],
            serde_json::json!(["command", "mcp_elicitation"])
        );
        assert_eq!(descriptor["capabilities"]["steer"], true);
        assert_eq!(descriptor["capabilities"]["subagents"], true);
        assert_eq!(descriptor["capabilities"]["orch_tools"], "mcp");
        super::switch_off(&mut descriptor, &["orch_tools".to_owned()]);
        assert_eq!(descriptor["capabilities"]["orch_tools"], "none");
        assert_eq!(descriptor["capabilities"]["steer"], true);
    }

    #[test]
    fn only_boolean_capabilities_the_check_named_are_switched_off() {
        let mut descriptor = serde_json::json!({
            "kind": "codex", "harness": { "version": "0.170.0" },
            "capabilities": { "plan_mode": true, "steer": true, "approvals": ["command"] }
        });
        let off = ["plan_mode", "approvals", "absent"].map(str::to_owned);
        super::switch_off(&mut descriptor, &off);
        assert_eq!(descriptor["capabilities"]["plan_mode"], false);
        assert_eq!(descriptor["capabilities"]["steer"], true);
        assert_eq!(
            descriptor["capabilities"]["approvals"],
            serde_json::json!(["command"])
        );
    }

    #[test]
    fn auto_is_a_mode_of_a_build_only_where_a_passed_case_proves_it_and_no_claim_alone_makes_it_one()
     {
        let claimed =
            serde_json::json!({ "approval_modes": ["ask", "on_request", "auto", "bypass"] });
        let mut unproven = claimed.clone();
        super::keep_auto_only_where_proven(&mut unproven, &["steer".to_owned()]);
        assert_eq!(
            unproven["approval_modes"],
            serde_json::json!(["ask", "on_request", "bypass"]),
            "a build that passed nothing for it has no Auto"
        );
        let mut proven = claimed.clone();
        super::keep_auto_only_where_proven(&mut proven, &["approval_modes.auto".to_owned()]);
        assert_eq!(proven["approval_modes"], claimed["approval_modes"]);
        let mut none = serde_json::json!({ "capabilities": {} });
        super::keep_auto_only_where_proven(&mut none, &[]);
        assert_eq!(
            none,
            serde_json::json!({ "capabilities": {} }),
            "a descriptor with no modes is left as it is"
        );
    }

    #[test]
    fn a_mode_whose_case_failed_is_no_mode_of_the_build_and_other_modes_stay() {
        let mut descriptor = serde_json::json!({
            "approval_modes": ["ask", "auto", "bypass"],
            "capabilities": { "steer": true }
        });
        super::switch_off(&mut descriptor, &["approval_modes.auto".to_owned()]);
        assert_eq!(
            descriptor["approval_modes"],
            serde_json::json!(["ask", "bypass"])
        );
        assert_eq!(
            descriptor["capabilities"],
            serde_json::json!({ "steer": true }),
            "the feature name is not taken for a capability"
        );
    }
}
