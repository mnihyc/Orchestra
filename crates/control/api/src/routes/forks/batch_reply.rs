//! Publication of the accepted native batch snapshot under its own current Read sources.
use super::preparation::engine_error;
use super::*;
use crate::key_operations::{
    native::{self, reply},
    targets,
};
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, key_scope::KeyTargetKind};
use orch_api_contract::{
    fork_sources::{self as rows, PlanSource as Role},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use std::collections::{BTreeSet, HashMap};
const ROW: &Declaration<Role> = &rows::BATCH_CONFIRM;
pub(super) struct Outcome {
    pub batch: Id,
    pub rows: Vec<JobRow>,
    pub roots: Vec<ObjectRef>,
    pub actor: Id,
}
pub(super) struct View {
    visible: BTreeSet<Id>,
    names: Vec<orch_api_contract::nodes::NodeRef>,
    machines: HashMap<Id, String>,
}
async fn objects(
    conn: &mut Conn,
    role: Role,
    objects: &[ObjectRef],
) -> ApiResult<native::Facts<Role>> {
    let mut selected = Vec::new();
    for object in objects {
        selected.push(targets::page_object(conn, *object).await?);
    }
    Ok(super::batch_preparation::collection(role, selected))
}
pub(super) async fn render(
    ctx: &Ctx,
    accepted: &Caller,
    input: Outcome,
) -> ApiResult<Option<ForkBatch>> {
    reply::render::<Readback>(ctx, accepted, input).await
}
struct Readback;
impl reply::Projection for Readback {
    type Source = Role;
    type Input = Outcome;
    type Subject = Vec<JobRow>;
    type Value = View;
    type Reply = ForkBatch;
    const READ_SOURCE: Role = Role::Subjects;
    fn declaration(_input: &Outcome) -> &'static Declaration<Role> {
        ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(Vec<JobRow>, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let current = orch_db::queries::jobs::forks::of_batch(conn, input.batch).await?;
            if current.len() != input.rows.len() {
                return Err(ApiError::not_found());
            }
            for old in &input.rows {
                let row = current
                    .iter()
                    .find(|row| {
                        row.id == old.id
                            && row.created_by == Some(input.actor)
                            && row.batch_id == Some(input.batch)
                    })
                    .ok_or_else(ApiError::not_found)?;
                let before = orch_engine::jobs::fork::operation::Preparation::from_job(old)
                    .map_err(engine_error)?
                    .ok_or_else(ApiError::not_found)?;
                let after = orch_engine::jobs::fork::operation::Preparation::from_job(row)
                    .map_err(engine_error)?
                    .ok_or_else(ApiError::not_found)?;
                if !before.preserves(&after) {
                    return Err(ApiError::not_found());
                }
            }
            Ok((
                current,
                vec![
                    objects(conn, Role::Sources, &input.roots).await?,
                    native::Facts::collection(Role::CopiedPages, Vec::new()),
                    objects(conn, Role::Subjects, &input.roots).await?,
                ],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a Vec<JobRow>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if principal.user() == input.actor {
                Ok(())
            } else {
                Err(ApiError::not_found())
            }
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a Vec<JobRow>,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let mut visible = BTreeSet::new();
            let mut names = Vec::new();
            let mut shown = Vec::new();
            let mut content = BTreeSet::new();
            for row in &input.rows {
                let Some(id) = row.node_id else { continue };
                if let Some(named) =
                    super::batch_read::member(conn, principal, id, &NameBounds::default()).await?
                {
                    visible.insert(row.id);
                    names.push(named);
                    shown.push(row.clone());
                    content.insert((ObjectKind::Thread, id));
                    let graph = orch_authz::engine::Graph::load(conn, &[id])
                        .await
                        .map_err(ApiError::internal)?;
                    if let Some(node) = graph.get(id) {
                        let refs = NameBounds::default()
                            .refs(conn, principal, &node.path)
                            .await?;
                        for named in refs
                            .into_values()
                            .filter(|named| !named.redacted && named.name.is_some())
                        {
                            let target = targets::node(conn, named.id).await?;
                            if let KeyTargetKind::Object(kind) = target.object.kind {
                                content.insert((kind, named.id));
                            }
                        }
                    }
                    if let Some(id) = row.environment_id {
                        content.insert((ObjectKind::Environment, id));
                    }
                    if let Some(id) = row.provider_id {
                        content.insert((ObjectKind::Provider, id));
                    }
                    if let Some(id) = job_view::waiting_machine(row) {
                        content.insert((ObjectKind::Machine, id));
                    }
                }
            }
            names.sort_by_key(|named| named.id);
            let machines = job_view::machine_name_sources(conn, &shown).await?;
            let objects = content
                .into_iter()
                .map(|(kind, id)| ObjectRef { kind, id })
                .collect::<Vec<_>>();
            Ok((
                View {
                    visible,
                    names,
                    machines,
                },
                vec![objects_for_content(conn, &objects).await?],
            ))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ForkBatch>> {
        Box::pin(async move {
            super::batch_read::view(
                conn,
                principal,
                input.batch,
                &input.rows,
                &NameBounds::default(),
                Some(&prepared.value.machines),
            )
            .await
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
        Box::pin(async move {
            Ok(old.value.visible == fresh.value.visible
                && old.value.names == fresh.value.names
                && old.value.machines == fresh.value.machines)
        })
    }
}
async fn objects_for_content(
    conn: &mut Conn,
    values: &[ObjectRef],
) -> ApiResult<native::Facts<Role>> {
    objects(conn, Role::Content, values).await
}
