//! The data a system admin keeps for the whole system: the definitions of capabilities, the builds of harnesses that hosts may fetch, and the agent profiles that have no home.
//!
//! A capability definition is a row like any other: the ones that ship are edited, more are added,
//! and a custom one is removed once no environment holds it. The builds of a harness come from two
//! places that are told apart by `source`: the list the release wrote, which is read from the
//! manifest in its payload when the release has one, and the builds an admin added, which win where
//! both name the same build. A profile of the system belongs to no org; its page starts with nobody
//! on it, and system admins hold every permission on it.

use super::{admin_of, invalid, is_address, record};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::downloads::payload_dir;
use crate::routes::resources::view::{Holding, accesses, addresses, parse};
use crate::routes::resources::{Home, clean_name, record as record_resource};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_agent_contract::facts::AgentProfile;
use orch_api_contract::admin::{
    AdminAgentCreate, AdminCapabilityDelete, AdminCapabilityList, AdminCapabilitySet,
    AdminHarnessChannelsGet, AdminHarnessChannelsRead, AdminHarnessCheck,
    AdminHarnessChecksumsImport, AdminHarnessList, AdminHarnessSet, AdminHarnessUsageList,
    EnvironmentHarnessBuildList, HarnessBuild, HarnessBuildList, HarnessBuildSource,
    HarnessBuildStatus, HarnessBuildUsage, HarnessChecksumsImportBody, HarnessChecksumsImportReply,
    HarnessVersionFetch, HarnessVersionFetchBody, RunningHarnessBuild,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{
    Agent, AgentRef, CapabilityDef, CapabilityDefaultDecision, OrgAgentCreateBody,
};
use orch_authz::{Policy, Principal, resources};
use orch_db::queries::resources::management::{
    self, CapabilityPut, CapabilityRow, CapabilityWrite,
};
use orch_db::queries::resources::routes as agent_rows;
use orch_db::{Conn, DbError};
use orch_types::{Json, Time};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

/// The fewest and the most characters of the key of a capability: what the schema allows.
const CAPABILITY_KEY_CHARS: std::ops::RangeInclusive<usize> = 2..=32;
/// The most characters of the label of a capability.
const LABEL_MAX_CHARS: usize = 80;
/// The most characters of the description of a capability.
const DESCRIPTION_MAX_CHARS: usize = 1_000;
/// The most characters of the name of a driver, a harness kind or a version.
const WORD_MAX_CHARS: usize = 64;
/// How many characters a SHA-256 is in hexadecimal.
const SHA256_HEX_CHARS: usize = 64;
/// The members an agent profile may have: which build to run, how the harness is reached, what is
/// merged over the adapter's configuration and the variables it finds.
const PROFILE_MEMBERS: [&str; 4] = ["harness_version", "attach", "config", "env"];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminCapabilityList, capability_list)
        .on(AdminCapabilitySet, capability_set)
        .on(AdminCapabilityDelete, capability_delete)
        .on(AdminHarnessList, harness_list)
        .on(AdminHarnessSet, harness_set)
        .on(AdminHarnessChannelsRead, harness_channels_read)
        .on(AdminHarnessChannelsGet, harness_channels_status)
        .on(AdminHarnessChecksumsImport, harness_checksums_import)
        .on(AdminHarnessCheck, harness_check)
        .on(HarnessBuildList, builds_for_choice)
        .on(HarnessVersionFetch, fetch_version)
        .on(AdminHarnessUsageList, build_usage)
        .on(EnvironmentHarnessBuildList, environment_builds)
        .on(AdminAgentCreate, agent_create);
}

// ---- capabilities ----

/// A definition as the API shows it.
fn def_of(row: CapabilityRow) -> ApiResult<CapabilityDef> {
    let patches: BTreeMap<String, Json> = match row.driver_patch {
        Json::Object(each) => each.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    Ok(CapabilityDef {
        key: row.key,
        label: row.label,
        description: row.description,
        risk: parse(&row.risk)?,
        driver_patch: patches,
        default_decision: parse(&row.default_decision)?,
        default_approver_scope: parse(&row.default_approver_scope)?,
        builtin: row.builtin,
        revision: row.revision,
    })
}

/// Every definition there is, by key.
async fn capability_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<CapabilityDef>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    management::capabilities(&mut conn)
        .await?
        .into_iter()
        .map(def_of)
        .collect()
}

/// The word of a decision as the database keeps it.
fn decision_word(decision: CapabilityDefaultDecision) -> &'static str {
    match decision {
        CapabilityDefaultDecision::Allow => "allow",
        CapabilityDefaultDecision::Ask => "ask",
        CapabilityDefaultDecision::Deny => "deny",
    }
}

fn is_key(key: &str) -> bool {
    CAPABILITY_KEY_CHARS.contains(&key.len())
        && key.starts_with(|first: char| first.is_ascii_lowercase())
        && key
            .chars()
            .all(|each| each.is_ascii_lowercase() || each.is_ascii_digit() || each == '_')
}

/// Writes a definition: adds it, or replaces the one there is when the caller saw its revision.
async fn capability_set(mut ctx: Ctx, (): (), body: CapabilityDef) -> ApiResult<CapabilityDef> {
    admin_of(&ctx)?;
    let key = ctx.param("key")?.to_owned();
    if !is_key(&key) {
        return Err(ApiError::not_found());
    }
    if body.key != key {
        return Err(invalid("key", "must be the key the address names"));
    }
    let label = body.label.trim();
    if label.is_empty() || label.chars().count() > LABEL_MAX_CHARS {
        return Err(invalid("label", "must be 1 to 80 characters"));
    }
    if body.description.chars().count() > DESCRIPTION_MAX_CHARS {
        return Err(invalid("description", "must be at most 1000 characters"));
    }
    for (driver, patch) in &body.driver_patch {
        if driver.is_empty() || driver.chars().count() > WORD_MAX_CHARS {
            return Err(invalid(
                "driver_patch",
                "a driver is named by 1 to 64 characters",
            ));
        }
        if !patch.is_object() {
            return Err(invalid(
                &format!("driver_patch.{driver}"),
                "a patch is an object, in the shape its driver declares",
            ));
        }
    }
    let risk = serde_json::to_value(body.risk)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::internal("a risk is not a word"))?;
    let scope = serde_json::to_value(body.default_approver_scope)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::internal("an approver scope is not a word"))?;
    let patches = Json::Object(body.driver_patch.clone().into_iter().collect());
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let put = management::put_capability(
        &mut tx,
        &CapabilityWrite {
            key: &key,
            label,
            description: &body.description,
            risk: &risk,
            driver_patch: &patches,
            default_decision: decision_word(body.default_decision),
            default_approver_scope: &scope,
        },
        body.revision,
        ctx.now,
    )
    .await?;
    let (row, how) = match put {
        CapabilityPut::Added(row) => (row, "added"),
        CapabilityPut::Replaced(row) => (row, "replaced"),
        CapabilityPut::Stale(row) => {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the definition was changed since it was read",
            )
            .with("current", def_of(row)?));
        }
    };
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        orch_db::audit::action!(AuditAction::CapabilityDefUpdated),
        None,
        &format!("{who} {how} the definition of {key}"),
        json!({ "key": key, "how": how, "revision": row.revision }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    def_of(row)
}

/// Removes the definition of a capability that nobody was given and that does not ship.
async fn capability_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<CapabilityDef> {
    admin_of(&ctx)?;
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let existing = management::capability(&mut tx, &key)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if existing.builtin {
        return Err(invalid(
            "key",
            "a capability that ships with the system is changed, its default decision set to deny, and is not removed: it would be made again at the next start",
        ));
    }
    let holding = management::environments_holding(&mut tx, &key).await?;
    if holding > 0 {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "environments were given this capability: it stays while they have it",
        )
        .with("environments", holding));
    }
    let removed = management::delete_capability(&mut tx, &key)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        AuditAction::CapabilityDefUpdated,
        None,
        &format!("{who} removed the definition of {key}"),
        json!({ "key": key, "how": "removed" }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    def_of(removed)
}

// ---- harness builds ----

/// The builds of harnesses the release lists, from the manifest in its payload. A release without
/// one lists none, and an entry that does not read as a build is not one.
pub(super) async fn release_builds(dir: &Path) -> Vec<HarnessBuild> {
    let Ok(text) = tokio::fs::read_to_string(dir.join("version.json")).await else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_str::<Json>(&text) else {
        return Vec::new();
    };
    manifest
        .get("harness_builds")
        .and_then(Json::as_array)
        .map(|listed| {
            listed
                .iter()
                .filter_map(|entry| serde_json::from_value::<HarnessBuild>(entry.clone()).ok())
                // The builds a release is made and tested with are its checked ones.
                .map(|build| HarnessBuild {
                    source: HarnessBuildSource::Release,
                    status: HarnessBuildStatus::Checked,
                    ..build
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What names a build among the others: its harness, version, system and processor.
fn identity(build: &HarnessBuild) -> (String, String, String, String) {
    let word = |value: &Json| value.as_str().unwrap_or_default().to_owned();
    (
        build.kind.clone(),
        build.version.clone(),
        word(&json!(build.os)),
        word(&json!(build.arch)),
    )
}

/// The key under which an added build is stored.
fn storage_key(build: &HarnessBuild) -> String {
    let (kind, version, os, arch) = identity(build);
    format!(
        "{}{kind}:{version}:{os}:{arch}",
        management::HARNESS_BUILD_PREFIX
    )
}

/// Every build: the release's, and the ones added, which replace the release's of the same name
/// and take the default of their harness, system and processor from the release's.
async fn all_builds(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Vec<HarnessBuild>> {
    Ok(builds_listed(&payload_dir(&ctx.app.config), conn).await?)
}

/// The builds the system lists, from the manifest in the payload of its release, from the vendors' release
/// channels and from the rows an admin added: what a host is told to fetch, and what a person is shown. Where two
/// name the same build the release's stands before a channel's, and an added one before both; an added build that
/// is marked default is the harness's default for its system and processor. A build read from a channel is never a
/// default: a place asks for it by `agents.harness_versions`.
pub(crate) async fn builds_listed(
    payload: &Path,
    conn: &mut Conn,
) -> Result<Vec<HarnessBuild>, DbError> {
    let mut by_identity: BTreeMap<(String, String, String, String), HarnessBuild> = BTreeMap::new();
    let trust_mirror = crate::harness_channels::trusts_mirror(conn).await?;
    for stored in management::channel_harness_builds(conn).await? {
        if !crate::harness_channels::checksum_trusted(&stored, trust_mirror) {
            continue;
        }
        if let Ok(build) = serde_json::from_value::<HarnessBuild>(stored) {
            let build = HarnessBuild {
                is_default: false,
                ..build
            };
            by_identity.insert(identity(&build), build);
        }
    }
    for build in release_builds(payload).await {
        by_identity.insert(identity(&build), build);
    }
    for stored in management::added_harness_builds(conn).await? {
        if !crate::harness_channels::checksum_trusted(&stored, trust_mirror) {
            continue;
        }
        if let Ok(build) = serde_json::from_value::<HarnessBuild>(stored) {
            by_identity.insert(identity(&build), build);
        }
    }
    // One default for each harness, system and processor: an added one wins.
    let admin_defaults: Vec<(String, String, String)> = by_identity
        .values()
        .filter(|build| {
            build.is_default
                && matches!(
                    build.source,
                    HarnessBuildSource::Admin | HarnessBuildSource::Mirror
                )
        })
        .map(|build| {
            let (kind, _, os, arch) = identity(build);
            (kind, os, arch)
        })
        .collect();
    let mut builds: Vec<HarnessBuild> = by_identity.into_values().collect();
    let programs = orch_db::programs::registry(conn).await?;
    let neutral: BTreeMap<String, bool> =
        orch_db::queries::resources::binding::neutral_import_of_kinds(conn)
            .await?
            .into_iter()
            .collect();
    for build in &mut builds {
        if crate::harness_checks::lacks_native_proof(&programs, build) {
            build.status = HarnessBuildStatus::Untested;
        }
        // What switches do to a history this build cannot take over natively is worked out here, from what its
        // program declares and what its check switched off, so that no screen has to know.
        let off = build
            .check
            .as_ref()
            .map(|check| check.off.clone())
            .unwrap_or_default();
        build.carry = Some(crate::programs::carry_of(
            neutral.get(&build.kind).copied().unwrap_or(false),
            &off,
        ));
        let (kind, _, os, arch) = identity(build);
        if build.source == HarnessBuildSource::Release && admin_defaults.contains(&(kind, os, arch))
        {
            build.is_default = false;
        }
    }
    let mirror = orch_settings::get::<String>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
    )
    .await
    .map_err(|error| DbError::Other(sqlx::Error::Protocol(error.to_string())))?;
    for build in &mut builds {
        build.url = crate::harness_channels::download_url(&programs, build, mirror.as_deref());
    }
    Ok(builds)
}

/// The builds of harnesses that hosts may fetch.
async fn harness_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

async fn builds_for_choice(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

/// Scope checks are repeated with the action guard after any vendor request.
async fn authorize_fetch(
    conn: &mut Conn,
    principal: &Principal,
    body: &HarnessVersionFetchBody,
    system_admin: bool,
) -> ApiResult<()> {
    use crate::middleware::authorization;
    match (body.environment_id, body.node_id) {
        (Some(id), None) => {
            if !orch_engine::updates::may_manage(
                conn,
                principal,
                orch_engine::updates::Target::Environment(id),
            )
            .await
            .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "environment.manage is required to fetch a build",
                ));
            }
        }
        (None, Some(id)) => {
            let (_, kind) = crate::routes::nodes::placed(conn, id).await?;
            if !matches!(
                kind,
                orch_api_contract::nodes::NodeKind::Org
                    | orch_api_contract::nodes::NodeKind::Project
            ) {
                return Err(invalid("node_id", "must be an org or project"));
            }
            authorization::require(
                conn,
                principal,
                id,
                authorization::exact(orch_access_contract::permissions::Permission::SettingsManage),
                orch_authz::Act::Use,
                Policy::default(),
            )
            .await?;
        }
        (None, None) if system_admin => {}
        (None, None) => {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "a system administrator browser session is required without a settings scope",
            ));
        }
        _ => return Err(invalid("environment_id", "name one settings scope")),
    }
    Ok(())
}

async fn fetch_version(
    ctx: Ctx,
    (): (),
    body: HarnessVersionFetchBody,
) -> ApiResult<Vec<HarnessBuild>> {
    fetch_exact(ctx.clone(), body).await?;
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

/// Fetches one version under the same transaction authority, keeping the reply bounded by platforms.
pub(super) async fn fetch_exact(
    mut ctx: Ctx,
    body: HarnessVersionFetchBody,
) -> ApiResult<Vec<HarnessBuild>> {
    use crate::middleware::authorization;
    if body.environment_id.is_none() && body.node_id.is_none() {
        admin_of(&ctx)?;
    }
    let kind = ctx.param("kind")?.to_owned();
    let version = ctx.param("version")?.to_owned();
    if !is_word(&version, &['-', '_', '.', '+']) {
        return Err(invalid("version", "must be a version"));
    }
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    authorize_fetch(&mut conn, &principal, &body, ctx.user()?.is_system_admin()).await?;
    let known = super::programs::exact_builds(&ctx, &mut conn, &kind, &version).await?;
    let programs = orch_db::programs::registry(&mut conn).await?;
    let fetch = !known
        .iter()
        .any(|build| build.kind == kind && build.version == version);
    let mirror = if fetch {
        orch_settings::get::<String>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
        )
        .await
        .map_err(ApiError::internal)?
    } else {
        None
    };
    let trust_mirror = crate::harness_channels::trusts_mirror(&mut conn).await?;
    drop(conn);
    let fetched = if fetch {
        Some(
            crate::harness_channels::read_exact_configured(
                &reqwest::Client::new(),
                &programs,
                mirror.as_deref(),
                trust_mirror,
                &kind,
                &version,
            )
            .await
            .map_err(|reason| invalid("version", &reason))?,
        )
    } else {
        None
    };

    // No network work holds the caller's authority locks. The write and check claims use fresh authority.
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    authorize_fetch(
        &mut tx,
        guard.principal(),
        &body,
        guard.user().is_system_admin(),
    )
    .await?;
    if let Some(builds) = fetched {
        crate::harness_channels::keep(&mut tx, &builds, ctx.now).await?;
    }
    let known = super::programs::exact_builds(&ctx, &mut tx, &kind, &version).await?;
    if !known
        .iter()
        .any(|build| build.kind == kind && build.version == version)
    {
        return Err(invalid(
            "version",
            "the requested build is no longer listed; fetch it again",
        ));
    }
    let mut claimed = Vec::new();
    for build in known.into_iter().filter(|build| {
        build.kind == kind
            && build.version == version
            && build.source == HarnessBuildSource::Channel
            && build.status == HarnessBuildStatus::Untested
            && build.check.is_none()
    }) {
        let key = crate::harness_channels::channel_key(&build);
        if crate::harness_checks::claim_in(&mut tx, &key, ctx.now).await? {
            claimed.push((key, build));
        }
    }
    let all = super::programs::exact_builds(&ctx, &mut tx, &kind, &version).await?;
    tx.commit().await.map_err(DbError::from)?;
    for (key, build) in claimed {
        let app = ctx.app.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::harness_checks::finish(&app, &key, &build).await {
                tracing::warn!(%error, "an explicitly requested build could not be checked");
            }
        });
    }
    Ok(all)
}

/// Projects only the immutable checks of the exact tools a plan will install.
pub(crate) async fn target_programs(
    ctx: &Ctx,
    conn: &mut Conn,
    tools: &[orch_engine::hosting::ToolBuild],
) -> ApiResult<Vec<RunningHarnessBuild>> {
    let builds = all_builds(ctx, conn).await?;
    let mut programs = program_tools(tools, &builds);
    name_running_programs(conn, &mut programs).await?;
    Ok(programs)
}

/// Name only the selected builds. A batch uses the same indexed declarations as the Admin page;
/// leaving the catalogue unregistered is distinct from inventing a name from its kind.
async fn name_running_programs(
    conn: &mut Conn,
    programs: &mut [RunningHarnessBuild],
) -> ApiResult<()> {
    for page in programs.chunks_mut(orch_db::programs::admin::NAME_PAGE_SIZE) {
        let kinds: Vec<_> = page
            .iter()
            .map(|program| program.kind.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let names: BTreeMap<_, _> = orch_db::programs::admin::names(conn, &kinds)
            .await?
            .into_iter()
            .map(super::programs::normalized)
            .map(|row| {
                let registry = super::programs::registry(&row);
                let known = registry.program(&row.kind).is_some()
                    || row
                        .descriptor
                        .as_ref()
                        .and_then(|d| d.pointer("/harness/name"))
                        .and_then(Json::as_str)
                        .is_some_and(|name| !name.trim().is_empty());
                let name = known.then(|| {
                    crate::programs::harness_name(&registry, &row.kind, row.descriptor.as_ref())
                });
                (row.kind, name)
            })
            .collect();
        for program in page {
            program.harness = names.get(&program.kind).cloned().flatten();
        }
    }
    Ok(())
}

fn program_tools(
    tools: &[orch_engine::hosting::ToolBuild],
    builds: &[HarnessBuild],
) -> Vec<RunningHarnessBuild> {
    tools
        .iter()
        .filter(|tool| tool.name != "orch-connector")
        .map(|tool| RunningHarnessBuild {
            kind: tool.name.clone(),
            harness: None,
            version: tool.version.clone(),
            build: builds
                .iter()
                .find(|build| {
                    build.kind == tool.name
                        && build.version == tool.version
                        && build.sha256 == tool.sha256
                })
                .cloned(),
        })
        .collect()
}

/// A thread's own report identifies its version. Only the selected runtime's frozen bytes can
/// identify a catalogue check; a connected program's version alone proves no artifact identity.
pub(crate) async fn reported_program(
    conn: &mut Conn,
    thread: orch_types::Id,
) -> ApiResult<Option<RunningHarnessBuild>> {
    let report: Option<(String, String, Option<orch_types::Id>)> = sqlx::query_as(
        "SELECT a.harness_kind,h.harness_version,
        (SELECT r.id FROM runtimes r WHERE r.machine_id=h.machine_id AND r.successor_of IS NULL
            AND r.state NOT IN ('retired','removed') ORDER BY r.id LIMIT 1)
        FROM threads t JOIN agents a ON a.id=t.agent_id JOIN providers p ON p.id=t.provider_id
        JOIN environments e ON e.id=t.environment_id JOIN harnesses h ON h.provider_id=p.id
            AND h.agent_id=a.id AND h.machine_id=CASE WHEN t.arrangement='separated' THEN p.machine_id ELSE e.machine_id END
        WHERE t.node_id=$1 AND h.harness_version IS NOT NULL",
    )
    .bind(thread)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((kind, version, runtime)) = report else {
        return Ok(None);
    };
    let tools: Vec<orch_engine::hosting::ToolBuild> = match runtime {
        Some(runtime) => orch_db::queries::jobs::changes::tools_among(conn, &[runtime])
            .await?
            .into_iter()
            .find_map(|(_, tools)| tools)
            .and_then(|tools| serde_json::from_value(tools).ok())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let programs = orch_db::programs::registry(conn).await?;
    let builds = management::stored_harness_builds(conn)
        .await?
        .into_iter()
        .filter_map(|(_, value)| serde_json::from_value::<HarnessBuild>(value).ok())
        .map(|mut build| {
            if crate::harness_checks::lacks_native_proof(&programs, &build) {
                build.status = HarnessBuildStatus::Untested;
            }
            build
        })
        .collect::<Vec<_>>();
    let mut report = report_with_tools(kind, version, &tools, &builds);
    name_running_programs(conn, std::slice::from_mut(&mut report)).await?;
    Ok(Some(report))
}

fn report_with_tools(
    kind: String,
    version: String,
    tools: &[orch_engine::hosting::ToolBuild],
    builds: &[HarnessBuild],
) -> RunningHarnessBuild {
    program_tools(tools, builds)
        .into_iter()
        .find(|program| program.kind == kind && program.version == version)
        .unwrap_or(RunningHarnessBuild {
            kind,
            harness: None,
            version,
            build: None,
        })
}

async fn held_builds(
    ctx: &Ctx,
    conn: &mut Conn,
    environments: &[orch_types::Id],
    providers: &[orch_types::Id],
) -> ApiResult<
    Vec<(
        orch_db::queries::environments::hosting::RuntimeFull,
        Vec<RunningHarnessBuild>,
    )>,
> {
    let runtimes =
        orch_db::queries::environments::hosting::live_runtimes_of(conn, environments, providers)
            .await?;
    let tools = orch_db::queries::jobs::changes::tools_among(
        conn,
        &runtimes
            .iter()
            .map(|runtime| runtime.id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let builds = all_builds(ctx, conn).await?;
    Ok(runtimes
        .into_iter()
        .map(|runtime| {
            let tools: Vec<orch_engine::hosting::ToolBuild> = tools
                .iter()
                .find(|(id, _)| *id == runtime.id)
                .and_then(|(_, tools)| tools.clone())
                .and_then(|tools| serde_json::from_value(tools).ok())
                .unwrap_or_default();
            let programs = program_tools(&tools, &builds);
            (runtime, programs)
        })
        .collect())
}

async fn environment_builds(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<RunningHarnessBuild>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = crate::middleware::authorization::principal(&ctx, &mut conn).await?;
    let id = ctx.id("id")?;
    crate::routes::environments::show(&ctx, &mut conn, &principal, id).await?;
    let mut held: Vec<_> = held_builds(&ctx, &mut conn, &[id], &[])
        .await?
        .into_iter()
        .flat_map(|(_, programs)| programs)
        .collect();
    if !held.is_empty() {
        name_running_programs(&mut conn, &mut held).await?;
        return Ok(held);
    }
    let reported: Vec<(String,String)> = sqlx::query_as("SELECT DISTINCT a.harness_kind,h.harness_version FROM threads t JOIN environments e ON e.id=t.environment_id JOIN agents a ON a.id=t.agent_id JOIN harnesses h ON h.provider_id=t.provider_id AND h.agent_id=t.agent_id AND h.machine_id=e.machine_id WHERE e.id=$1 AND h.harness_version IS NOT NULL")
        .bind(id).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut programs: Vec<_> = reported
        .into_iter()
        .map(|(kind, version)| RunningHarnessBuild {
            kind,
            harness: None,
            version,
            build: None,
        })
        .collect();
    name_running_programs(&mut conn, &mut programs).await?;
    Ok(programs)
}

async fn build_usage(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuildUsage>> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = crate::middleware::authorization::principal(&ctx, &mut conn).await?;
    let environments: Vec<orch_types::Id> =
        sqlx::query_scalar("SELECT id FROM environments WHERE deleted_at IS NULL")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let providers: Vec<orch_types::Id> =
        sqlx::query_scalar("SELECT id FROM providers WHERE deleted_at IS NULL")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let environments =
        crate::routes::environments::environments_of(&ctx, &mut conn, &principal, &environments)
            .await?;
    let providers =
        crate::routes::resources::view::provider_refs(&mut conn, &providers, &[]).await?;
    let held = held_builds(
        &ctx,
        &mut conn,
        &environments
            .iter()
            .map(|env| env.base.id)
            .collect::<Vec<_>>(),
        &providers
            .iter()
            .map(|(_, provider)| provider.id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let pending = ctx
        .app
        .engine
        .pending_runtime_updates(
            &mut conn,
            &held
                .iter()
                .map(|(runtime, _)| runtime.id)
                .collect::<Vec<_>>(),
        )
        .await
        .map_err(ApiError::internal)?;
    let mut usage: BTreeMap<(String, String), HarnessBuildUsage> = BTreeMap::new();
    for (runtime, programs) in held {
        for program in programs {
            let row = usage
                .entry((program.kind.clone(), program.version.clone()))
                .or_insert_with(|| HarnessBuildUsage {
                    kind: program.kind.clone(),
                    version: program.version.clone(),
                    environments: Vec::new(),
                    providers: Vec::new(),
                    update_environments: Vec::new(),
                    update_providers: Vec::new(),
                });
            let behind = pending.get(&runtime.id).is_some_and(|updates| {
                updates.iter().any(|update| {
                    update.what == orch_api_contract::environment::UpdateWhat::Harness
                        && update.name == program.kind
                })
            });
            if let Some(environment) = environments
                .iter()
                .find(|env| Some(env.base.id) == runtime.environment_id)
            {
                row.environments.push(environment.base.clone());
                if behind {
                    row.update_environments.push(environment.base.id);
                }
            }
            if let Some((_, provider)) = providers
                .iter()
                .find(|(_, provider)| Some(provider.id) == runtime.provider_id)
            {
                row.providers.push(provider.clone());
                if behind {
                    row.update_providers.push(provider.id);
                }
            }
        }
    }
    Ok(usage.into_values().collect())
}

fn is_word(text: &str, extra: &[char]) -> bool {
    !text.is_empty()
        && text.chars().count() <= WORD_MAX_CHARS
        && text.starts_with(|first: char| first.is_ascii_alphanumeric())
        && text
            .chars()
            .all(|each| each.is_ascii_alphanumeric() || extra.contains(&each))
}

/// Adds a build of a harness, or replaces the one of the same name, and lists them all.
async fn harness_set(ctx: Ctx, (): (), body: HarnessBuild) -> ApiResult<Vec<HarnessBuild>> {
    set_build(ctx.clone(), body).await?;
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

/// Validates, authorizes, audits and writes the same build for both catalogue interfaces.
pub(super) async fn set_build(mut ctx: Ctx, body: HarnessBuild) -> ApiResult<HarnessBuild> {
    admin_of(&ctx)?;
    let (kind, version) = (
        ctx.param("kind")?.to_owned(),
        ctx.param("version")?.to_owned(),
    );
    if body.kind != kind {
        return Err(invalid("kind", "must be the harness the address names"));
    }
    if body.version != version {
        return Err(invalid("version", "must be the version the address names"));
    }
    if !is_word(&kind, &['-', '_']) {
        return Err(invalid("kind", "is letters, digits, `-` and `_`"));
    }
    if !is_word(&version, &['-', '_', '.', '+']) {
        return Err(invalid(
            "version",
            "is letters, digits, `.`, `+`, `-` and `_`",
        ));
    }
    if !is_address(&body.url) {
        return Err(invalid("url", "is an http or https address"));
    }
    if body.sha256.len() != SHA256_HEX_CHARS
        || !body
            .sha256
            .chars()
            .all(|each| each.is_ascii_digit() || ('a'..='f').contains(&each))
    {
        return Err(invalid("sha256", "is 64 hexadecimal digits in lower case"));
    }
    // What a build is known to do is the system's to say, never the request's.
    let mirror_checksum = body.source == HarnessBuildSource::Mirror;
    let channel_choice = body.source == HarnessBuildSource::Channel;
    let build = HarnessBuild {
        source: if mirror_checksum {
            HarnessBuildSource::Mirror
        } else {
            HarnessBuildSource::Admin
        },
        status: HarnessBuildStatus::Untested,
        check: None,
        ..body
    };
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    if channel_choice {
        let stored = orch_db::queries::settings::row(
            &mut tx,
            None,
            &crate::harness_channels::channel_key(&build),
        )
        .await?;
        if stored.as_ref().is_none_or(|stored| {
            !crate::harness_channels::checksum_trusted(&stored.value, false)
                || stored.value.get("sha256").and_then(Json::as_str) != Some(build.sha256.as_str())
        }) {
            return Err(invalid(
                "source",
                "import this build's vendor checksum before choosing it",
            ));
        }
    }
    if mirror_checksum && !crate::harness_channels::trusts_mirror(&mut tx).await? {
        return Err(invalid(
            "source",
            "mirror checksums require the explicit system trust setting",
        ));
    }
    let mut stored = serde_json::to_value(&build).map_err(ApiError::internal)?;
    stored["checksum_origin"] = Json::from(if mirror_checksum {
        "mirror"
    } else if channel_choice {
        "vendor"
    } else {
        "admin"
    });
    management::put_harness_build(&mut tx, &storage_key(&build), &stored, admin, ctx.now).await?;
    let who = guard.user().user.display_name.clone();
    record(
        &ctx,
        &mut tx,
        admin,
        AuditAction::SystemSettingsUpdated,
        None,
        &format!("{who} added the build {version} of {kind}"),
        json!({
            "what": "harness_build", "kind": kind, "version": version,
            "os": build.os, "arch": build.arch, "is_default": build.is_default,
        }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(build)
}

/// Imports vendor checksums carried in from the connected side, with one digest-bound audit entry.
async fn harness_checksums_import(
    mut ctx: Ctx,
    (): (),
    body: HarnessChecksumsImportBody,
) -> ApiResult<HarnessChecksumsImportReply> {
    use sha2::{Digest, Sha256};
    admin_of(&ctx)?;
    let builds: Vec<HarnessBuild> = serde_json::from_str(&body.document)
        .map_err(|_| invalid("document", "must be a JSON list of gathered vendor builds"))?;
    if builds.is_empty() {
        return Err(invalid("document", "must list at least one build"));
    }
    let mut identities = std::collections::BTreeSet::new();
    for build in &builds {
        if !is_word(&build.kind, &['-', '_'])
            || !is_word(&build.version, &['-', '_', '.', '+'])
            || !is_address(&build.url)
            || build.sha256.len() != SHA256_HEX_CHARS
            || !build
                .sha256
                .bytes()
                .all(|each| each.is_ascii_digit() || (b'a'..=b'f').contains(&each))
            || !identities.insert(identity(build))
        {
            return Err(invalid(
                "document",
                "each build needs a unique identity, download address and lower-case SHA-256",
            ));
        }
    }
    let digest = hex::encode(Sha256::digest(body.document.as_bytes()));
    // A list may name successive defaults. As for individual additions, its last
    // default for a platform wins; importing checksums must not leave two defaults.
    let mut defaults = BTreeMap::new();
    for build in &builds {
        if build.is_default {
            let (kind, version, os, arch) = identity(build);
            defaults.insert((kind, os, arch), version);
        }
    }
    let count = i64::try_from(builds.len()).map_err(ApiError::internal)?;
    let mut anchored = Vec::with_capacity(builds.len());
    for build in builds {
        let (kind, version, os, arch) = identity(&build);
        let build = HarnessBuild {
            source: HarnessBuildSource::Admin,
            status: HarnessBuildStatus::Untested,
            check: None,
            is_default: build.is_default && defaults.get(&(kind, os, arch)) == Some(&version),
            ..build
        };
        let key = storage_key(&build);
        let mut value = serde_json::to_value(build).map_err(ApiError::internal)?;
        value["checksum_origin"] = Json::from("vendor");
        anchored.push((key, value));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    management::put_imported_harness_builds(&mut tx, &anchored, guard.user().id(), ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        guard.user().id(),
        AuditAction::SystemSettingsUpdated,
        None,
        &format!("Imported vendor checksum list {digest}"),
        json!({"what":"harness_checksums", "sha256":digest, "count":count}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(HarnessChecksumsImportReply {
        sha256: digest,
        count,
    })
}

/// Reads the vendors' release channels now, as the schedule does, keeps what is new and lists every build. A vendor
/// that cannot be read is said, and the other's builds are kept all the same.
async fn harness_channels_read(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    read_channels(ctx.clone(), None).await?;
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

/// The explicit channel operation does not need the catalogue as its acknowledgement.
pub(super) async fn read_channels(ctx: Ctx, kind: Option<String>) -> ApiResult<u32> {
    admin_of(&ctx)?;
    let (mirror, trust_mirror) = {
        let mut conn = ctx.app.db.acquire().await?;
        let mirror = orch_settings::get::<String>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::AGENTS_HARNESS_MIRROR,
        )
        .await
        .map_err(ApiError::internal)?;
        let trust_mirror = crate::harness_channels::trusts_mirror(&mut conn).await?;
        (mirror, trust_mirror)
    };
    let client = reqwest::Client::new();
    let (programs, kept) = {
        let mut conn = ctx.app.db.acquire().await?;
        (
            orch_db::programs::registry(&mut conn).await?,
            if let Some(kind) = kind.as_deref() {
                sqlx::query_scalar::<_, String>(
                    "SELECT setting_key FROM agent_program_builds WHERE kind=$1 AND priority=1",
                )
                .bind(kind)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?
                .into_iter()
                .collect()
            } else {
                crate::harness_channels::kept_keys(&mut conn).await?
            },
        )
    };
    let (builds, failures) = if let Some(kind) = kind {
        crate::harness_channels::read_for_configured(
            &client,
            &programs,
            mirror.as_deref(),
            trust_mirror,
            &std::collections::BTreeSet::from([kind]),
            &kept,
        )
        .await
    } else {
        crate::harness_channels::read_configured(
            &client,
            &programs,
            mirror.as_deref(),
            trust_mirror,
            &kept,
        )
        .await
    };
    let unavailable = builds.is_empty() && !failures.is_empty();
    // What was read is kept in the transaction that checks the caller's authority again, never during the reads.
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    crate::harness_channels::record_read(&mut tx, guard.now().get(), !unavailable).await?;
    if unavailable {
        tx.commit().await.map_err(DbError::from)?;
        return Err(ApiError::new(
            ErrorCode::HarnessChannelsUnavailable,
            format!(
                "the release channels could not be read: {}",
                failures.join("; ")
            ),
        ));
    }
    let retained = crate::harness_channels::keep(&mut tx, &builds, guard.now().get()).await?;
    let kept = u32::try_from(retained).map_err(ApiError::internal)?;
    tx.commit().await.map_err(DbError::from)?;
    // What is new is checked in the background; the list says which build is being checked.
    let app = ctx.app.clone();
    tokio::spawn(async move { crate::harness_checks::check_waiting(&app).await });
    Ok(kept)
}

/// Reads local channel state and policy without contacting a release service.
async fn harness_channels_status(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::admin::HarnessChannelsStatus> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    Ok(crate::harness_channels::status(&mut conn).await?)
}

/// Checks every kept build of a harness at a version again (one for each system and processor), in the background,
/// and lists the builds, those being checked marked so. A release's own builds are checked with the release.
async fn harness_check(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<HarnessBuild>> {
    queue_check(ctx.clone()).await?;
    let mut conn = ctx.app.db.acquire().await?;
    all_builds(&ctx, &mut conn).await
}

/// Claims exact-version checks under the same authority transaction, without loading the catalogue.
pub(super) async fn queue_check(ctx: Ctx) -> ApiResult<u32> {
    admin_of(&ctx)?;
    let kind = ctx.param("kind")?;
    let version = ctx.param("version")?;
    // The builds are marked as being checked in the transaction that checks the caller's authority again; their
    // checks run once it is committed.
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let kept = crate::harness_checks::kept(&mut tx, kind, version).await?;
    if kept.is_empty() {
        return Err(ApiError::not_found());
    }
    let mut claimed = Vec::new();
    for (key, build) in kept {
        if crate::harness_checks::claim_in(&mut tx, &key, guard.now().get()).await? {
            claimed.push((key, build));
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    let queued = u32::try_from(claimed.len()).map_err(ApiError::internal)?;
    for (key, build) in claimed {
        let app = ctx.app.clone();
        tokio::spawn(async move {
            if let Err(error) = crate::harness_checks::finish(&app, &key, &build).await {
                tracing::warn!(%error, "a harness build could not be checked");
            }
        });
    }
    Ok(queued)
}

// ---- profiles of the system ----

/// The profile as a body gives it: only the members a profile has, and of the right kinds; a
/// profile of the system is attached to no machine.
fn clean_profile(profile: &Json) -> ApiResult<Json> {
    let Some(members) = profile.as_object() else {
        return Err(invalid("profile", "a profile is an object"));
    };
    if let Some(unknown) = members
        .keys()
        .find(|name| !PROFILE_MEMBERS.contains(&name.as_str()))
    {
        return Err(invalid(
            &format!("profile.{unknown}"),
            "a profile has no such member",
        ));
    }
    let parsed: AgentProfile = serde_json::from_value(profile.clone())
        .map_err(|error| invalid("profile", &error.to_string()))?;
    if parsed.attach.is_some() {
        return Err(invalid(
            "profile.attach",
            "only a profile of a harness on a person's machine says how it is reached",
        ));
    }
    serde_json::to_value(&parsed).map_err(ApiError::internal)
}

/// The kind of a harness as an adapter names itself.
fn clean_harness_kind(kind: &str) -> ApiResult<String> {
    let kind = kind.trim();
    if is_word(kind, &['-', '_']) && kind.starts_with(|first: char| first.is_ascii_lowercase()) {
        Ok(kind.to_owned())
    } else {
        Err(invalid(
            "harness_kind",
            "a harness kind is lower case letters, digits, `-` and `_`, beginning with a letter",
        ))
    }
}

/// Makes an agent profile of the system, which has no home and no owner.
async fn agent_create(mut ctx: Ctx, (): (), body: OrgAgentCreateBody) -> ApiResult<Agent> {
    admin_of(&ctx)?;
    let name = clean_name("name", &body.name)?;
    let harness_kind = clean_harness_kind(&body.harness_kind)?;
    let profile = clean_profile(&body.profile)?;
    let id = orch_types::new_id();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    management::insert_system_agent(&mut tx, id, &name, &harness_kind, &profile, ctx.now).await?;
    let who = guard.user().user.display_name.clone();
    record_resource(
        &ctx,
        &mut tx,
        AuditAction::AgentCreated,
        id,
        Home::default(),
        &format!("{who} added the agent profile {name} to the system"),
        json!({ "agent": id, "name": name, "harness_kind": harness_kind, "system": true }),
    )
    .await?;
    let row = agent_rows::agents(&mut tx, &[id])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("a profile was made and is not there"))?;
    let held = resources::effective(
        &mut tx,
        principal,
        ObjectRef {
            kind: ResourceKind::Agent.into(),
            id,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let holding = Holding {
        admin_only: &held.admin_only,
        admin_access: held.admin_access,
        admin_place: held.admin_place,
        participating: held.participating,
        id,
        owner: held.owner,
        administers: held.administers,
        held: &held.held,
    };
    let access = accesses(&mut tx, principal, &[holding])
        .await?
        .remove(&id)
        .ok_or_else(|| ApiError::internal("what the caller holds on a profile was not read"))?;
    let address = addresses(&mut tx, &[id])
        .await?
        .remove(&id)
        .unwrap_or_default();
    tx.commit().await.map_err(DbError::from)?;
    Ok(Agent {
        providers: None,
        models: None,
        base: AgentRef {
            id,
            name: row.name,
            harness: crate::programs::shipped_name(&row.harness_kind, Some(&row.descriptor)),
            harness_kind: row.harness_kind,
            address,
            machine: None,
            disabled: row.disabled,
        },
        owner: None,
        org_id: None,
        home: None,
        profile: row.profile,
        descriptor: row.descriptor,
        descriptor_overridden: Vec::new(),
        revision: row.revision,
        created_at: Time::new(row.created_at),
        updated_at: Time::new(row.updated_at),
        access,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    async fn a_running_build_without_a_catalogue_check_keeps_its_declared_localized_name() {
        let test = orch_testkit::database::TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        sqlx::query("INSERT INTO agent_program_kinds(kind,manifest) VALUES ($1,$2)")
            .bind("named-external")
            .bind(
                json!({ "manifest_version": 1, "kind": "named-external", "protocol": "fake",
                "names": { "en": "Research assistant", "zh-CN": "研究助理" } }),
            )
            .execute(&mut *conn)
            .await
            .unwrap();
        let mut programs: Vec<_> = (0..117)
            .map(|index| RunningHarnessBuild {
                kind: match index % 3 {
                    0 => "fake",
                    1 => "named-external",
                    _ => "unreported-name",
                }
                .into(),
                harness: None,
                version: "1.0.0".into(),
                build: None,
            })
            .collect();
        name_running_programs(&mut conn, &mut programs)
            .await
            .unwrap();
        assert_eq!(
            programs.len(),
            117,
            "name batches must preserve every selected build"
        );
        for program in &programs {
            assert!(
                program.build.is_none(),
                "names must not invent a catalogue check"
            );
            match program.kind.as_str() {
                "fake" => {
                    let name = program.harness.as_ref().unwrap();
                    assert_eq!(name.name, "Example agent");
                    assert_eq!(name.in_language("zh-CN"), "示例代理");
                }
                "named-external" => {
                    let name = program.harness.as_ref().unwrap();
                    assert_eq!(name.name, "Research assistant");
                    assert_eq!(name.in_language("zh-CN"), "研究助理");
                }
                _ => assert!(
                    program.harness.is_none(),
                    "an unknown kind is not a human name"
                ),
            }
        }
        let too_many = vec!["fake".to_owned(); orch_db::programs::admin::NAME_PAGE_SIZE + 1];
        assert!(
            orch_db::programs::admin::names(&mut conn, &too_many)
                .await
                .is_err(),
            "the lookup must reject an unbounded name selection"
        );
    }

    #[test]
    fn a_reported_version_has_no_borrowed_check_without_the_same_frozen_bytes() {
        let selected = HarnessBuild {
            kind: "codex".into(),
            version: "0.164.0".into(),
            os: orch_api_contract::resources::MachineOs::Linux,
            arch: orch_api_contract::admin::HarnessArch::X86_64,
            url: "https://build.test/program".into(),
            sha256: "a".repeat(64),
            is_default: false,
            source: HarnessBuildSource::Channel,
            status: HarnessBuildStatus::Failed,
            check: None,
            carry: None,
        };
        let other = HarnessBuild {
            arch: orch_api_contract::admin::HarnessArch::Aarch64,
            sha256: "b".repeat(64),
            status: HarnessBuildStatus::Checked,
            ..selected.clone()
        };
        let tool = orch_engine::hosting::ToolBuild {
            name: selected.kind.clone(),
            version: selected.version.clone(),
            url: selected.url.clone(),
            sha256: selected.sha256.clone(),
        };
        let report = |tools: &[orch_engine::hosting::ToolBuild], builds: &[HarnessBuild]| {
            report_with_tools(
                selected.kind.clone(),
                selected.version.clone(),
                tools,
                builds,
            )
        };
        assert!(report(&[], std::slice::from_ref(&other)).build.is_none());
        assert!(
            report(std::slice::from_ref(&tool), std::slice::from_ref(&other))
                .build
                .is_none()
        );
        assert_eq!(
            report(std::slice::from_ref(&tool), &[other, selected.clone()]).build,
            Some(selected.clone())
        );
        assert!(
            report_with_tools(
                selected.kind.clone(),
                "0.162.1".into(),
                &[tool],
                &[selected],
            )
            .build
            .is_none()
        );
    }

    #[test]
    fn a_capability_key_is_what_the_schema_allows() {
        for good in ["gpu", "net_admin", "a1"] {
            assert!(is_key(good), "{good}");
        }
        for bad in ["", "g", "GPU", "1gpu", "net-admin", &"k".repeat(33)] {
            assert!(!is_key(bad), "{bad}");
        }
    }
}
