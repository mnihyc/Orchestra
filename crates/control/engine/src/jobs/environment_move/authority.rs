//! Original node move confirmation identity retained by each actual native lifecycle job.

use super::*;
use crate::operation_scope::{
    request_origin::{self, Credential},
    sources::{self, Facts},
    targets,
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    environment::NodeEnvironmentMoveCreate,
    move_batch_sources::{CREATE, Source},
    routes::Endpoint,
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_types::Clock;
use std::collections::{BTreeMap, BTreeSet};

/// A present but invalid source always refuses the forward act.
pub const SOURCE: &str = "node_move_authority";
/// A known source never becomes an ordinary caller when one private metadata field is lost.
pub fn present(row: &jobs::JobRow) -> bool {
    [SOURCE, "lifecycle_owner", "lifecycle_commands"]
        .iter()
        .any(|field| row.plan.get(*field).is_some())
}
/// Real original confirmation and raw authenticator, without a fabricated browser session.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<orch_types::Time>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_http"
    )]
    http: Option<crate::operation_scope::http_origin::HttpOrigin>,
    node: Id,
}
impl Origin {
    /// Capture only this actual admitted native confirmation.
    pub fn capture(
        caller: &Caller,
        credential: Credential,
        until: Option<orch_types::Time>,
        node: Id,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(CREATE.operation) || !credential.matches(caller) {
            return Err(ended("the node move identity does not match"));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(ended)?,
            credential,
            until,
            http: None,
            node,
        })
    }
}
fn present_http<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<crate::operation_scope::http_origin::HttpOrigin>, D::Error> {
    crate::operation_scope::http_origin::HttpOrigin::deserialize(d).map(Some)
}
impl Origin {
    /// Retain the actual common HTTP identity and every accepted native preparation minimum.
    pub fn from_http(
        http: crate::operation_scope::http_origin::HttpOrigin,
        node: Id,
    ) -> Result<Self, EngineError> {
        if http.origin().operation() != CREATE.operation {
            return Err(EngineError::unreadable(
                "The native confirmation source changed its operation.",
            ));
        }
        Ok(Self {
            origin: http.origin().clone(),
            credential: http.credential().clone(),
            until: http.until(),
            node,
            http: Some(http),
        })
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(Caller, Option<orch_types::Time>), EngineError> {
        match &self.http {
            Some(http) => {
                let current = http.current(conn, clock).await.map_err(source_error)?;
                Ok((current.caller().clone(), current.until()))
            }
            None => request_origin::current(
                conn,
                clock,
                &self.origin,
                &self.credential,
                self.until,
                CREATE.operation,
            )
            .await
            .map_err(source_error),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    origin: Origin,
    job: Id,
    batch: Id,
    environment: Id,
    owner: Id,
    home: Id,
    generation: i32,
    machine: Option<Id>,
    runtime: Option<Id>,
    recipe: Hash,
}
#[derive(Debug, thiserror::Error)]
#[error("the original native lifecycle source is closed: {0}")]
pub(crate) struct Ended(String);
fn ended(error: impl std::fmt::Display) -> EngineError {
    EngineError::BeforeEffect(Box::new(Ended(error.to_string())))
}
fn source_error(error: crate::operation_scope::continuing::Error) -> EngineError {
    match error {
        crate::operation_scope::continuing::Error::Native(error) => error,
        error => ended(error),
    }
}

/// The original native node confirmation's actual retained HTTP source.
pub(crate) fn retained_http(
    row: &jobs::JobRow,
) -> Result<crate::operation_scope::http_origin::HttpOrigin, EngineError> {
    let binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| ended("the node source disappeared"))?,
    )
    .map_err(|_| ended("the node source is invalid"))?;
    binding
        .origin
        .http
        .ok_or_else(|| ended("the node lifecycle needs its actual HTTP source"))
}
type NativePlan = (Id, Id, serde_json::Value, BTreeSet<(ObjectKind, Id)>);

fn native(row: &jobs::JobRow) -> Result<NativePlan, EngineError> {
    if let Some(value) = row.plan.get("portable") {
        let recipe: super::super::machine_move::Recipe = serde_json::from_value(value.clone())?;
        let destinations = recipe
            .destination_access()
            .into_iter()
            .map(|(object, _)| (object.kind, object.id))
            .collect();
        Ok((
            recipe.environment,
            recipe.by,
            serde_json::to_value(recipe)?,
            destinations,
        ))
    } else {
        let recipe: MoveRecipe = serde_json::from_value(row.plan.clone())?;
        let destinations = [
            (ObjectKind::Host, recipe.destination),
            (ObjectKind::Template, recipe.replacement.template),
        ]
        .into_iter()
        .collect();
        Ok((
            recipe.replacement.target.id(),
            recipe.replacement.by,
            serde_json::to_value(recipe)?,
            destinations,
        ))
    }
}
async fn admit(
    conn: &mut Conn,
    caller: &Caller,
    environment: Id,
    destinations: &BTreeSet<(ObjectKind, Id)>,
) -> Result<Caller, EngineError> {
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(ended)?;
    let mut selected = BTreeMap::new();
    for (kind, id) in destinations {
        let target = match kind {
            ObjectKind::Machine => targets::machine_subject(conn, *id).await.map_err(ended)?,
            _ => targets::resource_identity(
                conn,
                ObjectRef {
                    kind: *kind,
                    id: *id,
                },
            )
            .await
            .map_err(ended)?,
        };
        selected
            .entry(KeyTargetKind::Object(*kind))
            .or_insert_with(Vec::new)
            .push(target);
    }
    let facts = vec![
        Facts::collection(
            Source::Environments,
            vec![(KeyTargetKind::Object(ObjectKind::Environment), vec![target])],
        ),
        Facts::collection(Source::Destinations, selected.into_iter().collect()),
    ];
    sources::bind_required(NodeEnvironmentMoveCreate::ROUTE, &CREATE, &facts)
        .map_err(ended)?
        .admit(caller, false)
        .map_err(ended)
}
/// Only a new native job on its original confirmation transaction may retain the source.
pub async fn retain(
    conn: &mut Conn,
    row: &jobs::JobRow,
    batch: Id,
    origin: &Origin,
    accepted: &Caller,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let (environment, by, recipe, destinations) = native(row)?;
    let made: bool =
        sqlx::query_scalar("SELECT xmin=pg_current_xact_id()::xid FROM jobs WHERE id=$1")
            .bind(row.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if !made
        || row.kind != "environment_move"
        || row.batch_id != Some(batch)
        || row.environment_id != Some(environment)
        || row.state != "queued"
        || by != origin.origin.user()
        || row.created_by != Some(by)
        || row.plan.get(SOURCE).is_some()
    {
        return Err(ended(
            "the original confirmation does not own this new move",
        ));
    }
    if !envs::environments_under(conn, origin.node)
        .await?
        .contains(&environment)
    {
        return Err(ended("the reviewed node no longer selects the move"));
    }
    let state = envs::environment(conn, environment)
        .await?
        .ok_or_else(|| ended("the accepted move source disappeared"))?;
    let runtime = envs::live_runtime(conn, environment)
        .await?
        .map(|runtime| runtime.id);
    let (caller, until) = origin.current(conn, clock).await?;
    let caller = caller.intersect_restrictions(accepted).map_err(ended)?;
    let caller = admit(conn, &caller, environment, &destinations).await?;
    let mut origin = origin.clone();
    origin.origin = CapturedOperation::capture(&caller).map_err(ended)?;
    origin.until = until;
    if let Some(http) = &origin.http {
        origin.http = Some(http.accepted(&caller, until).map_err(source_error)?);
    }
    let binding = Binding {
        origin,
        job: row.id,
        batch,
        environment,
        owner: state.owner_id,
        home: state.home_node_id,
        generation: state.generation,
        machine: state.machine_id,
        runtime,
        recipe: crate::jobs::command_identity::hash(&recipe)?,
    };
    jobs::merge_plan(
        conn,
        row.id,
        &json!({SOURCE:binding,"lifecycle_owner":"node_move","lifecycle_http_required":binding.origin.http.is_some()}),
        clock.now(),
    )
    .await?;
    Ok(())
}
/// Restore the same source, scopes, current ordinary prerequisites and immutable native association.
pub async fn principal(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        use sqlx::Connection as _;
        let mut tx = conn.begin().await.map_err(orch_db::DbError::from)?;
        let principal = principal_in(&mut tx, row, clock).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(principal);
    }
    principal_in(conn, row, clock).await
}
async fn principal_in(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Principal, EngineError> {
    let mut binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| ended("the known node move source disappeared"))?,
    )?;
    if row.plan.get("lifecycle_http_required") == Some(&json!(true))
        && binding.origin.http.is_none()
    {
        return Err(ended("The original HTTP source disappeared."));
    }
    let (environment, by, recipe, destinations) = native(row)?;
    if row.kind != "environment_move"
        || row.id != binding.job
        || row.batch_id != Some(binding.batch)
        || row.environment_id != Some(environment)
        || environment != binding.environment
        || by != binding.origin.origin.user()
        || row.created_by != Some(by)
        || crate::jobs::command_identity::hash(&recipe)? != binding.recipe
    {
        return Err(ended("the native move/source association changed"));
    }
    let (caller, until) = binding.origin.current(conn, clock).await?;
    let state = envs::lock_environment(conn, environment)
        .await?
        .ok_or_else(|| ended("the native move source disappeared"))?;
    if state.owner_id != binding.owner
        || state.home_node_id != binding.home
        || state.generation != binding.generation
        || state.machine_id != binding.machine
        || envs::live_runtime(conn, environment)
            .await?
            .map(|runtime| runtime.id)
            != binding.runtime
        || !envs::environments_under(conn, binding.origin.node)
            .await?
            .contains(&environment)
    {
        return Err(ended("the native move source changed"));
    }
    let caller = admit(conn, &caller, environment, &destinations).await?;
    binding.origin.origin = CapturedOperation::capture(&caller).map_err(ended)?;
    binding.origin.until = until;
    if let Some(http) = &binding.origin.http {
        binding.origin.http = Some(http.accepted(&caller, until).map_err(source_error)?);
    }
    let principal = Principal::load(conn, caller, clock.time()).await?;
    if !crate::updates::may_manage(
        conn,
        &principal,
        crate::updates::Target::Environment(environment),
    )
    .await?
    {
        return Err(ended("the original move no longer manages the environment"));
    }
    for (kind, id) in destinations {
        let permission = match kind {
            ObjectKind::Host => Permission::HostUse,
            ObjectKind::Machine => Permission::MachineUse,
            ObjectKind::Template => Permission::TemplateUse,
            _ => return Err(ended("invalid native move destination")),
        };
        if !orch_authz::resources::check(
            conn,
            &principal,
            ObjectRef { kind, id },
            permission,
            Policy::default(),
        )
        .await?
        .is_allowed()
        {
            return Err(ended("the original move no longer has destination use"));
        }
    }
    let narrowed = serde_json::to_value(binding)?;
    if row.plan.get(SOURCE) != Some(&narrowed) {
        jobs::merge_plan(conn, row.id, &json!({SOURCE:narrowed}), clock.now()).await?;
    }
    Ok(principal)
}
/// Reuse the native phase boundary; compensation and already committed cleanup keep their own authority.
pub(crate) async fn guard(job: &Job) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    if present(&row) && row.plan.get("committed").and_then(Json::as_bool) != Some(true) {
        principal(&mut tx, &row, job.engine().inner.clock.as_ref())
            .await
            .map_err(|error| fail("access.denied", error.to_string()))?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
    }
    Ok(())
}

/// Earliest retained real credential expiry, independent of a native review's expiry.
pub(crate) fn deadline(row: &jobs::JobRow) -> Result<Option<orch_types::Time>, EngineError> {
    let binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| EngineError::unreadable("the native source disappeared"))?,
    )?;
    Ok(binding.origin.until)
}

/// A committed move owns only its old unit stop or old registration detach during cleanup.
/// This is accepted cleanup of exact native material, independent of withdrawn forward authority.
pub(crate) async fn cleanup_matches(
    conn: &mut Conn,
    row: &jobs::JobRow,
    machine: Id,
    key: Option<&str>,
    kind: &str,
    body: &Json,
) -> Result<bool, EngineError> {
    let binding: Binding = serde_json::from_value(
        row.plan
            .get(SOURCE)
            .cloned()
            .ok_or_else(|| ended("the cleanup source disappeared"))?,
    )?;
    let (environment, by, recipe, _) = native(row)?;
    if row.kind != "environment_move"
        || row.id != binding.job
        || row.batch_id != Some(binding.batch)
        || row.environment_id != Some(environment)
        || row.created_by != Some(by)
        || by != binding.origin.origin.user()
        || environment != binding.environment
        || crate::jobs::command_identity::hash(&recipe)? != binding.recipe
        || row.plan.get("committed").and_then(Json::as_bool) != Some(true)
        || crate::jobs::runner::read_progress(row).0.phase != "cleanup"
    {
        return Ok(false);
    }
    let old = if let Some(portable) = row.plan.get("portable") {
        let recipe: super::super::machine_move::Recipe = serde_json::from_value(portable.clone())?;
        if recipe.old_runtime().is_none() {
            return Ok(kind == "env.detach"
                && machine == recipe.source_machine
                && key
                    == orch_link_contract::commands::SerialKey::Environment(environment)
                        .wire()
                        .as_deref()
                && body.get("environment_id") == Some(&json!(environment)));
        }
        recipe.old_runtime()
    } else {
        Some(
            serde_json::from_value::<MoveRecipe>(row.plan.clone())?
                .replacement
                .old,
        )
    };
    let Some(old) = old else {
        return Ok(false);
    };
    let Some(runtime) = envs::runtime(conn, old).await?.filter(|runtime| {
        runtime.environment_id == Some(environment)
            && runtime.successor_of.is_none()
            && runtime.state == "retired"
    }) else {
        return Ok(false);
    };
    let Some(host) = hosts::host(conn, runtime.host_id).await? else {
        return Ok(false);
    };
    Ok(kind == "node.runtime.stop"
        && machine == host.machine_id
        && key
            == orch_link_contract::commands::SerialKey::Runtime(old)
                .wire()
                .as_deref()
        && body.get("runtime_id") == Some(&json!(old)))
}
