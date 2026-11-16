//! Native file versions and bounded memory responses for the files view.

use super::*;
use crate::key_operations::publication;
use futures::StreamExt;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_authz::{Policy, Principal};
use orch_blob_contract::range::ResolvedRange;
use orch_link_contract::commands::FsRelaySend;

#[derive(Clone)]
struct Source {
    environment: Id,
    machine: Id,
    generation: i32,
    root: String,
    thread: Option<Id>,
}

impl Source {
    async fn current(&self, conn: &mut orch_db::Conn, principal: &Principal) -> ApiResult<()> {
        let allowed = crate::middleware::authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            self.environment,
            Permission::FilesView,
            Policy::default(),
        )
        .await;
        match allowed {
            Ok(()) => {}
            Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
                crate::middleware::authorization::require_resource(
                    conn,
                    principal,
                    ResourceKind::Environment,
                    self.environment,
                    Permission::FilesManage,
                    Policy::default(),
                )
                .await?
            }
            Err(error) => return Err(error),
        }
        let row = orch_db::queries::environments::workspace::environment(conn, self.environment)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
        if row.machine_id != Some(self.machine)
            || row.generation != self.generation
            || row.workspace_root != self.root
        {
            return Err(ApiError::not_found());
        }
        if let Some(thread) = self.thread {
            door::beside(conn, principal, self.environment, thread).await?;
        }
        Ok(())
    }

    async fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let principal = ctx.operation_principal(&mut tx).await?;
        self.current(&mut tx, &principal).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

/// The source's actual content version, without storing an original file on the API.
pub(super) async fn describe(
    ctx: &Ctx,
    door: &Door,
    machine: Id,
    path: &str,
    mount: Option<String>,
) -> ApiResult<FsFetchResult> {
    live::run(
        ctx,
        machine,
        SerialKey::None,
        &orch_link_contract::commands::FsRelayDescribe {
            environment_id: door.env.id,
            path: path.to_owned(),
            mount,
        },
    )
    .await
    .map_err(|failure| failure.problem(Doing::Reading))
}

/// A selected native file; the version remains a true content hash rather than metadata identity.
pub(super) struct File {
    pub path: String,
    pub mount: Option<String>,
    pub version: BlobMeta,
    pub media: String,
}

/// Collects only the preview's configured source bound, then reuses the one shared preview decoder.
pub(super) async fn preview(
    ctx: &Ctx,
    door: Door,
    machine: Id,
    file: File,
    policy: crate::previews::Policy,
    facts: Option<orch_api_contract::workspace::FileSourceFacts>,
) -> ApiResult<Response> {
    let limit = policy.source_bytes();
    if file.version.size > limit {
        return crate::previews::unavailable(
            ctx,
            file.version.size,
            &file.media,
            crate::media::is_text_by_name(paths::name_of(&file.path)),
            true,
            policy,
            facts,
        )
        .await;
    }
    let source = Source {
        environment: door.env.id,
        machine,
        generation: door.env.generation,
        root: door.env.workspace_root.clone(),
        thread: door.thread,
    };
    let name = paths::name_of(&file.path).to_owned();
    let media = file.media.clone();
    let mut original = ctx.clone();
    original.headers.remove(header::RANGE);
    original.headers.remove(header::IF_NONE_MATCH);
    let collect = async {
        let response = content(&original, door, machine, file, false).await?;
        axum::body::to_bytes(
            response.into_body(),
            usize::try_from(limit).unwrap_or(usize::MAX),
        )
        .await
        .map_err(ApiError::internal)
    };
    let response = crate::previews::serve_bytes(ctx, collect, &name, &media, policy, facts).await?;
    checked(ctx, source, response).await
}

/// The original source is checked after queueing and again for each exposed response frame.
pub(super) async fn content(
    ctx: &Ctx,
    door: Door,
    machine: Id,
    file: File,
    download: bool,
) -> ApiResult<Response> {
    let File {
        path,
        mount,
        version: BlobMeta { hash, size },
        media,
    } = file;
    let source = Source {
        environment: door.env.id,
        machine,
        generation: door.env.generation,
        root: door.env.workspace_root.clone(),
        thread: door.thread,
    };
    source.check(ctx).await?;
    let range = range_of(&ctx.headers);
    let selected = match range {
        Some(range) => match range.resolve(size) {
            Ok(range) => range,
            Err(_) => return checked(ctx, source, unsatisfiable(size)).await,
        },
        None => ResolvedRange::whole(size),
    };

    let header = publication::frame(
        ctx,
        door.principal.caller(),
        source.clone(),
        |_, conn, principal, source| Box::pin(source.current(conn, principal)),
    )?;
    let empty = BlobRead {
        meta: BlobMeta { hash, size },
        range: selected,
        body: futures::stream::empty().boxed(),
    };
    let mut response = serve(
        empty,
        range.is_some(),
        &ctx.headers,
        &hash,
        paths::name_of(&path),
        &media,
        download,
    );
    // The native completion check owns EOF. A late changed-file failure must truncate a chunked
    // response even after its final data piece, rather than accepting only a declared byte count.
    response.headers_mut().remove(header::CONTENT_LENGTH);
    if response.status() == StatusCode::NOT_MODIFIED {
        return checked(ctx, source, response).await;
    }
    let reading = crate::relay::Read::create(ctx, selected.len).await?;
    source.check(ctx).await?;
    reading
        .issue(
            machine,
            &FsRelaySend {
                environment_id: door.env.id,
                mount,
                path,
                hash,
                offset: selected.start,
                len: selected.len,
                relay_id: reading.relay.id(),
                idle_secs: reading.options.idle_secs,
                retry_ms: reading.options.retry_ms,
            },
        )
        .await?;
    let first = reading.next().await?;
    let initial = (Some(first), reading, header.clone());
    let stream = futures::stream::try_unfold(initial, |(first, reading, header)| async move {
        let piece = match first {
            Some(piece) => piece,
            None => reading.next().await?,
        };
        Ok(piece.map(|bytes| (header.data(bytes), (None, reading, header))))
    });
    publication::streaming_response(ctx, response, header, stream)
}

/// An archive decoder sends its bounded entry through the same rendezvous as a file range.
pub(crate) async fn archive(
    ctx: &Ctx,
    door: Door,
    machine: Id,
    path: String,
    entry: String,
    preview: i64,
) -> ApiResult<Response> {
    let source = Source {
        environment: door.env.id,
        machine,
        generation: door.env.generation,
        root: door.env.workspace_root.clone(),
        thread: door.thread,
    };
    let reading = crate::relay::Read::create(ctx, preview).await?;
    source.check(ctx).await?;
    let body = orch_link_contract::commands::FsRelayArchiveRead {
        environment_id: door.env.id,
        path,
        entry: entry.clone(),
        max_bytes: preview,
        relay_id: reading.relay.id(),
        idle_secs: reading.options.idle_secs,
        retry_ms: reading.options.retry_ms,
    };
    let bytes = reading
        .collect_bounded(async {
            let result = reading.run(machine, &body).await?;
            Ok(BlobMeta {
                hash: result.blob.hash,
                size: result.blob.size,
            })
        })
        .await
        .map_err(|error| {
            if error.code() == ErrorCode::RequestTooLarge {
                error.with("limit", preview)
            } else {
                error
            }
        })?;
    source.check(ctx).await?;
    let meta = BlobMeta {
        hash: Hash::of(&bytes),
        size: i64::try_from(bytes.len()).map_err(ApiError::internal)?,
    };
    let asked = range_of(&ctx.headers);
    let range = match asked {
        Some(asked) => match asked.resolve(meta.size) {
            Ok(range) => range,
            Err(_) => return checked(ctx, source, unsatisfiable(meta.size)).await,
        },
        None => ResolvedRange::whole(meta.size),
    };
    let from = usize::try_from(range.start).map_err(ApiError::internal)?;
    let count = usize::try_from(range.len).map_err(ApiError::internal)?;
    let bytes = bytes.slice(from..from + count);
    let stream =
        futures::stream::try_unfold((Some(bytes), reading), |(bytes, reading)| async move {
            Ok(bytes.map(|bytes| (bytes, (None, reading))))
        })
        .boxed();
    let response = serve(
        BlobRead {
            meta,
            range,
            body: stream,
        },
        asked.is_some(),
        &ctx.headers,
        &meta.hash,
        paths::name_of(&entry),
        crate::media::media_type(paths::name_of(&entry), &[]),
        false,
    );
    checked(ctx, source, response).await
}

async fn checked(ctx: &Ctx, source: Source, response: Response) -> ApiResult<Response> {
    let ctx = ctx.clone();
    crate::response_body::checked(response, move || {
        let (ctx, source) = (ctx.clone(), source.clone());
        async move { source.check(&ctx).await }
    })
    .await
}
