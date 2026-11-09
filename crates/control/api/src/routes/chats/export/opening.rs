//! Actual chat export sources, one native supplier, and bounded current byte publication.
use super::*;
use crate::key_operations::native::{self, effects};
use crate::key_operations::{publication, targets};
use crate::middleware::transaction_guard;
use futures::{StreamExt, future::BoxFuture};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::chat_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Principal};
use orch_chat::Acting;
use orch_db::{
    Conn,
    queries::{chats as rows, hierarchy},
};
use orch_types::Id;
use std::collections::{BTreeSet, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

/// Bound the producer's retained transport chunk to the existing small-file stream scale.
pub(super) const FRAME_BYTES: usize = 32 * 1024;
pub(super) struct Export;
#[derive(Clone)]
pub(super) struct Prepared {
    pub row: rows::ChatRow,
    pub query: ChatExportGetQuery,
    pub sources: written::ChatExportSources,
    pub names: Option<HashSet<Id>>,
    facts: Vec<native::Facts<Source>>,
    // Only a native content-version witness is cached; every actor/key/name gate stays live.
    checked_sequence: Arc<AtomicI64>,
}
pub(super) struct Produced {
    response: Response,
    source: Arc<Prepared>,
}

async fn allowed(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<rows::ChatRow> {
    let origin = crate::sign_in::origin(ctx);
    let acting = Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let loaded = orch_chat::chats::load(conn, &acting, id, Mode::Read).await;
    let loaded = match loaded {
        Ok(row) => row,
        Err(error) => {
            return Err(failed_projected(
                conn,
                principal,
                Some(id),
                error,
                native::modern(principal.caller()),
            )
            .await);
        }
    };
    if loaded.row.is_people_chat() {
        if !loaded.standing.export {
            return Err(denied_for(
                Some(id),
                "chat.export",
                "a direct or a group chat is taken away as a file by those who are in it",
            ));
        }
    } else if let Some(node) = loaded.row.node_id {
        authorization::require_projected(
            conn,
            principal,
            node,
            exact(Permission::ChatExport),
            Act::Use,
            Policy::default(),
            native::modern(principal.caller()),
        )
        .await?;
    }
    Ok(loaded.row)
}
async fn principal(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    id: Id,
) -> ApiResult<(Principal, rows::ChatRow)> {
    let row = rows::chat(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let target = targets::from_chat(conn, &row).await?;
    let plan = native::bind_known(
        ctx,
        &sources::EXPORT,
        &[native::Facts::one(
            Source::Chat,
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            target,
        )],
    )?;
    // This provisional primary proof must not leave the final raw response pending.
    // The complete selector binds its actual optional name roles on the enclosing context.
    let discovery = ctx.for_effect_discovery(ctx.user()?.as_ref().clone(), ctx.app.clock.now());
    let principal = native::preparation_principal(&discovery, conn, caller, &plan).await?;
    let row = allowed(ctx, conn, &principal, id).await?;
    Ok((principal, row))
}
async fn name_facts(
    conn: &mut Conn,
    principal: &Principal,
    known: &crate::registry::OperationPlan,
    candidates: &[Id],
) -> ApiResult<(Vec<native::Facts<Source>>, Option<HashSet<Id>>)> {
    let graph = orch_authz::engine::Graph::load(conn, candidates)
        .await
        .map_err(|error| ApiError::engine(error.into()))?;
    let ids = candidates.iter().copied().collect::<BTreeSet<_>>();
    let (facts, names) = native::node_name_facts(
        known,
        &sources::EXPORT,
        principal.caller(),
        &graph,
        &ids,
        &[
            (ObjectKind::Org, Source::NameOrgs),
            (ObjectKind::Project, Source::NameProjects),
            (ObjectKind::Topic, Source::NameTopics),
            (ObjectKind::Thread, Source::NameThreads),
        ],
    )?;
    Ok((
        facts,
        native::modern(principal.caller()).then(|| names.into_iter().collect()),
    ))
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    id: Id,
    query: ChatExportGetQuery,
) -> ApiResult<Prepared> {
    let (principal, row) = principal(ctx, conn, caller, id).await?;
    let target = targets::from_chat(conn, &row).await?;
    let primary = native::Facts::one(Source::Chat, target.object.kind, target);
    let known = native::bind_known(ctx, &sources::EXPORT, std::slice::from_ref(&primary))?;
    let sources = ctx
        .app
        .engine
        .chat_export_sources(
            conn,
            id,
            (query.from, query.to),
            query.format.unwrap_or(ExportFormat::Markdown),
        )
        .await
        .map_err(ApiError::engine)?;
    let (additional, names) =
        name_facts(conn, &principal, &known, sources.name_candidates()).await?;
    let mut facts = vec![primary];
    facts.extend(additional);
    let checked_sequence = Arc::new(AtomicI64::new(row.next_seq));
    Ok(Prepared {
        row,
        query,
        sources,
        names,
        facts,
        checked_sequence,
    })
}
async fn check(ctx: &Ctx, conn: &mut Conn, reader: &Principal, old: &Prepared) -> ApiResult<()> {
    let (principal, row) = principal(ctx, conn, reader.caller(), old.row.id).await?;
    let refused = || {
        ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the original chat export source is no longer available",
        )
    };
    if row.kind != old.row.kind
        || row.node_id != old.row.node_id
        || row.org_id != old.row.org_id
        || row.direct_key != old.row.direct_key
    {
        return Err(refused());
    }
    let target = targets::from_chat(conn, &row).await?;
    let primary = native::Facts::one(Source::Chat, target.object.kind, target);
    let known = native::bind_known(ctx, &sources::EXPORT, std::slice::from_ref(&primary))?;
    let (additional, names) =
        name_facts(conn, &principal, &known, old.sources.name_candidates()).await?;
    let mut facts = vec![primary];
    facts.extend(additional);
    let plan = native::bind(ctx, &sources::EXPORT, &facts)?;
    // Finish the current transport content proof as well as its scope admission. The
    // preliminary source principal leaves optional name disclosure pending until here.
    if plan
        .read_response(ctx, conn, principal.caller(), |_, _| {
            Box::pin(async { Ok(()) })
        })
        .await?
        .is_none()
    {
        return Err(refused());
    }
    if let Some(old) = &old.names
        && names.as_ref().is_none_or(|fresh| !old.is_subset(fresh))
    {
        return Err(refused());
    }
    // Native message writers serialize on Chat and advance this sequence. A new sequence
    // validates the old exact numbered range once; unrelated later messages cannot replace it.
    if old.checked_sequence.load(Ordering::Acquire) != row.next_seq {
        let fresh = ctx
            .app
            .engine
            .chat_export_sources(
                conn,
                row.id,
                old.sources.range(),
                old.query.format.unwrap_or(ExportFormat::Markdown),
            )
            .await
            .map_err(ApiError::engine)?;
        if old.sources != fresh {
            return Err(refused());
        }
        old.checked_sequence.store(row.next_seq, Ordering::Release);
    }
    Ok(())
}

impl effects::Effect for Export {
    type Source = Source;
    type Query = ChatExportGetQuery;
    type Body = ();
    type Value = Prepared;
    type Output = Produced;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::EXPORT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: ChatExportGetQuery,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            principal(ctx, conn, caller, id).await?;
            let row = rows::lock(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let anchors = [row.home()];
            let ids = hierarchy::with_ancestors(conn, &anchors)
                .await?
                .into_iter()
                .map(|row| row.id)
                .collect::<BTreeSet<_>>();
            for id in ids {
                hierarchy::lock(conn, id).await?;
            }
            let prepared = select(ctx, conn, caller, id, query).await?;
            let facts = prepared.facts.clone();
            Ok((prepared, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            allowed(ctx, conn, principal, value.row.id)
                .await
                .map(|_| ())
        })
    }
}
impl effects::ApplyOwned for Export {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        _principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Produced>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            deadline.check()?;
            let response = super::supply(ctx, &mut tx, value).await?;
            // Repeating native preparation never repeats the archive/blob supplier.
            let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
            check(&current, &mut tx, guard.principal(), value).await?;
            super::record(&current, &mut tx, guard.principal(), value, move || {
                before.check()
            })
            .await?;
            deadline.check()?;
            guard.check_deadline(&current)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(Produced {
                response,
                source: Arc::new(value.clone()),
            }))
        })
    }
}

pub(super) async fn read(ctx: Ctx, _query: ChatExportGetQuery) -> ApiResult<Response> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Export>(&ctx, tx).await?;
    let produced = completed.output;
    let header = publication::frame(
        &completed.ctx,
        &completed.accepted,
        produced.source,
        |ctx, conn, reader, source| Box::pin(check(ctx, conn, reader, source)),
    )?;
    let (parts, body) = produced.response.into_parts();
    let stream = body.into_data_stream();
    let frame = header.clone();
    let stream = futures::stream::try_unfold(
        (Box::pin(stream), Bytes::new(), frame),
        |(mut source, mut pending, frame)| async move {
            while pending.is_empty() {
                let Some(next) = source.next().await else {
                    return Ok(None);
                };
                pending = next.map_err(ApiError::internal)?;
            }
            let next = pending.split_to(pending.len().min(FRAME_BYTES));
            Ok::<_, ApiError>(Some((frame.data(next), (source, pending, frame))))
        },
    );
    publication::streaming_response(
        &completed.ctx,
        Response::from_parts(parts, Body::empty()),
        header,
        stream,
    )
}
