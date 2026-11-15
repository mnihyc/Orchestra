//! Native file versions and bounded memory responses for the files view.

use super::*;
use crate::key_operations::publication;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_authz::{Policy, Principal};
use orch_blob_contract::range::ResolvedRange;
use orch_link_contract::commands::FsRelaySend;
use std::sync::{Arc, Mutex, PoisonError};

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
pub(super) async fn preview(ctx: &Ctx, door: Door, machine: Id, file: File) -> ApiResult<Response> {
    let limit = crate::previews::source_limit(ctx).await?;
    if file.version.size > limit {
        return crate::previews::unavailable(
            ctx,
            file.version.size,
            &file.media,
            crate::media::is_text_by_name(paths::name_of(&file.path)),
            true,
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
    let response = crate::previews::serve_bytes(ctx, collect, &name, &media).await?;
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
    let (concurrency, idle_secs, retry_ms, io_secs) = {
        let mut conn = ctx.app.db.acquire().await?;
        // Read one resolved setting at a time on the existing connection; none of these are file caps.
        let concurrency = orch_settings::get::<u64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_RELAY_CONCURRENCY,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the relay concurrency default is missing"))?;
        let idle = orch_settings::get::<u64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_RELAY_IDLE_SECS,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the relay progress allowance is missing"))?;
        let retry = orch_settings::get::<u64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_RELAY_RETRY_MS,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the relay retry default is missing"))?;
        let io = orch_settings::get::<u64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::JOB_IO_TIMEOUT_SECS,
        )
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("the file-work allowance is missing"))?;
        (concurrency, idle, retry, io)
    };
    let relay = ctx
        .app
        .blobs
        .relays()
        .create(
            usize::try_from(concurrency).unwrap_or(usize::MAX),
            u64::try_from(selected.len).map_err(ApiError::internal)?,
        )
        .await;
    source.check(ctx).await?;
    let dispatcher = ctx.app.link.dispatcher();
    let id = orch_types::new_id();
    let until = i64::try_from(io_secs)
        .ok()
        .and_then(ChronoDuration::try_seconds)
        .and_then(|wait| ctx.app.clock.now().checked_add_signed(wait))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
    let cancellation = Cancel {
        relay: Arc::clone(&relay),
        dispatcher: dispatcher.clone(),
        id,
    };
    dispatcher
        .issue_as(
            id,
            machine,
            &SerialKey::None,
            Time::new(until),
            &FsRelaySend {
                environment_id: door.env.id,
                mount,
                path,
                hash,
                offset: selected.start,
                len: selected.len,
                relay_id: relay.id(),
                idle_secs,
                retry_ms,
            },
        )
        .await
        .map_err(ApiError::internal)?;
    let failure = Arc::new(Mutex::new(None));
    let (waiting, failed, finished) =
        (dispatcher.clone(), Arc::clone(&failure), Arc::clone(&relay));
    let _task = tokio::spawn(async move {
        match waiting.wait_for::<FsRelaySend>(id).await {
            Ok(_) => finished.finish(),
            Err(error) => {
                *failed.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(live::command_failure(error).problem(Doing::Reading));
                finished.cancel();
            }
        }
    });
    let first = relay.next().await.map_err(|_| problem(&failure))?;
    let initial = (Some(first), relay, header.clone(), failure, cancellation);
    let stream = futures::stream::try_unfold(
        initial,
        |(first, relay, header, failure, cancellation)| async move {
            let piece = match first {
                Some(piece) => piece,
                None => relay.next().await.map_err(|_| problem(&failure))?,
            };
            Ok(piece.map(|bytes| {
                (
                    header.data(bytes),
                    (None, relay, header, failure, cancellation),
                )
            }))
        },
    );
    publication::streaming_response(ctx, response, header, stream)
}

async fn checked(ctx: &Ctx, source: Source, response: Response) -> ApiResult<Response> {
    let ctx = ctx.clone();
    crate::response_body::checked(response, move || {
        let (ctx, source) = (ctx.clone(), source.clone());
        async move { source.check(&ctx).await }
    })
    .await
}

fn problem(failure: &Arc<Mutex<Option<ApiError>>>) -> ApiError {
    failure
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| {
            ApiError::new(
                ErrorCode::EnvironmentUnavailable,
                "the file relay ended before its native source completed",
            )
        })
}

struct Cancel {
    relay: Arc<orch_api_base::relay::Relay>,
    dispatcher: crate::dispatch::Dispatcher,
    id: Id,
}
impl Drop for Cancel {
    fn drop(&mut self) {
        self.relay.cancel();
        let (dispatcher, id) = (self.dispatcher.clone(), self.id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = dispatcher.cancel(id).await;
            });
        }
    }
}
