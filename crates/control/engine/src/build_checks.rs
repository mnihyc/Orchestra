//! Native build-check claims and their exact current operation, program and physical host.

use crate::{
    EngineError,
    operation_scope::{
        continuing::{self, Error, FirstEffect, Source},
        sources::Facts,
        targets,
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    admin::{AdminHarnessCheck, HarnessBuild, HarnessVersionFetch},
    admin_sources::{self, Source as Role},
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver},
    program_admin::{AdminProgramBuildCheck, AdminProgramVersionFetch},
    routes::{Endpoint, Route},
};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        resources::{
            hosting::{self, HostState},
            management,
        },
        settings,
    },
};
use orch_types::{Clock, Hash, Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

/// The mutually exclusive native settings destination, resolved again at every new act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// Existing environment management.
    Environment(Id),
    /// Existing organization/project settings management.
    Node(Id),
    /// Current ordinary system administration.
    System,
}
impl Destination {
    /// The same factual target used by the native fetch and its retained workers.
    pub async fn facts(&self, conn: &mut Conn) -> Result<Facts<Role>, EngineError> {
        let family = match self {
            Self::Environment(_) => Some(KeyTargetKind::Object(ObjectKind::Environment)),
            Self::System => Some(targets::system().object.kind),
            Self::Node(_) => None,
        };
        let target = match self {
            Self::Environment(id) => targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: *id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?,
            Self::Node(id) => {
                let target = targets::node(conn, *id)
                    .await
                    .map_err(EngineError::unreadable)?;
                if !matches!(
                    target.object.kind,
                    KeyTargetKind::Object(ObjectKind::Org | ObjectKind::Project)
                ) {
                    return Err(EngineError::unreadable(
                        "the build destination must be an organization or project",
                    ));
                }
                target
            }
            Self::System => targets::system(),
        };
        Ok(Facts::one(
            Role::Destination,
            family.unwrap_or(target.object.kind),
            target,
        ))
    }
    /// The existing ordinary native prerequisite; scope is admitted separately by the common frame.
    pub async fn may_manage(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<bool, EngineError> {
        match self {
            Self::Environment(id) => {
                crate::updates::may_manage(
                    conn,
                    principal,
                    crate::updates::Target::Environment(*id),
                )
                .await
            }
            Self::Node(id) => Ok(orch_authz::nodes::check(
                conn,
                principal,
                *id,
                Permission::SettingsManage,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()),
            Self::System => Ok(orch_authz::system_administration::on_system(
                principal,
                targets::system().object.kind,
            )),
        }
    }
    /// Canonical selection comes only from the validated native destination.
    #[must_use]
    pub fn declaration(&self) -> &'static Declaration<Role> {
        match self {
            Self::Environment(_) => &admin_sources::FETCH_ENVIRONMENT,
            Self::Node(_) => &admin_sources::FETCH_NODE,
            Self::System => &admin_sources::FETCH_SYSTEM,
        }
    }
}

const SYSTEM: Binding<Role> = Binding {
    source: Role::Destination,
    target: TargetResolver::System,
    usage: FacetUsage::Required,
    cardinality: Cardinality::One,
};
const CATALOGUE: Binding<Role> = Binding {
    source: Role::Catalogue,
    target: TargetResolver::System,
    usage: FacetUsage::ResponseContent,
    cardinality: Cardinality::One,
};
/// The existing administrative program fetch shares this native writer and its own canonical operation.
pub const PROGRAM_FETCH: Declaration<Role> = Declaration {
    operation: "AdminProgramVersionFetch",
    bindings: &[SYSTEM, CATALOGUE],
};
const ADMIN_CHECK: Declaration<Role> = Declaration {
    operation: "AdminHarnessCheck",
    bindings: &[SYSTEM, CATALOGUE],
};
const PROGRAM_CHECK: Declaration<Role> = Declaration {
    operation: "AdminProgramBuildCheck",
    bindings: &[SYSTEM],
};

/// Exact native request provenance; this is never stored in a public setting value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    origin: CapturedOperation,
    destination: Destination,
}
impl Request {
    /// Capture the actual admitted operation, including the older callers of the same native checker.
    pub fn capture(caller: &Caller, destination: Destination) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let valid = match origin.operation() {
            "HarnessVersionFetch.environment" => matches!(destination, Destination::Environment(_)),
            "HarnessVersionFetch.node" => matches!(destination, Destination::Node(_)),
            "HarnessVersionFetch.system"
            | "AdminProgramVersionFetch"
            | "AdminHarnessCheck"
            | "AdminProgramBuildCheck" => destination == Destination::System,
            _ => false,
        };
        if !valid {
            return Err(EngineError::unreadable(
                "the native build request changed its operation or destination",
            ));
        }
        Ok(Self {
            origin,
            destination,
        })
    }
}

/// Immutable build bytes and platform, excluding mutable check status and projected download URLs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    kind: String,
    version: String,
    os: String,
    arch: String,
    sha256: String,
    source: String,
}
impl From<&HarnessBuild> for Build {
    fn from(value: &HarnessBuild) -> Self {
        let word = |value: Json| value.as_str().unwrap_or_default().to_owned();
        Self {
            kind: value.kind.clone(),
            version: value.version.clone(),
            os: word(serde_json::json!(value.os)),
            arch: word(serde_json::json!(value.arch)),
            sha256: value.sha256.clone(),
            source: word(serde_json::json!(value.source)),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Origin {
    Scheduled,
    Requested { request: Request },
}

/// One actual claimed native check. A later scheduled check has a different ID and origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    id: Id,
    key: String,
    build: Build,
    program: Option<Hash>,
    origin: Origin,
}
impl Claim {
    /// Prepare the actual immutable native claim before its first status write.
    pub async fn prepare(
        conn: &mut Conn,
        key: &str,
        request: Option<Request>,
    ) -> Result<Self, EngineError> {
        let row = settings::row(conn, None, key)
            .await?
            .ok_or_else(|| EngineError::unreadable("the claimed build disappeared"))?;
        let build: HarnessBuild =
            serde_json::from_value(row.value).map_err(EngineError::unreadable)?;
        Ok(Self {
            id: new_id(),
            key: key.to_owned(),
            program: program(conn, &build.kind).await?,
            build: Build::from(&build),
            origin: request.map_or(Origin::Scheduled, |request| Origin::Requested { request }),
        })
    }
    /// Keep authority in its own server-private column, in the same transaction as the claim.
    pub async fn store(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let value = serde_json::to_value(self)?;
        sqlx::query("UPDATE settings SET harness_check_origin=$2 WHERE node_id IS NULL AND key=$1")
            .bind(&self.key)
            .bind(value)
            .execute(conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    /// Load a real claim; malformed or absent evidence never reconstructs a caller.
    pub async fn load(conn: &mut Conn, key: &str) -> Result<Option<Self>, EngineError> {
        let value: Option<Json> = sqlx::query_scalar(
            "SELECT harness_check_origin FROM settings WHERE node_id IS NULL AND key=$1",
        )
        .bind(key)
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)?
        .flatten();
        value
            .map(|value| serde_json::from_value(value).map_err(EngineError::unreadable))
            .transpose()
    }
    /// Release only this accepted claim without granting a new certification or reviving a key.
    pub async fn release(&self, conn: &mut Conn, at: Time) -> Result<(), EngineError> {
        let value = serde_json::to_value(self)?;
        sqlx::query("UPDATE settings SET value=jsonb_set(value,'{status}','\"untested\"'),revision=revision+1,updated_at=$3
            WHERE node_id IS NULL AND key=$1 AND harness_check_origin=$2 AND value->>'status'='checking' AND value->>'sha256'=$4")
            .bind(&self.key).bind(value).bind(at.get()).bind(&self.build.sha256).execute(conn).await.map_err(DbError::from)?;
        Ok(())
    }
    /// Accepted negative results can remove availability but cannot certify a new build.
    pub async fn finish_negative(
        &self,
        conn: &mut Conn,
        status: orch_api_contract::admin::HarnessBuildStatus,
        check: &Json,
        at: Time,
    ) -> Result<(), EngineError> {
        let status = match status {
            orch_api_contract::admin::HarnessBuildStatus::Untested => "untested",
            orch_api_contract::admin::HarnessBuildStatus::Failed => "failed",
            _ => {
                return Err(EngineError::unreadable(
                    "a positive certification needs its current original operation",
                ));
            }
        };
        let origin = serde_json::to_value(self)?;
        sqlx::query("UPDATE settings SET value=jsonb_set(jsonb_set(value,'{status}',to_jsonb($3::text)),'{check}',$4),revision=revision+1,updated_at=$5
            WHERE node_id IS NULL AND key=$1 AND harness_check_origin=$2 AND value->>'status'='checking' AND value->>'sha256'=$6")
            .bind(&self.key).bind(origin).bind(status).bind(check).bind(at.get()).bind(&self.build.sha256)
            .execute(conn).await.map_err(DbError::from)?;
        Ok(())
    }
    /// The real build supplied to the worker must match this claim's original bytes and platform.
    #[must_use]
    pub fn matches(&self, build: &HarnessBuild) -> bool {
        self.build == Build::from(build)
    }
    /// A native writer cannot validate one claim and publish into another retained row.
    #[must_use]
    pub fn owns(&self, key: &str, build: &HarnessBuild) -> bool {
        self.key == key && self.matches(build)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    id: Id,
    machine: Id,
    home: Option<Id>,
    org: Option<Id>,
}

/// Immutable native work association through every command and result-publication wait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    claim: Claim,
    host: Option<Host>,
}
impl Work {
    /// Start only from the claim actually returned by the native claimant.
    #[must_use]
    pub fn new(claim: Claim) -> Self {
        Self { claim, host: None }
    }
    /// Keep the actual selected native host, not a fresh replacement selected after a wait.
    #[must_use]
    pub fn on_host(&self, host: &HostState) -> Self {
        Self {
            claim: self.claim.clone(),
            host: Some(Host {
                id: host.id,
                machine: host.machine_id,
                home: host.home_id,
                org: host.org_id,
            }),
        }
    }
    /// Keep the exact accepted claim for its independent cancellation/release path.
    #[must_use]
    pub fn claim(&self) -> &Claim {
        &self.claim
    }
    /// Prepare native operands, then use the shared current original-operation frame when present.
    pub async fn checkpoint(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Checked>, Error> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(
                EngineError::unreadable("native build work requires its transaction").into(),
            );
        }
        let Origin::Requested { request } = &self.claim.origin else {
            orch_db::queries::transaction_guard::lock_action(conn).await?;
            factual(conn, self).await?;
            locks(conn, self).await?;
            factual(conn, self).await?;
            return Ok(None);
        };
        macro_rules! frame {
            ($mode:ty) => {{
                let parts = continuing::prepare::<Native<$mode>>(
                    conn,
                    clock,
                    &request.origin,
                    self.clone(),
                )
                .await?
                .into_parts();
                Ok(Some(Checked {
                    caller: parts.principal.caller().clone(),
                    before: parts.first_effect,
                }))
            }};
        }
        match request.origin.operation() {
            "HarnessVersionFetch.environment" => frame!(Environment),
            "HarnessVersionFetch.node" => frame!(Node),
            "HarnessVersionFetch.system" => frame!(System),
            "AdminProgramVersionFetch" => frame!(ProgramFetch),
            "AdminHarnessCheck" => frame!(AdminCheck),
            "AdminProgramBuildCheck" => frame!(ProgramCheck),
            _ => Err(Error::Refused),
        }
    }
}
/// Current original actor and the once-only native first-effect deadline cut.
pub struct Checked {
    /// Passed through the dispatcher's later native/currentness waits.
    pub caller: Caller,
    /// Consumed at the actual SQL or command insertion on this transaction.
    pub before: FirstEffect,
}

/// Existing checksum publisher semantics, shared by the ordinary catalogue and checker.
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
/// Current ordinary mirror-publisher choice, with the original native database error shape.
pub async fn trusts_mirror(conn: &mut Conn) -> Result<bool, DbError> {
    orch_settings::get::<bool>(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::AGENTS_HARNESS_TRUST_MIRROR,
    )
    .await
    .map(|value| value.unwrap_or(false))
    .map_err(|error| DbError::Other(sqlx::Error::Protocol(error.to_string())))
}
async fn program(conn: &mut Conn, kind: &str) -> Result<Option<Hash>, EngineError> {
    orch_db::programs::registry(conn)
        .await?
        .program(kind)
        .map(|manifest| crate::jobs::command_identity::hash(&serde_json::to_value(manifest)?))
        .transpose()
}
async fn factual(conn: &mut Conn, work: &Work) -> Result<(), Error> {
    if Claim::load(conn, &work.claim.key).await?.as_ref() != Some(&work.claim) {
        return Err(Error::Refused);
    }
    let row = settings::row(conn, None, &work.claim.key)
        .await?
        .ok_or(Error::Refused)?;
    if row.value.get("status").and_then(Json::as_str) != Some("checking")
        || !checksum_trusted(&row.value, trusts_mirror(conn).await?)
    {
        return Err(Error::Refused);
    }
    let build: HarnessBuild = serde_json::from_value(row.value).map_err(EngineError::unreadable)?;
    if !work.claim.matches(&build) || program(conn, &build.kind).await? != work.claim.program {
        return Err(Error::Refused);
    }
    if let Some(old) = &work.host {
        let host = hosting::host(conn, old.id).await?.ok_or(Error::Refused)?;
        orch_db::queries::transaction_guard::watch_nodes(
            conn,
            &host.home_id.into_iter().collect::<Vec<_>>(),
        )
        .await?;
        if host.machine_id != old.machine
            || host.home_id != old.home
            || host.org_id != old.org
            || !eligible_host(conn, &host, &work.claim.build.os, &work.claim.build.arch).await?
        {
            return Err(Error::Refused);
        }
    }
    Ok(())
}
async fn locks(conn: &mut Conn, work: &Work) -> Result<(), Error> {
    sqlx::query("SELECT key FROM settings WHERE node_id IS NULL AND key=$1 FOR UPDATE")
        .bind(&work.claim.key)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if let Some(host) = &work.host {
        sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
            .bind(host.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
        sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
            .bind(host.machine)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    Ok(())
}
/// The original native checker eligibility, also used when retaining a selected host.
pub async fn eligible_host(
    conn: &mut Conn,
    host: &HostState,
    os: &str,
    arch: &str,
) -> Result<bool, DbError> {
    Ok(candidate(host, os, arch)
        && management::personal_orgs(conn, &host.org_id.into_iter().collect::<Vec<_>>())
            .await?
            .is_empty())
}
fn candidate(host: &HostState, os: &str, arch: &str) -> bool {
    host.online
        && host.machine_status == "active"
        && !host.disabled
        && host.platform_os == os
        && host.platform_arch == arch
}
/// The original fleet selector and ordering, shared with retained physical-host validation.
pub async fn host_for(conn: &mut Conn, os: &str, arch: &str) -> Result<Option<HostState>, DbError> {
    let hosts = hosting::hosts(conn, None)
        .await?
        .into_iter()
        .filter(|host| candidate(host, os, arch))
        .collect::<Vec<_>>();
    let personal = management::personal_orgs(
        conn,
        &hosts
            .iter()
            .filter_map(|host| host.org_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let mut hosts = hosts
        .into_iter()
        .filter(|host| host.org_id.is_none_or(|org| !personal.contains(&org)))
        .collect::<Vec<_>>();
    hosts.sort_by_key(|host| (host.org_id.is_some(), !host.schedulable));
    Ok(hosts.into_iter().next())
}

trait Mode: Send + Sync + 'static {
    const ROUTE: Route;
    const ROW: &'static Declaration<Role>;
}
macro_rules! mode {
    ($name:ident, $route:expr, $row:expr) => {
        struct $name;
        impl Mode for $name {
            const ROUTE: Route = $route;
            const ROW: &'static Declaration<Role> = $row;
        }
    };
}
mode!(
    Environment,
    HarnessVersionFetch::ROUTE,
    &admin_sources::FETCH_ENVIRONMENT
);
mode!(Node, HarnessVersionFetch::ROUTE, &admin_sources::FETCH_NODE);
mode!(
    System,
    HarnessVersionFetch::ROUTE,
    &admin_sources::FETCH_SYSTEM
);
mode!(
    ProgramFetch,
    AdminProgramVersionFetch::ROUTE,
    &PROGRAM_FETCH
);
mode!(AdminCheck, AdminHarnessCheck::ROUTE, &ADMIN_CHECK);
mode!(ProgramCheck, AdminProgramBuildCheck::ROUTE, &PROGRAM_CHECK);
struct Native<M>(PhantomData<M>);
impl<M: Mode> Source for Native<M> {
    type Role = Role;
    type Input = Work;
    type Value = Work;
    const ROUTE: Route = M::ROUTE;
    const DECLARATION: &'static Declaration<Role> = M::ROW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Work,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Work, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            factual(conn, input).await?;
            let Origin::Requested { request } = &input.claim.origin else {
                return Err(Error::Refused);
            };
            if request.origin.operation() != M::ROW.operation {
                return Err(Error::Refused);
            }
            Ok((input.clone(), vec![request.destination.facts(conn).await?]))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Work,
        _value: &'a Work,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let Origin::Requested { request } = &input.claim.origin else {
                return Err(Error::Refused);
            };
            if request.destination.may_manage(conn, principal).await? {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Work,
        _value: &'a Work,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(locks(conn, input))
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Work,
        old: &'a Work,
        fresh: &'a Work,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

/// Server-private physical command evidence; it carries no machine-visible authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckCommand {
    work: Work,
    canonical: Json,
    kind: CommandKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandKind {
    Ensure,
    Interface,
    Conformance,
}
impl CheckCommand {
    /// Bind the actual server-produced command to its immutable native claim and physical host.
    pub fn capture(work: &Work, command: &crate::commands::Outgoing) -> Result<Self, EngineError> {
        let kind = match command.kind {
            orch_link_contract::commands::CommandKind::NodeToolEnsure => CommandKind::Ensure,
            orch_link_contract::commands::CommandKind::NodeToolCheck => CommandKind::Interface,
            orch_link_contract::commands::CommandKind::NodeToolConformance => {
                CommandKind::Conformance
            }
            _ => return Err(EngineError::unreadable("not a native build check command")),
        };
        let same_build = match kind {
            CommandKind::Ensure => {
                let body: orch_link_contract::commands::NodeToolEnsure =
                    serde_json::from_value(command.body.clone())?;
                body.name == work.claim.build.kind
                    && body.version == work.claim.build.version
                    && body.sha256 == work.claim.build.sha256
                    && body.job_id.is_none()
            }
            CommandKind::Interface => {
                let body: orch_link_contract::commands::NodeToolCheck =
                    serde_json::from_value(command.body.clone())?;
                body.name == work.claim.build.kind && body.version == work.claim.build.version
            }
            CommandKind::Conformance => {
                let body: orch_link_contract::commands::NodeToolConformance =
                    serde_json::from_value(command.body.clone())?;
                body.name == work.claim.build.kind && body.version == work.claim.build.version
            }
        };
        if !same_build
            || command.key.wire()
                != orch_link_contract::commands::SerialKey::Tool(work.claim.build.kind.clone())
                    .wire()
        {
            return Err(EngineError::unreadable(
                "the check command differs from its claimed native build",
            ));
        }
        if work
            .host
            .as_ref()
            .is_none_or(|host| host.machine != command.machine)
        {
            return Err(EngineError::unreadable(
                "the native check command changed its selected host",
            ));
        }
        Ok(Self {
            work: work.clone(),
            kind,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    /// Exact native command kind, never selected from a client operation claim.
    #[must_use]
    pub fn command_kind(&self) -> &'static str {
        match self.kind {
            CommandKind::Ensure => "node.tool.ensure",
            CommandKind::Interface => "node.tool.check",
            CommandKind::Conformance => "node.tool.conformance",
        }
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != self.command_kind()
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        match self.work.checkpoint(conn, clock).await {
            Ok(Some(checked)) => {
                checked
                    .before
                    .check(clock)
                    .map_err(EngineError::unreadable)?;
                Ok(Some(vec![(Some(checked.caller), None)]))
            }
            Ok(None) => Ok(Some(Vec::new())),
            Err(Error::Refused) => Ok(None),
            Err(error) => Err(EngineError::unreadable(error)),
        }
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}
