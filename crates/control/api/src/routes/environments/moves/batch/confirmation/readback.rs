//! The accepted batch's current native environment references and job readers.

use super::*;
use crate::routes::environments;

pub(super) struct Content;
pub(super) struct Subject {
    rows: Vec<jobs::JobRow>,
}
pub(super) struct Value {
    inventory: environments::PreparedInventory,
    jobs: Vec<environments::jobs::PreparedRead>,
}
async fn content_facts(
    conn: &mut Conn,
    input: &Readback,
    rows: &[jobs::JobRow],
) -> ApiResult<native::Facts<Source>> {
    let mut actual = input
        .jobs()
        .iter()
        .map(|(id, _)| (ObjectKind::Environment, *id))
        .collect::<BTreeSet<_>>();
    let context = targets::node_identity(conn, input.node()).await?;
    let KeyTargetKind::Object(kind) = context.object.kind else {
        return Err(ApiError::internal("a native batch context is not a node"));
    };
    actual.insert((kind, input.node()));
    let ids = input.jobs().iter().map(|(id, _)| *id).collect::<Vec<_>>();
    for id in orch_authz::resources::address_name_sources(conn, &ids)
        .await
        .map_err(ApiError::internal)?
    {
        let target = targets::node_identity(conn, id).await?;
        let KeyTargetKind::Object(kind) = target.object.kind else {
            return Err(ApiError::internal("a native address context is not a node"));
        };
        actual.insert((kind, id));
    }
    for row in rows {
        if let Some(id) = environments::jobs::waiting_machine(row) {
            actual.insert((ObjectKind::Machine, id));
        }
    }
    objects(conn, Source::Content, actual).await
}
impl reply::Projection for Content {
    type Source = Source;
    type Input = Readback;
    type Subject = Subject;
    type Value = Value;
    type Reply = api::EnvironmentBatch;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        &CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if input.node() != ctx.id("id")? {
                return Err(ApiError::not_found());
            }
            let account = ctx.user()?.id();
            let mut rows = Vec::new();
            for (environment, job) in input.jobs() {
                let row = jobs::job(conn, *job)
                    .await?
                    .filter(|row| {
                        row.kind == "environment_move"
                            && row.environment_id == Some(*environment)
                            && row.batch_id == Some(input.batch())
                            && (!matches!(input, Readback::Replay { .. })
                                || row.created_by == Some(account))
                    })
                    .ok_or_else(ApiError::not_found)?;
                rows.push(row);
            }
            let mut selected = input.required(conn).await?;
            selected.push(content_facts(conn, input, &rows).await?);
            Ok((Subject { rows }, selected))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            node(conn, principal, input.node()).await?;
            for (id, _) in input.jobs() {
                environments::reading::decide(conn, principal, *id).await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = input.jobs().iter().map(|(id, _)| *id).collect::<Vec<_>>();
            let rows = orch_db::queries::environments::routes::environments(conn, &ids).await?;
            if rows.len() != ids.len() {
                return Err(ApiError::not_found());
            }
            let inventory = environments::prepare_shown_rows(conn, principal, rows).await?;
            let mut jobs = Vec::new();
            for row in &subject.rows {
                jobs.push(environments::jobs::prepare_read_for(conn, principal, row.id).await?);
            }
            Ok((Value { inventory, jobs }, Vec::new()))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::EnvironmentBatch>> {
        Box::pin(async move {
            let environments = environments::show_inventory_prepared(
                ctx,
                conn,
                principal,
                &prepared.value.inventory,
            )
            .await?
            .into_iter()
            .map(|environment| (environment.base.id, environment.base))
            .collect::<BTreeMap<_, _>>();
            let mut jobs = Vec::new();
            for reader in &prepared.value.jobs {
                let id = reader
                    .row()
                    .environment_id
                    .ok_or_else(ApiError::not_found)?;
                let environment = environments
                    .get(&id)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                jobs.push(api::EnvironmentBatchJob {
                    environment,
                    job: reader.render(conn, None).await?,
                });
            }
            Ok(api::EnvironmentBatch {
                batch_id: input.batch(),
                jobs,
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened
                .value
                .inventory
                .preserves_sources(&fresh.value.inventory)
                && opened.value.jobs.len() == fresh.value.jobs.len()
                && opened
                    .value
                    .jobs
                    .iter()
                    .zip(&fresh.value.jobs)
                    .all(|(old, new)| old.preserves(new)))
        })
    }
}
