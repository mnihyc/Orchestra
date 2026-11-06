//! One existing message edit: native no-op selection, stored source witnesses and receipt projection.
use super::*;
use crate::key_operations::{blob_sources, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::Caller;
use orch_chat::following::BlobOrigin;
use orch_types::Hash;

pub(super) struct Effects {
    pub selected: Vec<(Hash, BlobOrigin)>,
    pub no_op: bool,
}

// An actually selected opened association only: no Caller, content or authority.
#[derive(PartialEq)]
struct SelectedSources(Vec<(Hash, BlobOrigin)>);

pub(super) fn retain_sources(ctx: &Ctx, selected: &[(Hash, BlobOrigin)]) -> ApiResult<()> {
    ctx.retain_source(SelectedSources(selected.to_vec()))
}

async fn base(ctx: &Ctx, conn: &mut Conn) -> ApiResult<OperationPlan> {
    let target = targets::chat(conn, ctx.id("id")?).await?;
    let mut plan = operation::destination(ctx, &target)?;
    // As for the other message mutations, preparation discloses no live labels.
    // The common response reader replaces these with its actual bounded sources.
    operation::bind_names(&mut plan, &[])?;
    Ok(plan)
}

async fn quoted_source(
    conn: &mut Conn,
    work: &Work,
    row: &rows::ChatRow,
    parts: &[ChatPart],
) -> ApiResult<Vec<blob_sources::Binding>> {
    let quoted = orch_chat::quotes::source_of(conn, row, parts).await;
    let mut required = Vec::new();
    if let Some(quoted) = work.refuse(conn, Some(row.id), quoted).await? {
        required.push(blob_sources::Binding {
            source: blob_sources::SourceKind::Thread,
            target: targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: quoted.thread,
                },
            )
            .await?,
        });
    }
    Ok(required)
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    parts: Vec<ChatPart>,
) -> ApiResult<OperationPlan> {
    let mut plan = base(ctx, conn).await?;
    let work = operation::prepare_work(ctx, conn, caller, &plan).await?;
    let id = ctx.id("id")?;
    let loaded = load(conn, &work, id, Mode::Read).await?;
    let masking = redactor(conn, &work, &loaded).await?;
    let prepared = messages::prepare_edit(
        conn,
        &work.acting(),
        &loaded,
        ctx.id("message_id")?,
        parts,
        &masking,
    )
    .await;
    let prepared = work.refuse(conn, Some(id), prepared).await?;
    let retained = ctx.optional_retained_source::<SelectedSources>()?;
    // Once the actual writer retained a changed association, its consumption
    // stays fixed even if later metadata now compares equal. Some(emptyVec)
    // still represents a changed text/quote writer, never an unopened no-op.
    let no_op = prepared.unchanged().is_some() && retained.is_none();
    let selected = if no_op {
        // The native no-op does not consume a blob or quoted item. Do not add a
        // source prerequisite just because its unchanged stored body contains one.
        blob_sources::bind(&mut plan, &[])?;
        Vec::new()
    } else {
        let required = quoted_source(conn, &work, &loaded.row, prepared.parts()).await?;
        let hashes = orch_chat::attachments::blobs_in(prepared.parts());
        let sources = match retained {
            Some(selected) => {
                blob_sources::prepare_selected(
                    ctx,
                    conn,
                    caller,
                    &plan,
                    &hashes,
                    &required,
                    &selected.0,
                )
                .await?
            }
            None => blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &required).await?,
        };
        blob_sources::bind(&mut plan, &sources.bindings)?;
        sources.selected
    };
    // Live names are response sources; the final common message reader binds
    // its actual set after the edit. No field is resolved by loaning ChatGet.
    operation::bind_names(&mut plan, &[])?;
    Ok(plan.with_data(Effects { selected, no_op }))
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ChatMessageUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let result = prepare(ctx, conn, caller, body.parts).await;
        operation::project_current_result(ctx, conn, caller, result).await
    })
}

/// A preliminary source-free no-op can become changed before the writer's first
/// native observation. Admit its actual parts once before retaining any source;
/// this reads no new message observation and never rebases the native CAS.
pub(super) async fn first_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    work: &Work,
    loaded: &Loaded,
    parts: &[ChatPart],
) -> ApiResult<(Principal, Vec<(Hash, BlobOrigin)>)> {
    admit_sources(ctx, conn, work, loaded, parts, None).await
}

/// Re-admit only the original selected witnesses using their current identities.
/// A new alternate source or custody after the wait never replaces one.
pub(super) async fn current_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    work: &Work,
    loaded: &Loaded,
    parts: &[ChatPart],
    selected: &[(Hash, BlobOrigin)],
) -> ApiResult<Principal> {
    admit_sources(ctx, conn, work, loaded, parts, Some(selected))
        .await
        .map(|(principal, _)| principal)
}

async fn admit_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    work: &Work,
    loaded: &Loaded,
    parts: &[ChatPart],
    selected: Option<&[(Hash, BlobOrigin)]>,
) -> ApiResult<(Principal, Vec<(Hash, BlobOrigin)>)> {
    let mut plan = base(ctx, conn).await?;
    let required = quoted_source(conn, work, &loaded.row, parts).await?;
    let hashes = orch_chat::attachments::blobs_in(parts);
    let sources = match selected {
        Some(selected) => {
            blob_sources::prepare_selected(
                ctx,
                conn,
                work.principal.caller(),
                &plan,
                &hashes,
                &required,
                selected,
            )
            .await?
        }
        None => {
            blob_sources::prepare(
                ctx,
                conn,
                work.principal.caller(),
                &plan,
                &hashes,
                &required,
            )
            .await?
        }
    };
    blob_sources::bind(&mut plan, &sources.bindings)?;
    operation::bind_names(&mut plan, &[])?;
    let admitted = plan.admit(work.principal.caller())?;
    ctx.checkpoint_disclosure(admitted.disclosure);
    let principal = authorization::load_principal(
        conn,
        admitted.caller,
        orch_types::Time::new(ctx.app.clock.now()),
    )
    .await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
    Ok((principal, sources.selected))
}

/// Effects have completed. Keep this exact action and supply only its optional
/// current Chat/name response; completed byte consumption is never replayed.
pub(super) async fn response_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &MessageRow,
) -> ApiResult<OperationPlan> {
    if row.chat_id != ctx.id("id")? || row.id != ctx.id("message_id")? {
        return Err(ApiError::not_found());
    }
    let mut plan = base(ctx, conn).await?;
    blob_sources::bind(&mut plan, &[])?;
    Ok(plan)
}
