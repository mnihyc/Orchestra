//! Native locked archive opening, real first-download audit and continuing byte disclosure.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, ApplyOwned, Effect, FirstEffect, Outcome},
    },
    publication,
};
use futures::{StreamExt as _, future::BoxFuture};
use orch_api_contract::{
    artifact_sources::{EXPORT_DOWNLOAD, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, engine::Graph};
use orch_blob_contract::store::BlobRead;
use orch_db::{DbError, Tx};
use orch_engine::{
    exports::{
        PreparedExportOpen,
        download::{Archive, Input},
    },
    operation_scope::continuing::{Error as NativeError, Source as NativeSource},
};
use std::collections::BTreeSet;

pub(super) struct Open;

pub(super) struct Prepared {
    native: PreparedExportOpen,
    label: Option<String>,
    named: Option<Id>,
}

pub(super) struct Opened {
    prepared: Prepared,
    read: BlobRead,
}

fn native_error(error: NativeError) -> ApiError {
    match error {
        NativeError::Refused
        | NativeError::Binding(orch_engine::operation_scope::BindingError::Refused) => {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original archive source is no longer readable",
            )
        }
        NativeError::Binding(error) => ApiError::internal(error),
        NativeError::Native(error) => crate::key_operations::snapshot_read::engine_error(error),
    }
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    input: &Input,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let (value, mut facts) = Archive::prepare(conn, caller, input, ctx.app.clock.time())
        .await
        .map_err(native_error)?;
    let known = native::bind_known(ctx, &EXPORT_DOWNLOAD, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    Archive::require(conn, &principal, input, &value)
        .await
        .map_err(native_error)?;
    let label = match value.row().node_id {
        Some(node) => orch_db::queries::threads::node_name(conn, node).await?,
        None => None,
    };
    let candidates = if label.is_some() {
        value.row().node_id.into_iter().collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let graph = Graph::load(conn, &candidates.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (labels, allowed) = native::node_name_facts(
        &known,
        &EXPORT_DOWNLOAD,
        principal.caller(),
        &graph,
        &candidates,
        &read_sources::NODE_SOURCES,
    )?;
    let named = value.row().node_id.filter(|node| allowed.contains(node));
    facts.extend(labels);
    Ok((
        Prepared {
            native: value,
            label: label.filter(|_| named.is_some()),
            named,
        },
        facts,
    ))
}

impl Effect for Open {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Prepared;
    type Output = Opened;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &EXPORT_DOWNLOAD
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let selected = match ctx
                .app
                .engine
                .prepare_export_open(conn, caller.user(), ctx.id("id")?)
                .await
            {
                Ok(value) => value,
                Err(error) => return Err(failed(conn, error).await),
            };
            prepare(ctx, conn, caller, &Input::capture(&selected)).await
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            Archive::require(
                conn,
                principal,
                &Input::capture(&value.native),
                &value.native,
            )
            .await
            .map_err(native_error)
        })
    }
}

impl ApplyOwned for Open {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: Tx,
        _principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Opened>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            deadline.check()?;
            let read = match ctx.app.engine.read_prepared_export(&value.native).await {
                Ok(read) => read,
                Err(error) => return Err(failed(&mut tx, error).await),
            };
            deadline.check()?;
            if read.meta.hash != value.native.hash() {
                return Err(ApiError::internal(
                    "the opened archive does not match its stored content identity",
                ));
            }
            before.check()?;
            let changed = match ctx
                .app
                .engine
                .record_export_open(&mut tx, &value.native, &origin(ctx), ctx.now)
                .await
            {
                Ok(changed) => changed,
                Err(error) => return Err(failed(&mut tx, error).await),
            };
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            let output = Opened {
                prepared: Prepared {
                    native: value.native.clone(),
                    label: value.label.clone(),
                    named: value.named,
                },
                read,
            };
            Ok(if changed {
                Outcome::Applied(output)
            } else {
                Outcome::Unchanged(output)
            })
        })
    }
}

async fn check(ctx: &Ctx, conn: &mut Conn, reader: &Principal, opened: &Prepared) -> ApiResult<()> {
    let input = Input::capture(&opened.native);
    let (fresh, facts) = prepare(ctx, conn, reader.caller(), &input).await?;
    let same = opened.native.preserves(&fresh.native)
        && opened.named.is_none_or(|named| fresh.named == Some(named));
    let plan = native::bind(ctx, &EXPORT_DOWNLOAD, &facts)?;
    let value = fresh.native;
    plan.read_operation(
        ctx,
        conn,
        reader.caller(),
        move |conn, principal| -> BoxFuture<'_, ApiResult<()>> {
            Box::pin(async move {
                Archive::require(conn, principal, &input, &value)
                    .await
                    .map_err(native_error)?;
                if !same {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the original archive source changed",
                    ));
                }
                Ok(())
            })
        },
    )
    .await?
    .ok_or_else(|| ApiError::new(ErrorCode::AuthzDenied, "the archive has no current read"))
}

pub(super) async fn download(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let (tx, _guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Open>(&ctx, tx).await?;
    let Opened { prepared, read } = completed.output;
    let name =
        orch_engine::exports::archive_file_name(prepared.native.row(), prepared.label.as_deref());
    let frame = publication::frame(
        &completed.ctx,
        &completed.accepted,
        prepared,
        |ctx, conn, reader, opened| Box::pin(check(ctx, conn, reader, opened)),
    )?;
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_DISPOSITION, attachment(&name))
        .header(header::CONTENT_LENGTH, read.meta.size)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "private, no-store")
        .body(Body::empty())
        .map_err(ApiError::internal)?;
    let body_frame = frame.clone();
    let stream = read.body.map(move |piece| {
        piece
            .map(|bytes| body_frame.data(bytes))
            .map_err(ApiError::internal)
    });
    publication::streaming_response(&completed.ctx, response, frame, stream)
}
