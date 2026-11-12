//! Current original-operation frames for native cleanup work and its private command journal.
use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    http_origin::HttpOrigin,
    sources::Facts,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    key_scope::{KeyObjectRef, KeyTargetKind},
    permissions::{NodePermission, PermissionName},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    storage::{StorageCleanupCreate, StorageCleanupPreflight},
    storage_sources::{self as declarations, Source as Role},
};
use orch_authz::{Caller, CapturedOperation};
use orch_types::Clock;
use std::collections::BTreeSet;

const REQUEST: &str = "cleanup_operation";
const JOURNAL: &str = "cleanup_commands";
fn refused() -> Stop {
    failed(
        "access.denied",
        "The original cleanup operation no longer permits this item.",
    )
}
fn unavailable() -> Error {
    Error::Refused
}

/// The actual original credential, retaining its real browser sign-in separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// The accepted canonical operation and immutable caller ceilings.
    pub operation: CapturedOperation,
    /// The actual browser session, absent for a token.
    pub session: Option<Id>,
    /// A saved preview keeps its actual raw transport, independently of a later confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<HttpOrigin>,
}
impl Origin {
    /// Capture only an admitted native caller and its transport's genuine sign-in.
    pub fn capture(caller: &Caller, session: Option<Id>) -> Result<Self, EngineError> {
        if caller.grant().is_some() && session.is_some() {
            return Err(EngineError::unreadable(
                "a cleanup token cannot borrow a browser sign-in",
            ));
        }
        Ok(Self {
            operation: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            session,
            checkpoint: None,
        })
    }
    async fn raw(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        now: Time,
    ) -> Result<Option<Option<Time>>, EngineError> {
        if caller.grant().is_some() {
            return Ok(self.session.is_none().then_some(None));
        }
        let Some(session) = self.session else {
            return Ok(None);
        };
        let state =
            orch_db::queries::tokens::lock_session_state(conn, session, self.operation.user())
                .await?;
        Ok(state
            .filter(|(admin, end)| *end > now.get() && (!caller.as_admin() || *admin))
            .map(|(_, end)| Some(Time::new(end))))
    }
}
/// Immutable server-produced job association; this stores no admission or bearer secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    job: Id,
    origin: Origin,
    preview: Option<Origin>,
    sources: sources::Sources,
    recipe: Json,
}
impl Request {
    /// Retain the newly prepared native job's exact recipe, factual sources and original operations.
    pub fn capture(
        job: Id,
        caller: &Caller,
        session: Option<Id>,
        preview: Option<Origin>,
        sources: sources::Sources,
        recipe: &Recipe,
    ) -> Result<Self, EngineError> {
        let origin = Origin::capture(caller, session)?;
        if origin.operation.operation() != declarations::CREATE.operation
            || preview.as_ref().is_some_and(|preview| {
                preview.operation.operation() != declarations::PREFLIGHT.operation
                    || preview.operation.user() != caller.user()
            })
        {
            return Err(EngineError::unreadable(
                "the native cleanup origins changed",
            ));
        }
        Ok(Self {
            job,
            origin,
            preview,
            sources,
            recipe: serde_json::to_value(recipe)?,
        })
    }
    /// Present malformed provenance is never treated as old ordinary work.
    pub fn from_job(row: &jobs::JobRow) -> Result<Option<Self>, EngineError> {
        row.plan
            .get(REQUEST)
            .map(|value| serde_json::from_value(value.clone()).map_err(EngineError::from))
            .transpose()
    }
    fn matches(&self, row: &jobs::JobRow) -> bool {
        self.job == row.id
            && row.kind == KIND
            && row.created_by == Some(self.origin.operation.user())
            && self.recipe == row.plan["recipe"]
            && self.origin.operation.operation() == declarations::CREATE.operation
    }
}
/// The exact native source input of a current stage, independent of HTTP or worker transport.
#[derive(Clone)]
pub struct Input {
    sources: sources::Sources,
    work: Option<(Request, String)>,
}
/// Current native operands retained across its original-operation lock footprint.
pub struct Value {
    facts: Vec<Facts<Role>>,
    association: Json,
}
/// The confirmed cleanup stage.
pub struct Creating;
/// The saved preview stage, kept independent of confirmation.
pub struct Previewing;

async fn namespace(
    conn: &mut Conn,
    principal: &Principal,
    sources: &sources::Sources,
) -> Result<(), Error> {
    use orch_access_contract::key_scope::KeyIdentityKind;
    let object = sources.namespace.object;
    match (object.kind, object.id) {
        (KeyTargetKind::Identity(KeyIdentityKind::Account), Some(id)) if id == principal.user() => {
        }
        (KeyTargetKind::Identity(KeyIdentityKind::System), None) if principal.is_system_admin() => {
        }
        (KeyTargetKind::Object(kind), Some(id)) if kind.node().is_some() => {
            if orch_authz::nodes::holding(
                conn,
                principal,
                &[id],
                PermissionName::OfNode(NodePermission::View),
                orch_authz::Act::Use,
                Default::default(),
            )
            .await?
            .is_empty()
            {
                return Err(unavailable());
            }
        }
        (KeyTargetKind::Object(kind), Some(id)) => {
            if !resource_facts::authorities(conn, principal, &[id])
                .await?
                .iter()
                .any(|row| row.resource.object() == (ObjectRef { kind, id }) && row.manageable)
            {
                return Err(unavailable());
            }
        }
        _ => return Err(unavailable()),
    }
    Ok(())
}
fn work_of(request: &Request, key: &str) -> Result<Work, Error> {
    let recipe: Recipe =
        serde_json::from_value(request.recipe.clone()).map_err(EngineError::from)?;
    recipe
        .work
        .into_iter()
        .find(|work| work.id == key && work.choice != CleanupChoice::Keep && work.target.is_some())
        .ok_or_else(unavailable)
}
fn may_continue(row: &jobs::JobRow, work: &Work) -> bool {
    row.is_open()
        && (!row.cancel_requested()
            || match work.target.as_ref() {
                Some(Target::Environment { environment, .. }) => {
                    row.plan
                        .get("cleanup_purges")
                        .and_then(|purges| purges.get(environment.to_string()))
                        .and_then(|purge| purge.get("started"))
                        .and_then(Json::as_bool)
                        == Some(true)
                }
                _ => false,
            })
}
async fn association(conn: &mut Conn, input: &Input) -> Result<Json, Error> {
    let Some((request, key)) = &input.work else {
        return Ok(Json::Null);
    };
    let row = jobs::job(conn, request.job)
        .await?
        .filter(|row| request.matches(row))
        .ok_or_else(unavailable)?;
    let work = work_of(request, key)?;
    if !may_continue(&row, &work) {
        return Err(unavailable());
    }
    let target = work.target.as_ref().ok_or_else(unavailable)?;
    let mut stamp = json!({"job":row.id,"work":work,"recipe":request.recipe});
    match target {
        Target::Native { host, resource, .. } => {
            let current = orch_db::queries::resources::hosting::host(conn, *host)
                .await?
                .ok_or_else(unavailable)?;
            let saved: Option<Json> =
                sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
                    .bind(host)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            let inventory =
                saved.and_then(|value| serde_json::from_value::<ResourceInventory>(value).ok());
            if let Some(native) = inventory.as_ref().and_then(|inventory| {
                inventory
                    .resources
                    .iter()
                    .find(|item| item.resource == *resource)
            }) && let Some(expected) = input.sources.associations.get(key)
            {
                if expected.object.kind == KeyTargetKind::Object(ObjectKind::Host)
                    && expected.object.id != Some(*host)
                {
                    return Err(unavailable());
                }
                if expected.object.kind != KeyTargetKind::Object(ObjectKind::Host) {
                    let runtime = native
                        .labels
                        .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                        .and_then(|id| id.parse::<Id>().ok())
                        .ok_or_else(unavailable)?;
                    let owner:Option<(Option<Id>,Option<Id>)>=sqlx::query_as("SELECT environment_id,provider_id FROM runtimes WHERE id=$1 AND host_id=$2").bind(runtime).bind(host).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
                    let owner = owner
                        .and_then(|(environment, provider)| {
                            environment
                                .map(|id| (ObjectKind::Environment, id))
                                .or_else(|| provider.map(|id| (ObjectKind::Provider, id)))
                        })
                        .ok_or_else(unavailable)?;
                    if expected.object
                        != (KeyObjectRef {
                            kind: KeyTargetKind::Object(owner.0),
                            id: Some(owner.1),
                        })
                    {
                        return Err(unavailable());
                    }
                }
            }
            stamp["machine"] = json!(current.machine_id);
        }
        Target::Environment {
            environment,
            revision,
        } => {
            let promise = retained::removal(conn, *environment)
                .await?
                .ok_or_else(unavailable)?;
            if promise.removal_revision != *revision {
                return Err(unavailable());
            }
            let volumes: Vec<retained::Volume> =
                serde_json::from_value(promise.retained_volumes.clone())
                    .map_err(EngineError::from)?;
            for volume in &volumes {
                let host = orch_db::queries::resources::hosting::host(conn, volume.host_id)
                    .await?
                    .ok_or_else(unavailable)?;
                if !input.sources.physical.iter().any(|reference| {
                    reference.family == KeyTargetKind::Object(ObjectKind::Host)
                        && reference.object
                            == (KeyObjectRef {
                                kind: KeyTargetKind::Object(ObjectKind::Host),
                                id: Some(host.id),
                            })
                }) || !input
                    .sources
                    .machines
                    .iter()
                    .any(|(id, _)| *id == host.machine_id)
                {
                    return Err(unavailable());
                }
            }
            stamp["volumes"] = promise.retained_volumes;
        }
        Target::SkillVersion { skill, version } => {
            let parent: Option<Id> =
                sqlx::query_scalar("SELECT skill_id FROM skill_versions WHERE id=$1")
                    .bind(version)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            if parent.is_some_and(|parent| parent != *skill) {
                return Err(unavailable());
            }
            stamp["parent"] = json!(parent);
        }
        Target::MountData { mount, kept } => {
            let data = orch_db::queries::resources::mounts::kept(conn, &[*mount])
                .await?
                .into_iter()
                .find(|row| row.id == *kept);
            if let Some(data) = data {
                if !input
                    .sources
                    .machines
                    .iter()
                    .any(|(id, _)| *id == data.machine_id)
                {
                    return Err(unavailable());
                }
                stamp["location"] = json!([
                    data.machine_id.to_string(),
                    data.source,
                    data.storage.unwrap_or_default()
                ]);
            }
        }
    }
    Ok(stamp)
}
async fn prepare(conn: &mut Conn, input: &Input) -> Result<(Value, Vec<Facts<Role>>), Error> {
    let facts = input
        .sources
        .required(conn)
        .await?
        .ok_or_else(unavailable)?;
    let association = association(conn, input).await?;
    Ok((
        Value {
            facts: facts.clone(),
            association,
        },
        facts,
    ))
}
async fn require(conn: &mut Conn, principal: &Principal, input: &Input) -> Result<(), Error> {
    namespace(conn, principal, &input.sources).await?;
    if let Some((request, key)) = &input.work {
        if request.origin.operation.user() != principal.user() {
            return Err(unavailable());
        }
        let work = work_of(request, key)?;
        require_work(conn, principal, &work).await?;
    }
    Ok(())
}
async fn require_work(conn: &mut Conn, principal: &Principal, work: &Work) -> Result<(), Error> {
    if let Some(Target::Environment {
        environment,
        revision,
    }) = work.target.as_ref()
    {
        let (manager, admin) =
            super::super::retained::authority(conn, principal, *environment).await?;
        let promise = retained::removal(conn, *environment)
            .await?
            .ok_or_else(unavailable)?;
        if promise.removal_revision != *revision
            || !admin
                && !(manager
                    && promise
                        .volumes_purge_at
                        .is_some_and(|until| until <= principal.now().get()))
        {
            return Err(unavailable());
        }
    }
    if let Some(Target::MountData { mount, kept }) = work.target.as_ref()
        && let Some(data) = orch_db::queries::resources::mounts::kept(conn, &[*mount])
            .await?
            .into_iter()
            .find(|data| data.id == *kept)
    {
        use orch_authz::resources::{Locating, LocationAsk, LocationKind};
        let ask = LocationAsk {
            mount: *mount,
            machine: data.machine_id,
            kind: if data.kind == "volume" {
                LocationKind::Volume
            } else {
                LocationKind::Path
            },
            storage: data.storage,
        };
        if !matches!(
            orch_authz::resources::may_add_location(conn, principal, &ask, &Default::default())
                .await?,
            Locating::Allowed
        ) {
            return Err(unavailable());
        }
    }
    let object = match work.target.clone().ok_or_else(unavailable)? {
        Target::Native { host, .. } => ObjectRef {
            kind: ObjectKind::Host,
            id: host,
        },
        Target::Environment { environment, .. } => ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        Target::SkillVersion { skill, .. } => ObjectRef {
            kind: ObjectKind::Skill,
            id: skill,
        },
        Target::MountData { mount, .. } => ObjectRef {
            kind: ObjectKind::Mount,
            id: mount,
        },
    };
    if !resource_facts::authorities(conn, principal, &[object.id])
        .await?
        .iter()
        .any(|row| row.resource.object() == object && row.manageable)
    {
        return Err(unavailable());
    }
    Ok(())
}

async fn locks(conn: &mut Conn, input: &Input, value: &Value) -> Result<(), Error> {
    let nodes = value
        .facts
        .iter()
        .flat_map(|fact| &fact.groups)
        .flat_map(|(_, targets)| targets)
        .flat_map(|target| target.node_ancestors.iter().filter_map(|object| object.id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(nodes)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let hosts = std::iter::once(&input.sources.namespace)
        .chain(&input.sources.records)
        .chain(&input.sources.physical)
        .filter_map(|reference| {
            (reference.object.kind == KeyTargetKind::Object(ObjectKind::Host))
                .then_some(reference.object.id)
                .flatten()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let host_lock = if input.work.is_some() {
        "SELECT id FROM hosts WHERE id=ANY($1) ORDER BY id FOR UPDATE"
    } else {
        "SELECT id FROM hosts WHERE id=ANY($1) ORDER BY id FOR SHARE"
    };
    sqlx::query(host_lock)
        .bind(hosts)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let machines = input
        .sources
        .machines
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(machines)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if let Some((request, key)) = &input.work {
        match work_of(request, key)?.target.ok_or_else(unavailable)? {
            Target::Native { host, .. } => {
                sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                    .bind(host)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                if let Some((request, key)) = &input.work {
                    let work = work_of(request, key)?;
                    if let Some(Target::Native { resource, .. }) = work.target {
                        let saved: Option<Json> =
                            sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
                                .bind(host)
                                .fetch_one(&mut *conn)
                                .await
                                .map_err(DbError::from)?;
                        let runtime = saved
                            .and_then(|value| {
                                serde_json::from_value::<ResourceInventory>(value).ok()
                            })
                            .and_then(|inventory| {
                                inventory
                                    .resources
                                    .into_iter()
                                    .find(|item| item.resource == resource)
                            })
                            .and_then(|item| {
                                item.labels
                                    .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                                    .and_then(|id| id.parse::<Id>().ok())
                            });
                        if let Some(runtime) = runtime {
                            sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
                                .bind(runtime)
                                .fetch_optional(&mut *conn)
                                .await
                                .map_err(DbError::from)?;
                        }
                    }
                }
            }
            Target::Environment { environment, .. } => {
                hosting::lock_environment(conn, environment).await?;
            }
            Target::SkillVersion { skill, version } => {
                sqlx::query("SELECT id FROM skills WHERE id=$1 FOR UPDATE")
                    .bind(skill)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                sqlx::query("SELECT id FROM skill_versions WHERE id=$1 FOR UPDATE")
                    .bind(version)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            Target::MountData { kept, .. } => {
                sqlx::query("SELECT id FROM mount_kept_data WHERE id=$1 FOR SHARE")
                    .bind(kept)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
        }
    }
    Ok(())
}
macro_rules! stage {
    ($kind:ty,$route:ty,$row:ident) => {
        impl Source for $kind {
            type Role = Role;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = <$route>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &declarations::$row;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a Input,
                _: Time,
            ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
                Box::pin(prepare(conn, input))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a Input,
                _: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, input))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                input: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(locks(conn, input, value))
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a Input,
                old: &'a Value,
                fresh: &'a Value,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move { Ok(old.association == fresh.association) })
            }
        }
    };
}
stage!(Creating, StorageCleanupCreate, CREATE);
stage!(Previewing, StorageCleanupPreflight, PREFLIGHT);

/// Live raw credential minimum retained only until the owner's actual rollbackable commit.
pub struct Deadline {
    callers: Vec<Caller>,
    raw: Vec<Option<Time>>,
}
impl Deadline {
    /// Recheck every actual retained key/sign-in minimum after the native writer's waits.
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if self
            .callers
            .iter()
            .all(|caller| caller.key_deadline_allows(clock))
            && self
                .raw
                .iter()
                .all(|until| until.is_none_or(|until| until > clock.time()))
        {
            Ok(())
        } else {
            Err(EngineError::unreadable(
                "the original cleanup credential ended",
            ))
        }
    }
}
/// Validate a saved preview's own canonical operation before confirming it under another operation.
pub async fn check_preview(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &Origin,
    sources: &sources::Sources,
) -> Result<Deadline, EngineError> {
    let checkpoint = origin.checkpoint.as_ref().ok_or_else(|| {
        EngineError::unreadable("the saved cleanup preview has no original HTTP checkpoint")
    })?;
    if checkpoint.origin() != &origin.operation || checkpoint.session() != origin.session {
        return Err(EngineError::unreadable(
            "the saved cleanup preview's original transport changed",
        ));
    }
    let frame = continuing::checkpoint::<Previewing>(
        conn,
        clock,
        checkpoint,
        Input {
            sources: sources.clone(),
            work: None,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    let raw = frame
        .http_origin()
        .ok_or_else(|| EngineError::unreadable("the cleanup preview lost its HTTP checkpoint"))?
        .until();
    let parts = frame.into_parts();
    let callers = vec![parts.principal.caller().clone()];
    parts
        .first_effect
        .check(clock)
        .map_err(EngineError::unreadable)?;
    Ok(Deadline {
        callers,
        raw: vec![raw],
    })
}
async fn legacy(
    conn: &mut Conn,
    row: &jobs::JobRow,
    now: Time,
) -> Result<(Principal, Option<Time>), Stop> {
    let user = row.created_by.ok_or_else(refused)?;
    let raw = if let Some(token) = row
        .plan
        .pointer("/identity/token_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    {
        let state:Option<(bool,Option<chrono::DateTime<chrono::Utc>>)>=sqlx::query_as("SELECT operation_scope IS NOT NULL,expires_at FROM api_tokens WHERE id=$1 AND user_id=$2 FOR SHARE").bind(token).bind(user).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let (modern, until) = state.ok_or_else(refused)?;
        if modern || until.is_some_and(|until| until <= now.get()) {
            return Err(refused());
        }
        until.map(Time::new)
    } else {
        let session = row
            .plan
            .pointer("/identity/session_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
            .ok_or_else(refused)?;
        let as_admin = row
            .plan
            .get("origin")
            .cloned()
            .and_then(|origin| serde_json::from_value::<orch_db::audit::Origin>(origin).ok())
            .is_some_and(|origin| origin.as_admin == Some(user));
        let (_, until) = orch_db::queries::tokens::lock_session_state(conn, session, user)
            .await?
            .filter(|(admin, until)| *until > now.get() && (!as_admin || *admin))
            .ok_or_else(refused)?;
        Some(Time::new(until))
    };
    let principal = super::super::retained::requester(conn, row, now.get()).await?;
    Ok((principal, raw))
}

/// The current original requester; old jobs keep their native path only after proving the token is legacy.
pub(super) async fn principal(conn: &mut Conn, job: &Job, key: &str) -> Result<Principal, Stop> {
    let row = jobs::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    let Some(request) = Request::from_job(&row)? else {
        return Ok(legacy(conn, &row, Time::new(job.now())).await?.0);
    };
    let (frame, _) = frame(conn, job.engine().inner.clock.as_ref(), &row, &request, key).await?;
    Ok(frame.into_parts().principal)
}
async fn frame(
    conn: &mut Conn,
    clock: &dyn Clock,
    row: &jobs::JobRow,
    request: &Request,
    key: &str,
) -> Result<(continuing::Frame<Creating>, Option<Time>), Stop> {
    if !request.matches(row) {
        return Err(refused());
    }
    if let Some(preview) = &request.preview {
        check_preview(conn, clock, preview, &request.sources).await?;
    }
    let frame = continuing::prepare::<Creating>(
        conn,
        clock,
        &request.origin.operation,
        Input {
            sources: request.sources.clone(),
            work: Some((request.clone(), key.into())),
        },
    )
    .await
    .map_err(|_| refused())?;
    let raw = request
        .origin
        .raw(conn, frame.principal().caller(), clock.time())
        .await?
        .ok_or_else(refused)?;
    frame.check_deadline(clock).map_err(|_| refused())?;
    Ok((frame, raw))
}
/// Consume the actual original-operation first-effect cut on the native writer's still-open transaction.
pub(super) async fn before_mutation(
    conn: &mut Conn,
    job: &Job,
    key: &str,
) -> Result<Option<Deadline>, Stop> {
    let row = jobs::job(conn, job.id()).await?.ok_or(Stop::Lost)?;
    let Some(request) = Request::from_job(&row)? else {
        let (principal, raw) = legacy(conn, &row, Time::new(job.now())).await?;
        return Ok(Some(Deadline {
            callers: vec![principal.caller().clone()],
            raw: vec![raw],
        }));
    };
    let clock = job.engine().inner.clock.as_ref();
    let (frame, raw) = frame(conn, clock, &row, &request, key).await?;
    let preview = match &request.preview {
        Some(origin) => Some(check_preview(conn, clock, origin, &request.sources).await?),
        None => None,
    };
    let parts = frame.into_parts();
    let mut callers = vec![parts.principal.caller().clone()];
    let mut raw = vec![raw];
    if let Some(preview) = preview {
        callers.extend(preview.callers);
        raw.extend(preview.raw);
    }
    parts.first_effect.check(clock).map_err(|_| refused())?;
    Ok(Some(Deadline { callers, raw }))
}

fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    crate::jobs::command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
}
fn stamp(request: &Request) -> Result<orch_types::Hash, EngineError> {
    Ok(orch_types::Hash::of(&serde_json::to_vec(request)?))
}
async fn expected(conn: &mut Conn, job: Id, work: &Work) -> Result<Vec<Outgoing>, Stop> {
    let mut commands = Vec::new();
    match work.target.as_ref().ok_or_else(refused)? {
        Target::Native {
            host,
            resource,
            last_used_at,
        } => {
            let host = orch_db::queries::resources::hosting::host(conn, *host)
                .await?
                .ok_or_else(refused)?;
            commands.push(
                Outgoing::new(
                    ids::derived(
                        "storage.cleanup.inventory",
                        &[job.as_bytes(), work.id.as_bytes()],
                    ),
                    host.machine_id,
                    SerialKey::Machine,
                    &NodeResourcesList {},
                )
                .map_err(EngineError::from)?,
            );
            commands.push(
                Outgoing::new(
                    ids::derived(
                        "storage.cleanup.remove",
                        &[job.as_bytes(), work.id.as_bytes()],
                    ),
                    host.machine_id,
                    SerialKey::Machine,
                    &NodeResourcesRemove {
                        kind: resource.kind,
                        id: resource.id.clone(),
                        stop_first: work.choice == CleanupChoice::StopAndRemove,
                        expected_last_used_at: if work.choice == CleanupChoice::StopAndRemove {
                            None
                        } else {
                            *last_used_at
                        },
                    },
                )
                .map_err(EngineError::from)?,
            );
        }
        Target::Environment { environment, .. } => {
            let promise = retained::removal(conn, *environment)
                .await?
                .ok_or_else(refused)?;
            let volumes: Vec<retained::Volume> =
                serde_json::from_value(promise.retained_volumes).map_err(EngineError::from)?;
            if let Some(volume) = volumes.first() {
                let host = orch_db::queries::resources::hosting::host(conn, volume.host_id)
                    .await?
                    .ok_or_else(refused)?;
                commands.push(
                    Outgoing::new(
                        ids::derived(
                            "storage.cleanup.environment",
                            &[job.as_bytes(), work.id.as_bytes()],
                        ),
                        host.machine_id,
                        SerialKey::Runtime(volume.runtime_id),
                        &NodeRuntimeRemove {
                            runtime_id: volume.runtime_id,
                            keep_volumes: false,
                            volume_refs: serde_json::from_value(json!(volumes))
                                .map_err(EngineError::from)?,
                        },
                    )
                    .map_err(EngineError::from)?,
                );
            }
        }
        Target::MountData { mount, kept } => {
            if let Some(data) = orch_db::queries::resources::mounts::kept(conn, &[*mount])
                .await?
                .into_iter()
                .find(|data| data.id == *kept)
                && data.kind == "volume"
            {
                let driver =
                    orch_db::queries::resources::mounts::data_driver(conn, data.machine_id)
                        .await?
                        .ok_or_else(refused)?
                        .0;
                commands.push(
                    Outgoing::new(
                        ids::derived("storage.cleanup.mount", &[job.as_bytes(), kept.as_bytes()]),
                        data.machine_id,
                        SerialKey::Volume(data.source.clone()),
                        &orch_link_contract::commands::NodeVolumeRemove {
                            driver,
                            name: data.source,
                            storage: data.storage,
                            labels: std::collections::BTreeMap::from([(
                                "orch.mount_id".into(),
                                mount.to_string(),
                            )]),
                        },
                    )
                    .map_err(EngineError::from)?,
                );
            }
        }
        Target::SkillVersion { .. } => {}
    }
    Ok(commands)
}
/// Only the actual immutable job/work/command association is stored in the private authority channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    work: String,
    request: Option<orch_types::Hash>,
    canonical: Json,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CleanupCommand([private])")
    }
}
impl Command {
    /// The command kind is supplied by the native creator, never by an external scope claim.
    pub fn command_kind(&self) -> &'static str {
        match self.canonical.get("kind").and_then(Json::as_str) {
            Some("node.resources.list") => "node.resources.list",
            Some("node.resources.remove") => "node.resources.remove",
            Some("node.runtime.remove") => "node.runtime.remove",
            Some("node.volume.remove") => "node.volume.remove",
            _ => "unknown",
        }
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if crate::jobs::command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? != self.canonical
        {
            return Ok(None);
        }
        let Some(row) = jobs::job(conn, self.job).await? else {
            return Ok(None);
        };
        let request = Request::from_job(&row)?;
        if row
            .plan
            .get(JOURNAL)
            .and_then(|journal| journal.get(command.id.to_string()))
            .is_none_or(|entry| entry["canonical"] != self.canonical)
        {
            return Ok(None);
        }
        let Some(request) = request else {
            if self.request.is_some() {
                return Ok(None);
            }
            let (principal, raw) = match legacy(conn, &row, clock.time()).await {
                Ok(value) => value,
                Err(_) => return Ok(None),
            };
            let recipe: Recipe = serde_json::from_value(row.plan["recipe"].clone())?;
            let Some(work) = recipe.work.iter().find(|work| {
                work.id == self.work && work.choice != CleanupChoice::Keep && work.target.is_some()
            }) else {
                return Ok(None);
            };
            if !may_continue(&row, work) {
                return Ok(None);
            }
            if require_work(conn, &principal, work).await.is_err() {
                return Ok(None);
            }
            let candidates = match expected(conn, row.id, work).await {
                Ok(value) => value,
                Err(_) => return Ok(None),
            };
            if !candidates
                .iter()
                .any(|expected| canonical(expected).is_ok_and(|value| value == self.canonical))
                || !principal.caller().key_deadline_allows(clock)
                || raw.is_some_and(|until| until <= clock.time())
            {
                return Ok(None);
            }
            return Ok(Some(vec![(Some(principal.caller().clone()), raw)]));
        };
        if Some(stamp(&request)?) != self.request {
            return Ok(None);
        }
        let (frame, raw) = match frame(conn, clock, &row, &request, &self.work).await {
            Ok(frame) => frame,
            Err(_) => return Ok(None),
        };
        let work = work_of(&request, &self.work).map_err(EngineError::unreadable)?;
        let candidates = match expected(conn, row.id, &work).await {
            Ok(candidates) => candidates,
            Err(_) => return Ok(None),
        };
        if !candidates
            .iter()
            .any(|expected| canonical(expected).is_ok_and(|value| value == self.canonical))
        {
            return Ok(None);
        }
        let mut callers = Vec::new();
        if let Some(preview) = &request.preview {
            let preview = match check_preview(conn, clock, preview, &request.sources).await {
                Ok(preview) => preview,
                Err(_) => return Ok(None),
            };
            callers.extend(
                preview
                    .callers
                    .into_iter()
                    .zip(preview.raw)
                    .map(|(caller, raw)| (Some(caller), raw)),
            );
        }
        let parts = frame.into_parts();
        parts
            .first_effect
            .check(clock)
            .map_err(EngineError::unreadable)?;
        callers.push((Some(parts.principal.caller().clone()), raw));
        Ok(Some(callers))
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
/// The existing native producer supplies its exact command once; queue and delivery restore current authority again.
pub(super) async fn command(
    job: &Job,
    work: &Work,
    mut command: Outgoing,
) -> Result<Outgoing, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let request = Request::from_job(&row)?;
    let saved: Recipe =
        serde_json::from_value(row.plan["recipe"].clone()).map_err(EngineError::from)?;
    if request
        .as_ref()
        .is_some_and(|request| !request.matches(&row))
        || !saved.work.iter().any(|saved| json!(saved) == json!(work))
    {
        return Err(refused());
    }
    if request.is_none() {
        legacy(&mut tx, &row, Time::new(job.now())).await?;
    }
    let value = canonical(&command)?;
    if let Some(saved) = row
        .plan
        .get(JOURNAL)
        .and_then(|journal| journal.get(command.id.to_string()))
    {
        if saved["canonical"] != value {
            return Err(refused());
        }
    } else {
        let candidates = expected(&mut tx, row.id, work).await?;
        if !candidates
            .iter()
            .any(|expected| canonical(expected).is_ok_and(|expected| expected == value))
        {
            return Err(refused());
        }
    }
    let authority = Command {
        job: job.id(),
        work: work.id.clone(),
        request: request.as_ref().map(stamp).transpose()?,
        canonical: value.clone(),
    };
    command.authority = Some(crate::commands::CommandAuthority::cleanup(authority));
    let mut journal = row
        .plan
        .get(JOURNAL)
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    if !journal.contains_key(&command.id.to_string()) {
        journal.insert(
            command.id.to_string(),
            json!({"canonical":value,"outgoing":command,"work":work.id}),
        );
        jobs::merge_plan(&mut tx, job.id(), &json!({JOURNAL:journal}), job.now()).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(command)
}
/// An accepted exact native receipt can finish without authorizing a second forward act.
pub(super) async fn reconcile(job: &Job, work: &Work) -> Result<bool, Stop> {
    use crate::commands::{Outcome as Ended, Stage};
    let row = job.row().await?;
    let request = Request::from_job(&row)?;
    if request
        .as_ref()
        .is_some_and(|request| !request.matches(&row))
    {
        return Err(refused());
    }
    let request_stamp = request.as_ref().map(stamp).transpose()?;
    if row
        .plan
        .get("cleanup_sql_receipts")
        .and_then(|receipts| receipts.get(&work.id))
        .and_then(Json::as_bool)
        == Some(true)
    {
        return Ok(true);
    }
    for entry in row
        .plan
        .get(JOURNAL)
        .and_then(Json::as_object)
        .into_iter()
        .flat_map(|journal| journal.values())
    {
        if entry["work"] != json!(work.id) {
            continue;
        }
        let command: Outgoing =
            serde_json::from_value(entry["outgoing"].clone()).map_err(EngineError::from)?;
        if command.kind == orch_link_contract::commands::CommandKind::NodeResourcesList {
            continue;
        }
        let Some(crate::commands::CommandAuthority::Cleanup(authority)) = &command.authority else {
            return Err(refused());
        };
        if authority.request != request_stamp
            || authority.canonical != entry["canonical"]
            || canonical(&command)? != authority.canonical
        {
            return Err(refused());
        }
        let native = orch_db::queries::link::command_with_authority(
            &mut *job.engine().db().acquire().await?,
            command.id,
        )
        .await?;
        if let Some(native) = native {
            let evidence = native
                .authority
                .map(serde_json::from_value::<crate::commands::CommandAuthority>)
                .transpose()
                .map_err(EngineError::from)?;
            if evidence != command.authority
                || crate::jobs::command_identity::canonical(
                    native.command.id,
                    native.command.machine_id,
                    native.command.serial_key(),
                    &native.command.kind,
                    &native.command.body,
                )? != authority.canonical
            {
                return Err(refused());
            }
        } else {
            if request.is_some() {
                continue;
            }
            let retained = job
                .engine()
                .inner
                .commands
                .retained_command(command.id)
                .await
                .map_err(|error| failed("command", error.to_string()))?;
            let Some(retained) = retained else {
                continue;
            };
            if crate::jobs::command_identity::canonical(
                command.id,
                retained.machine,
                retained.key.as_deref(),
                &retained.kind,
                &retained.body,
            )? != authority.canonical
            {
                return Err(refused());
            }
        }
        let status = job
            .engine()
            .inner
            .commands
            .status(command.id)
            .await
            .map_err(|error| failed("command", error.to_string()))?;
        let Some(status) = status else {
            continue;
        };
        if matches!(status.outcome, Some(Ended::Succeeded(_))) {
            return Ok(true);
        }
        if status.stage == Stage::Accepted {
            job.command_to_undo(command).await?;
            return Ok(true);
        }
    }
    Ok(false)
}
/// Commit an already accepted native removal's original database bookkeeping only.
pub(super) async fn settle_receipt(job: &Job, work: &Work) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    match work.target.as_ref() {
        Some(Target::Environment {
            environment,
            revision,
        }) => {
            retained::purged(&mut tx, *environment, *revision, job.now()).await?;
        }
        Some(Target::MountData { kept, .. }) => {
            orch_db::queries::resources::mounts::forget_kept(&mut tx, *kept).await?;
        }
        _ => {}
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
