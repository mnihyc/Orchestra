//! Actual retained-file reservation origins, shared current scopes and native command custody.

use super::*;
use crate::jobs::command_identity;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    environment::{EnvironmentPurge, EnvironmentRestore},
    environment_sources::{self as declarations, RetainedChangeSource as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_link_contract::commands::CommandKind;
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};

const FIELD: &str = "retained_operation";
const REQUIRED: &str = "retained_operation_required";
const JOURNAL: &str = "retained_commands";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    job: Id,
    environment: Id,
    revision: i64,
    purge: bool,
    original: Option<CapturedOperation>,
    inherited: bool,
    frozen: Hash,
}
pub(crate) fn applies(row: &jobs::JobRow) -> bool {
    row.plan.get(FIELD).is_some() || row.plan.get(REQUIRED).is_some()
}
pub(super) fn inherited(row: &jobs::JobRow, plan: &Json) -> Result<Option<Json>, EngineError> {
    if plan.get(FIELD).is_none() && plan.get(REQUIRED).is_none() {
        return Ok(None);
    }
    let mut source: Anchor = serde_json::from_value(plan[FIELD].clone())?;
    if plan[REQUIRED] != true
        || !source.purge
        || row.environment_id != Some(source.environment)
        || row.plan["removal_revision"] != json!(source.revision)
        || frozen(row)? != source.frozen
    {
        return Err(EngineError::unreadable(
            "the accepted purge custody changed on retry",
        ));
    }
    source.job = row.id;
    source.inherited = true;
    Ok(Some(
        json!({"retained_operation":source,"retained_operation_required":true}),
    ))
}
fn frozen(row: &jobs::JobRow) -> Result<Hash, EngineError> {
    command_identity::hash(
        &json!({"environment":row.environment_id,"removal_revision":row.plan["removal_revision"],"volume_refs":row.plan["volume_refs"]}),
    )
}
fn anchor(row: &jobs::JobRow) -> Result<Option<Anchor>, EngineError> {
    let Some(value) = row.plan.get(FIELD) else {
        return Ok(None);
    };
    let Ok(anchor) = serde_json::from_value::<Anchor>(value.clone()) else {
        return Ok(None);
    };
    if row.plan.get(REQUIRED) != Some(&json!(true))
        || anchor.job != row.id
        || row.environment_id != Some(anchor.environment)
        || row.kind
            != if anchor.purge {
                jobs::retained::PURGE
            } else {
                jobs::retained::RESTORE
            }
        || row.plan["removal_revision"] != json!(anchor.revision)
        || frozen(row)? != anchor.frozen
        || anchor.original.as_ref().is_some_and(|origin| {
            Some(origin.user()) != row.created_by
                || origin.operation()
                    != if anchor.purge {
                        declarations::PURGE.operation
                    } else {
                        declarations::RESTORE.operation
                    }
        })
        || anchor.original.is_none()
            && !(anchor.purge && anchor.inherited && row.plan["purge_started"] == true)
    {
        return Ok(None);
    }
    Ok(Some(anchor))
}
/// Save the exact accepted operation beside its native kept-file reservation.
pub async fn queue_for_operation(
    conn: &mut Conn,
    environment: Id,
    revision: i64,
    purge: bool,
    by: Id,
    identity: Json,
    original: &CapturedOperation,
    now: chrono::DateTime<chrono::Utc>,
    before: impl FnOnce() -> Result<(), EngineError> + Send,
) -> Result<jobs::JobRow, MakeError> {
    if original.user() != by
        || original.operation()
            != if purge {
                declarations::PURGE.operation
            } else {
                declarations::RESTORE.operation
            }
    {
        return Err(EngineError::unreadable(
            "the retained-file reservation changed its accepted operation",
        )
        .into());
    }
    let inherited = if purge {
        retained::started_purge(conn, environment, revision).await?
    } else {
        None
    };
    let mut row = super::queue_before(
        conn,
        environment,
        revision,
        purge,
        Some(by),
        identity,
        now,
        before,
    )
    .await?;
    let mut original = Some(original.clone());
    let mut inherited_cleanup = false;
    if row.plan["purge_started"] == true
        && let Some((_, plan)) = inherited
    {
        if plan.get(FIELD).is_some() || plan.get(REQUIRED).is_some() {
            let old: Anchor =
                serde_json::from_value(plan[FIELD].clone()).map_err(EngineError::from)?;
            if plan[REQUIRED] != true
                || old.environment != environment
                || old.revision != revision
                || !old.purge
            {
                return Err(
                    EngineError::unreadable("the original purge reservation changed").into(),
                );
            }
            original = old.original;
        } else {
            original = None;
        }
        inherited_cleanup = true;
    }
    let anchor = Anchor {
        job: row.id,
        environment,
        revision,
        purge,
        original,
        inherited: inherited_cleanup,
        frozen: frozen(&row)?,
    };
    let patch = json!({"retained_operation":anchor,"retained_operation_required":true});
    jobs::merge_plan(conn, row.id, &patch, now).await?;
    row.plan
        .as_object_mut()
        .ok_or_else(|| EngineError::unreadable("the retained-file job has no plan"))?
        .extend(
            patch
                .as_object()
                .ok_or_else(|| EngineError::unreadable("the retained-file job has no origin"))?
                .clone(),
        );
    Ok(row)
}
async fn removal(conn: &mut Conn, row: &jobs::JobRow) -> Result<retained::Removal, Error> {
    let anchor = anchor(row)?.ok_or(Error::Refused)?;
    let removal = retained::removal(conn, anchor.environment)
        .await?
        .ok_or(Error::Refused)?;
    if removal.removal_revision != anchor.revision
        || removal.retained_volumes != row.plan["volume_refs"]
        || removal.removal_phase.as_deref()
            != Some(if anchor.purge { "purging" } else { "restoring" })
        || removal.purged_at.is_some()
    {
        return Err(Error::Refused);
    }
    Ok(removal)
}
fn refused(error: Error) -> Stop {
    match error {
        Error::Native(error) => error.into(),
        _ => failed(
            "denied",
            "the original retained-file operation no longer permits this work",
        ),
    }
}
struct Restoring;
struct Purging;
struct Input {
    job: Id,
    original: CapturedOperation,
}
struct Value {
    row: jobs::JobRow,
    removal: retained::Removal,
}

async fn prepare_source(
    conn: &mut Conn,
    input: &Input,
) -> Result<(Value, Vec<Facts<Role>>), Error> {
    let row = jobs::job(conn, input.job).await?.ok_or(Error::Refused)?;
    let anchor = anchor(&row)?.ok_or(Error::Refused)?;
    if anchor.original.as_ref() != Some(&input.original) {
        return Err(Error::Refused);
    }
    let removal = removal(conn, &row).await?;
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: anchor.environment,
        },
    )
    .await
    .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
    Ok((
        Value { row, removal },
        vec![Facts::one(
            Role::Environment,
            KeyTargetKind::Object(ObjectKind::Environment),
            target,
        )],
    ))
}
macro_rules! stage {
    ($type:ty,$endpoint:ty,$declaration:ident,$purge:expr) => {
        impl Source for $type {
            type Role = Role;
            type Input = Input;
            type Value = Value;
            const ROUTE: Route = <$endpoint>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &declarations::$declaration;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a Input,
                _: Time,
            ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
                Box::pin(prepare_source(conn, input))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                _: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move {
                    let (manager, admin) =
                        super::authority(conn, principal, value.removal.id).await?;
                    if if $purge { admin } else { manager || admin } {
                        Ok(())
                    } else {
                        Err(Error::Refused)
                    }
                })
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                input: &'a Input,
                value: &'a Value,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move {
                    jobs::lock(conn, input.job).await?.ok_or(Error::Refused)?;
                    hosting::lock_environment(conn, value.removal.id)
                        .await?
                        .ok_or(Error::Refused)?;
                    Ok(())
                })
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a Input,
                opened: &'a Value,
                fresh: &'a Value,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move {
                    Ok(opened.row.id == fresh.row.id
                        && opened.removal.id == fresh.removal.id
                        && opened.removal.removal_revision == fresh.removal.removal_revision
                        && opened.removal.retained_volumes == fresh.removal.retained_volumes
                        && opened.removal.removal_phase == fresh.removal.removal_phase)
                })
            }
        }
    };
}
stage!(Restoring, EnvironmentRestore, RESTORE, false);
stage!(Purging, EnvironmentPurge, PURGE, true);

/// Restore the actual captured operation through the common current-source adapter.
pub(super) async fn requester(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Principal, Stop> {
    let anchor = anchor(row)?.ok_or_else(|| refused(Error::Refused))?;
    let original = anchor.original.ok_or_else(|| refused(Error::Refused))?;
    let input = Input {
        job: row.id,
        original: original.clone(),
    };
    if anchor.purge {
        continuing::prepare::<Purging>(conn, clock, &original, input)
            .await
            .map(|frame| frame.into_parts().principal)
            .map_err(refused)
    } else {
        continuing::prepare::<Restoring>(conn, clock, &original, input)
            .await
            .map(|frame| frame.into_parts().principal)
            .map_err(refused)
    }
}

/// Exact private physical command of the retained reservation, not a reusable actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    job: Id,
    kind: CommandKind,
    canonical: Json,
    cleanup: bool,
}
impl Authority {
    /// The kind signed by the actual native producer.
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    /// Keep native reservation locks before the canonical command row.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        if let Some(row) = jobs::lock(conn, self.job).await?
            && let Some(anchor) = anchor(&row)?
        {
            hosting::lock_environment(conn, anchor.environment).await?;
        }
        Ok(())
    }
    async fn callers(
        &self,
        conn: &mut Conn,
        id: Id,
        machine: Id,
        key: Option<&str>,
        kind: &str,
        body: &Json,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.lock_source(conn).await?;
        let Some(row) = jobs::job(conn, self.job).await? else {
            return Ok(None);
        };
        let Some(anchor) = anchor(&row)? else {
            return Ok(None);
        };
        let promise = match removal(conn, &row).await {
            Ok(promise) => promise,
            Err(Error::Native(error)) => return Err(error),
            Err(_) => return Ok(None),
        };
        let lease: Option<Time> = row
            .plan
            .pointer("/runner/until")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        let Some(lease) = lease.filter(|until| until.get() > clock.now() && row.is_open()) else {
            return Ok(None);
        };
        let canonical = command_identity::canonical(id, machine, key, kind, body)?;
        if self.kind.as_str() != kind
            || self.canonical != canonical
            || row
                .plan
                .get(JOURNAL)
                .and_then(|journal| journal.get(id.to_string()))
                != Some(&json!({"canonical":canonical,"cleanup":self.cleanup}))
        {
            return Ok(None);
        }
        if self.cleanup {
            let candidate = row
                .plan
                .get("runtime")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok());
            let allowed = match self.kind {
                CommandKind::NodeRuntimeRemove => {
                    body["runtime_id"] == json!(candidate)
                        && candidate.is_some()
                        && body["keep_volumes"] == true
                        && body
                            .get("volume_refs")
                            .and_then(Json::as_array)
                            .is_some_and(Vec::is_empty)
                }
                CommandKind::EnvDetach => {
                    body["environment_id"] == json!(anchor.environment)
                        && row.plan["registration_attach_started"] == true
                }
                _ => false,
            };
            return Ok(allowed.then(|| vec![(None, Some(lease))]));
        }
        if anchor.purge
            && (row.plan["purge_started"] == true
                || promise.volumes_purge_at.is_some_and(|at| at <= clock.now()))
        {
            return Ok(Some(vec![(None, Some(lease))]));
        }
        if row.cancel_requested() {
            return Ok(None);
        }
        let principal = match requester(conn, &row, clock).await {
            Ok(principal) => principal,
            Err(Stop::Failed(error)) if error.code == "denied" => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(format!("{error:?}"))),
        };
        if !anchor.purge && row.plan["connected"] == true {
            let Some(environment) = hosting::environment(conn, anchor.environment).await? else {
                return Ok(None);
            };
            if super::connected_machine(conn, &environment, &row.plan)
                .await
                .is_err()
            {
                return Ok(None);
            }
        }
        Ok(principal
            .caller()
            .key_deadline_allows(clock)
            .then(|| vec![(Some(principal.caller().clone()), Some(lease))]))
    }
    /// Validate the real proposed command after queue waits on this same connection.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
            clock,
        )
        .await
    }
    /// Validate the same native journaled command at its first-wire boundary.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.callers(
            conn,
            command.id,
            command.machine_id,
            command.serial_key(),
            &command.kind,
            &command.body,
            clock,
        )
        .await
    }
}
pub(crate) async fn decorate(job: &Job, command: &mut Outgoing, cleanup: bool) -> Result<(), Stop> {
    if !applies(&job.row().await?) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let anchor = anchor(&row)?.ok_or_else(|| refused(Error::Refused))?;
    hosting::lock_environment(&mut tx, anchor.environment).await?;
    removal(&mut tx, &row).await.map_err(refused)?;
    let canonical = command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )?;
    let entry = json!({"canonical":canonical,"cleanup":cleanup});
    let mut journal = row.plan.get(JOURNAL).cloned().unwrap_or_else(|| json!({}));
    if journal
        .get(command.id.to_string())
        .is_some_and(|old| old != &entry)
    {
        return Err(failed("identity", "the retained command changed"));
    }
    journal
        .as_object_mut()
        .ok_or_else(|| failed("identity", "the retained command journal changed"))?
        .insert(command.id.to_string(), entry);
    let authority = crate::commands::CommandAuthority::RetainedLifecycle(Box::new(Authority {
        job: job.id(),
        kind: command.kind,
        canonical,
        cleanup,
    }));
    if command
        .authority
        .as_ref()
        .is_some_and(|old| old != &authority)
    {
        return Err(failed(
            "identity",
            "the retained command has another native owner",
        ));
    }
    jobs::merge_plan(
        &mut tx,
        row.id,
        &json!({"retained_commands":journal}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    command.authority = Some(authority);
    Ok(())
}
