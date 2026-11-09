//! One bounded native uploaded-source preview outside retries, with current frozen source checks.

use super::*;
use crate::key_operations::{blob_sources, native, targets};
use futures::future::BoxFuture;
use native::effects::{self, Effect, Initial};
use orch_api_contract::{
    import_preview_sources::{PREVIEW, Source},
    operation_sources::Declaration,
    upload_sources::Source as Bytes,
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_chat::following::BlobOrigin;
use orch_types::{Hash, Id};
use std::sync::Arc;

pub(super) struct Preview;

pub(super) struct Known {
    node: Id,
    request: ImportPreviewRequest,
    selected: Vec<(Hash, BlobOrigin)>,
}

pub(super) struct Fetched {
    known: Known,
    origin: CapturedOperation,
    preview: ImportPreview,
}

pub(super) fn byte_facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    use blob_sources::SourceKind;
    [
        (Bytes::ReadChats, SourceKind::Chat),
        (Bytes::ReadThreads, SourceKind::Thread),
        (Bytes::ReadDocuments, SourceKind::Document),
        (Bytes::ReadEnvironments, SourceKind::Environment),
        (Bytes::ReadUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(source, kind)| {
        let targets = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect::<Vec<_>>();
        native::Facts::collection(
            Source::Bytes(source),
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), targets)]
            },
        )
    })
    .collect()
}

async fn sources(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: ImportPreviewRequest,
    old: Option<&Fetched>,
) -> ApiResult<(Known, Vec<native::Facts<Source>>)> {
    if request.source.files.is_empty() {
        return Err(ApiError::invalid(
            "source.files",
            "name the uploads to import",
        ));
    }
    let node = ctx.id("id")?;
    if old.is_some_and(|old| old.known.node != node || old.known.request != request) {
        return Err(ApiError::internal(
            "the original native preview input changed",
        ));
    }
    let target = targets::node_identity(conn, node).await?;
    let mut facts = vec![native::Facts::one(
        Source::Destination,
        target.object.kind,
        target,
    )];
    let known = native::bind_known(ctx, &PREVIEW, &facts)?;
    let known = match old {
        Some(old) => known.with_origin(ctx, conn, caller, &old.origin).await?,
        None => known,
    };
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    require(ctx, conn, &principal, node).await?;
    let hashes = request
        .source
        .files
        .iter()
        .map(|file| file.blob)
        .collect::<Vec<_>>();
    let selected = match old {
        None => blob_sources::prepare(ctx, conn, principal.caller(), &known, &hashes, &[]).await?,
        Some(old) => {
            blob_sources::prepare_selected(
                ctx,
                conn,
                principal.caller(),
                &known,
                &hashes,
                &[],
                &old.known.selected,
            )
            .await?
        }
    };
    if old.is_some_and(|old| old.known.selected != selected.selected) {
        return Err(ApiError::not_found());
    }
    facts.extend(byte_facts(&selected.bindings));
    Ok((
        Known {
            node,
            request,
            selected: selected.selected,
        },
        facts,
    ))
}

async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    match ctx
        .app
        .engine
        .import_place_seen(conn, principal, node)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(failed(conn, error).await),
    }
}

impl Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = ImportPreviewRequest;
    type Value = Known;
    type Output = ImportPreview;
    fn declaration(_value: &Known) -> &'static Declaration<Source> {
        &PREVIEW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportPreviewRequest,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(sources(ctx, conn, caller, body, None))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, value.node))
    }
}

impl Initial for Preview {
    type Input = Known;
    type Fetched = Fetched;
    fn declaration(_input: &Known) -> &'static Declaration<Source> {
        &PREVIEW
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ImportPreviewRequest,
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(sources(ctx, conn, caller, body, None))
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal, input.node))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        known: Known,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let origin = CapturedOperation::capture(caller).map_err(ApiError::internal)?;
            let preview = match ctx
                .app
                .engine
                .preview_import(caller, known.node, &known.request)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    let error = failed(&mut conn, error).await;
                    return Err(if native::modern(caller) {
                        authorization::problem_without_content(error)
                    } else {
                        error
                    });
                }
            };
            Ok(Fetched {
                known,
                origin,
                preview,
            })
        })
    }
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = ((), ImportPreviewRequest);
    type Value = Arc<Fetched>;
    type Reply = ImportPreview;
    const DECLARATION: &'static Declaration<Source> = &PREVIEW;
    fn origin<'a>(
        _query: &((), ImportPreviewRequest),
        value: &'a Arc<Fetched>,
    ) -> Option<&'a CapturedOperation> {
        Some(&value.origin)
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a ((), ImportPreviewRequest),
    ) -> BoxFuture<'a, ApiResult<(Arc<Fetched>, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Self>(ctx)?;
            let (_, facts) = sources(ctx, conn, caller, query.1.clone(), Some(&fetched)).await?;
            Ok((fetched, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require(ctx, conn, principal, ctx.id("id")?).await })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Arc<Fetched>,
    ) -> BoxFuture<'a, ApiResult<ImportPreview>> {
        Box::pin(async move { Ok(value.preview.clone()) })
    }
}
