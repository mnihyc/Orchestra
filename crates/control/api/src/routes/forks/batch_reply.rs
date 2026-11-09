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
    pub replay: bool,
}
pub(super) struct View {
    visible: BTreeSet<Id>,
    names: Vec<ReferenceName>,
    machines: HashMap<Id, String>,
}

/// Reference disclosure remains fixed while the thread's live execution facts advance.
#[derive(Debug, PartialEq, Eq)]
struct ReferenceName {
    id: Id,
    kind: orch_api_contract::nodes::NodeKind,
    name: Option<String>,
    redacted: bool,
    has_display: bool,
}
impl ReferenceName {
    fn of(reference: &orch_api_contract::nodes::NodeRef) -> Self {
        Self {
            id: reference.id,
            kind: reference.kind,
            name: reference.name.clone(),
            redacted: reference.redacted,
            has_display: reference.thread_display.is_some(),
        }
    }
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
    tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
        phase = "batch.reply.begin", disclosure = ?ctx.disclosure(), "native boundary entered");
    let result = native::observed(
        ctx,
        "batch.reply",
        reply::render::<Readback>(ctx, accepted, input).await,
    );
    tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
        phase = "batch.reply.end", disclosure = ?ctx.disclosure(),
        full = matches!(&result, Ok(Some(_))), "native reply selected");
    result
}
pub(super) async fn render_replay(ctx: &Ctx, input: Outcome) -> ApiResult<Option<ForkBatch>> {
    reply::render_completed_replay::<Readback>(ctx, input.actor, input).await
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
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(Vec<JobRow>, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let current = orch_db::queries::jobs::forks::of_batch(conn, input.batch).await?;
            if current.len() != input.rows.len() {
                tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
                    phase = "batch.reply.subject", reason = "member_count_changed", "native reply source closed");
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
                    .ok_or_else(|| {
                        tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
                            phase = "batch.reply.subject", reason = "member_association_changed", "native reply source closed");
                        ApiError::not_found()
                    })?;
                let before = native::observed(
                    ctx,
                    "batch.reply.original_evidence",
                    orch_engine::jobs::fork::operation::Preparation::from_job(old)
                        .map_err(engine_error),
                )?
                .ok_or_else(ApiError::not_found)?;
                let after = native::observed(
                    ctx,
                    "batch.reply.current_evidence",
                    orch_engine::jobs::fork::operation::Preparation::from_job(row)
                        .map_err(engine_error),
                )?
                .ok_or_else(ApiError::not_found)?;
                if !before.preserves(&after) {
                    tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
                        phase = "batch.reply.subject", reason = "native_evidence_changed", "native reply source closed");
                    return Err(ApiError::not_found());
                }
            }
            let mut facts = vec![objects(conn, Role::Subjects, &input.roots).await?];
            if !input.replay {
                facts.push(objects(conn, Role::Sources, &input.roots).await?);
                facts.push(native::Facts::collection(Role::CopiedPages, Vec::new()));
            }
            native::observed(ctx, "batch.reply.subject", Ok((current, facts)))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        _subject: &'a Vec<JobRow>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let result = if principal.user() == input.actor {
                Ok(())
            } else {
                Err(ApiError::not_found())
            };
            native::observed(ctx, "batch.reply.author", result)
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
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
                    names.push(ReferenceName::of(&named));
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
            native::observed(
                ctx,
                "batch.reply.prepare",
                Ok((
                    View {
                        visible,
                        names,
                        machines,
                    },
                    vec![objects_for_content(conn, &objects).await?],
                )),
            )
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ForkBatch>> {
        Box::pin(async move {
            native::observed(
                ctx,
                "batch.reply.render",
                super::batch_read::view(
                    conn,
                    principal,
                    input.batch,
                    &input.rows,
                    &NameBounds::default(),
                    Some(&prepared.value.machines),
                )
                .await,
            )
        })
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let preserved = old.value.visible == fresh.value.visible
                && old.value.names == fresh.value.names
                && old.value.machines == fresh.value.machines;
            tracing::debug!(target: "orch_api::native_operations", operation = ctx.route.name,
                phase = "batch.reply.validate", preserved,
                visible_preserved = old.value.visible == fresh.value.visible,
                names_preserved = old.value.names == fresh.value.names,
                machines_preserved = old.value.machines == fresh.value.machines,
                "native reply source checked");
            Ok(preserved)
        })
    }
}
async fn objects_for_content(
    conn: &mut Conn,
    values: &[ObjectRef],
) -> ApiResult<native::Facts<Role>> {
    objects(conn, Role::Content, values).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::nodes::{NodeKind, NodeRef};
    use orch_api_contract::threads::ThreadDisplay;

    #[test]
    fn execution_progress_keeps_reference_disclosure_but_changed_names_or_visibility_do_not() {
        let original = NodeRef {
            id: orch_types::new_id(),
            kind: NodeKind::Thread,
            name: Some("Batch member".into()),
            redacted: false,
            thread_display: Some(Box::new(ThreadDisplay::default())),
        };
        let mut progress = original.clone();
        progress.thread_display = Some(Box::new(ThreadDisplay {
            queued_inputs: 1,
            waiting_for_answer: true,
            ..ThreadDisplay::default()
        }));
        assert_ne!(original, progress);
        assert_eq!(ReferenceName::of(&original), ReferenceName::of(&progress));

        let mut changed = original.clone();
        changed.id = orch_types::new_id();
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
        changed = original.clone();
        changed.kind = NodeKind::Topic;
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
        changed = original.clone();
        changed.name = Some("Another name".into());
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
        changed = original.clone();
        changed.name = None;
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
        changed = original.clone();
        changed.redacted = true;
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
        changed = original.clone();
        changed.thread_display = None;
        assert_ne!(ReferenceName::of(&original), ReferenceName::of(&changed));
    }
}
