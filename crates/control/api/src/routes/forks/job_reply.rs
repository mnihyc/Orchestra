//! Current source facts of the native producing job snapshot, shared by single fork creators.
use super::preparation::engine_error;
use super::*;
use crate::key_operations::{
    native::{self, reply},
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyTargetKind};
use orch_api_contract::{fork_sources::PlanSource as Role, operation_sources::Declaration};
use orch_authz::Caller;
use std::collections::HashMap;
pub(super) struct Outcome {
    pub row: JobRow,
    pub parent: Id,
    pub actor: Id,
    pub declaration: &'static Declaration<Role>,
    /// None is a completed replay: spent copy facts are omitted, not fabricated empty.
    pub copied: Option<Vec<Id>>,
}
async fn primary(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Role>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Role::Thread, target.object.kind, target))
}
async fn copied(conn: &mut Conn, ids: &[Id]) -> ApiResult<native::Facts<Role>> {
    let mut targets = Vec::new();
    for &id in ids {
        targets.push(targets::node(conn, id).await?);
    }
    Ok(native::Facts::collection(
        Role::CopiedPages,
        if targets.is_empty() {
            vec![]
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Thread), targets)]
        },
    ))
}
pub(super) async fn render(
    ctx: &Ctx,
    accepted: &Caller,
    outcome: Outcome,
) -> ApiResult<Option<Job>> {
    reply::render::<Readback>(ctx, accepted, outcome).await
}
pub(super) async fn render_replay(ctx: &Ctx, outcome: Outcome) -> ApiResult<Option<Job>> {
    reply::render_completed_replay::<Readback>(ctx, outcome.actor, outcome).await
}
struct Readback;
impl reply::Projection for Readback {
    type Source = Role;
    type Input = Outcome;
    type Subject = JobRow;
    type Value = HashMap<Id, String>;
    type Reply = Job;
    const READ_SOURCE: Role = Role::Subjects;
    fn declaration(input: &Outcome) -> &'static Declaration<Role> {
        input.declaration
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(JobRow, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let row = orch_db::queries::jobs::job(conn, input.row.id)
                .await?
                .filter(|row| {
                    row.kind == orch_engine::jobs::fork::KIND
                        && row.node_id == Some(input.parent)
                        && row.created_by == Some(input.actor)
                })
                .ok_or_else(ApiError::not_found)?;
            let old = orch_engine::jobs::fork::operation::Preparation::from_job(&input.row)
                .map_err(engine_error)?
                .ok_or_else(ApiError::not_found)?;
            let current = orch_engine::jobs::fork::operation::Preparation::from_job(&row)
                .map_err(engine_error)?
                .ok_or_else(ApiError::not_found)?;
            if !old.preserves(&current) {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native fork job changed",
                ));
            }
            let target = targets::node(conn, input.parent).await?;
            let mut facts = vec![native::Facts::collection(
                Role::Subjects,
                vec![(target.object.kind, vec![target])],
            )];
            if let Some(ids) = &input.copied {
                facts.push(primary(conn, input.parent).await?);
                facts.push(copied(conn, ids).await?);
            }
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                input.parent,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a JobRow,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let mut groups = std::collections::BTreeMap::<KeyTargetKind, Vec<_>>::new();
            let mut objects = Vec::new();
            if let Some(id) = input.row.environment_id {
                objects.push(ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                });
            }
            if let Some(id) = input.row.provider_id {
                objects.push(ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                });
            }
            if let Some(id) = job_view::waiting_machine(&input.row) {
                objects.push(ObjectRef {
                    kind: ObjectKind::Machine,
                    id,
                });
            }
            for object in objects {
                let target = targets::page_object(conn, object).await?;
                groups.entry(target.object.kind).or_default().push(target);
            }
            let names =
                job_view::machine_name_sources(conn, std::slice::from_ref(&input.row)).await?;
            Ok((
                names,
                vec![native::Facts::collection(
                    Role::Content,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Outcome,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            job_view::view_with_machine_sources(conn, &input.row, Some(&prepared.value)).await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.subject.id == fresh.subject.id && old.value == fresh.value) })
    }
}
