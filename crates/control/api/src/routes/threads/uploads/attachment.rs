//! One native attachment producer for direct bytes and selected existing-byte witnesses.
use super::{ATTACHMENTS_DIR, filename_of};
use crate::attachments;
use crate::key_operations::{blob_sources, native, targets};
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::Ctx;
use crate::uploads;
use axum::body::Body;
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    common::{ErrorCode, MAX_BODY_BYTES, MAX_JSON_BYTES},
    operation_sources::Declaration,
    thread_sources::{self as sources, Source},
    threads::{FILENAME_HEADER, ThreadAttachmentBlobBody, ThreadAttachmentCreateReply},
};
use orch_authz::{Caller, Principal};
use orch_blob_contract::range::BlobMeta;
use orch_chat::following::BlobOrigin;
use orch_db::{Conn, DbError};
use orch_types::{Hash, Id};
use std::sync::Arc;

pub(super) struct Attachment;
pub(super) fn raw_creation()
-> crate::middleware::idempotency::RawCreation<(), ThreadAttachmentCreateReply> {
    use crate::middleware::idempotency::{RawCreation, RawFormat, RawInputSpec};
    RawCreation {
        kind: "thread_attachment",
        input: |ctx| {
            Box::pin(async move {
                let name = filename_of(ctx);
                let direct = name.is_some();
                Ok(RawInputSpec {
                    limit: i64::try_from(if direct {
                        MAX_BODY_BYTES
                    } else {
                        MAX_JSON_BYTES
                    })
                    .unwrap_or(i64::MAX),
                    format: if direct {
                        RawFormat::Bytes
                    } else {
                        RawFormat::Json
                    },
                    fields: serde_json::json!({"filename":name}),
                })
            })
        },
        replay: |ctx, (), incoming, created| {
            Box::pin(async move {
                if created.id != ctx.id("id")? {
                    return Err(ApiError::not_found());
                }
                let path = created
                    .data
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        ApiError::internal("the accepted attachment has no reserved path")
                    })?
                    .to_owned();
                let name = path
                    .strip_prefix(&format!("{ATTACHMENTS_DIR}/"))
                    .ok_or_else(|| {
                        ApiError::internal(
                            "the accepted attachment path is outside its native directory",
                        )
                    })?
                    .to_owned();
                let blob =
                    serde_json::from_value(created.data.get("blob").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted attachment has no native blob")
                    })?)
                    .map_err(ApiError::internal)?;
                if let Some(json) = incoming.json() {
                    let supplied: ThreadAttachmentBlobBody = serde_json::from_value(json.clone())
                        .map_err(|_| {
                        ApiError::invalid("body", "the supplied attachment reference is invalid")
                    })?;
                    if supplied.blob != blob {
                        return Err(ApiError::internal(
                            "the native reference differs from the accepted attachment",
                        ));
                    }
                } else if incoming.meta().hash != blob {
                    return Err(ApiError::internal(
                        "the native body differs from the accepted attachment",
                    ));
                }
                let size = created
                    .data
                    .get("size")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| {
                        ApiError::internal("the accepted attachment has no native byte count")
                    })?;
                let reply = ThreadAttachmentCreateReply {
                    path,
                    name,
                    blob,
                    size,
                };
                crate::key_operations::publication::receipt(&ctx, &reply)?;
                Ok(reply)
            })
        },
    }
}
pub(super) struct Known {
    name: Option<String>,
}
pub(super) struct Input {
    name: String,
    replace: Option<String>,
    bytes: Sent,
}
#[derive(Clone, Copy)]
enum Sent {
    Direct(BlobMeta),
    Reference(Hash),
}
impl Input {
    fn direct(&self) -> bool {
        matches!(self.bytes, Sent::Direct(_))
    }
    fn hash(&self) -> Hash {
        match self.bytes {
            Sent::Direct(meta) => meta.hash,
            Sent::Reference(hash) => hash,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Witness {
    thread: Id,
    hash: Hash,
    selected: Vec<(Hash, BlobOrigin)>,
}
pub(super) struct Prepared {
    input: Arc<Input>,
    witness: Witness,
}

fn declaration(direct: bool) -> &'static Declaration<Source> {
    if direct {
        &sources::ATTACHMENT_DIRECT
    } else {
        &sources::ATTACHMENT_REFERENCED
    }
}
fn byte_facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    use blob_sources::SourceKind;
    [
        (Source::AttachmentChats, SourceKind::Chat),
        (Source::AttachmentThreads, SourceKind::Thread),
        (Source::AttachmentDocuments, SourceKind::Document),
        (Source::AttachmentEnvironments, SourceKind::Environment),
        (Source::AttachmentUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(source, kind)| {
        let values: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect();
        native::Facts::collection(
            source,
            if values.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), values)]
            },
        )
    })
    .collect()
}
async fn primary(ctx: &Ctx, conn: &mut Conn) -> ApiResult<native::Facts<Source>> {
    let target = targets::node(conn, ctx.id("id")?).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(
        Source::Thread,
        target.object.kind,
        target,
    ))
}
async fn ordinary(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    authorization::require_on_path_in(ctx, conn, principal, "thread.interact").await
}
impl native::raw::Staged for Attachment {
    type Known = Known;
    type Input = Input;
    fn known_declaration(known: &Known) -> &'static Declaration<Source> {
        declaration(known.name.is_some())
    }
    fn known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Known, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            Ok((
                Known {
                    name: filename_of(ctx),
                },
                vec![primary(ctx, conn).await?],
            ))
        })
    }
    fn require_known<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _known: &'a Known,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(ordinary(ctx, conn, principal))
    }
    fn take<'a>(
        ctx: &'a Ctx,
        _caller: &'a Caller,
        known: Known,
        body: Body,
    ) -> BoxFuture<'a, ApiResult<Input>> {
        Box::pin(async move {
            if let Some(name) = known.name {
                let name = uploads::clean_name(FILENAME_HEADER, &name)?;
                let meta = ctx
                    .app
                    .blobs
                    .store_stream(
                        body.into_data_stream(),
                        i64::try_from(MAX_BODY_BYTES).unwrap_or(i64::MAX),
                    )
                    .await
                    .map_err(uploads::stored_badly)?;
                return Ok(Input {
                    name,
                    replace: None,
                    bytes: Sent::Direct(meta),
                });
            }
            let bytes = axum::body::to_bytes(body, MAX_JSON_BYTES)
                .await
                .map_err(|_| ApiError::new(ErrorCode::RequestTooLarge, "the body is too large"))?;
            let named: ThreadAttachmentBlobBody = serde_json::from_slice(&bytes).map_err(|_| ApiError::invalid("", "a file is sent as the body with its name in the header, or as the hash of an upload and a name"))?;
            Ok(Input {
                name: uploads::clean_name("name", &named.name)?,
                replace: named.replace,
                bytes: Sent::Reference(named.blob),
            })
        })
    }
}
impl native::effects::Effect for Attachment {
    type Source = Source;
    type Query = ();
    type Body = Arc<Input>;
    type Value = Prepared;
    type Output = ThreadAttachmentCreateReply;
    fn declaration(value: &Prepared) -> &'static Declaration<Source> {
        declaration(value.input.direct())
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        input: Arc<Input>,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let hash = input.hash();
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                if !orch_db::queries::transaction_guard::lock_parent(conn, thread).await? {
                    return Err(ApiError::not_found());
                }
                orch_db::queries::threads::lock(conn, thread)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                orch_db::queries::uploads::lock_blobs(conn, &[hash]).await?;
                // Preserve the native cleanup/replace footprint before the fresh action guard.
                sqlx::query("SELECT thread_id,path FROM thread_attachment_uploads WHERE expires_at<$1 ORDER BY thread_id,path FOR UPDATE").bind(ctx.now).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                if let Some(path) = &input.replace {
                    sqlx::query("SELECT path FROM thread_attachment_uploads WHERE thread_id=$1 AND path=$2 AND created_by=$3 FOR UPDATE").bind(thread).bind(path).bind(caller.user()).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
                }
            }
            let declaration = declaration(input.direct());
            let primary = primary(ctx, conn).await?;
            let base = native::bind_known(ctx, declaration, std::slice::from_ref(&primary))?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            ordinary(ctx, conn, &principal).await?;
            let frozen = ctx.optional_retained_source::<Witness>()?;
            if frozen
                .as_ref()
                .is_some_and(|old| old.thread != thread || old.hash != hash)
            {
                return Err(ApiError::internal(
                    "the attachment changed its opened source",
                ));
            }
            let mut facts = vec![primary];
            let selected = if input.direct() {
                Vec::new()
            } else {
                let prepared = match frozen.as_deref() {
                    Some(witness) => {
                        blob_sources::prepare_selected(
                            ctx,
                            conn,
                            principal.caller(),
                            &base,
                            &[hash],
                            &[],
                            &witness.selected,
                        )
                        .await?
                    }
                    None => {
                        blob_sources::prepare(ctx, conn, principal.caller(), &base, &[hash], &[])
                            .await?
                    }
                };
                facts.extend(byte_facts(&prepared.bindings));
                prepared.selected
            };
            let witness = Witness {
                thread,
                hash,
                selected,
            };
            if frozen.as_ref().is_some_and(|old| old.as_ref() != &witness) {
                return Err(ApiError::not_found());
            }
            Ok((Prepared { input, witness }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(ordinary(ctx, conn, principal))
    }
}
impl native::effects::ApplyOwned for Attachment {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        _principal: &'a Principal,
        value: &'a Prepared,
        before: native::effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<native::effects::Outcome<ThreadAttachmentCreateReply>>> {
        Box::pin(async move {
            ctx.retain_source(value.witness.clone())?;
            let input = &value.input;
            let meta = match input.bytes {
                Sent::Direct(meta) => meta,
                Sent::Reference(hash) => BlobMeta {
                    hash,
                    size: ctx
                        .app
                        .blobs
                        .store()
                        .size(&hash)
                        .await
                        .map_err(ApiError::internal)?
                        .ok_or_else(ApiError::not_found)?,
                },
            };
            let builtin =
                orch_db::queries::ask::program_root(&mut tx, value.witness.thread).await?;
            let described = if let Some(root) = builtin {
                let limit = orch_settings::get::<i64>(
                    &mut tx,
                    orch_settings::Place::Node(root),
                    "chat.attachment_max_bytes",
                )
                .await
                .map_err(ApiError::internal)?
                .unwrap_or(i64::MAX);
                if meta.size > limit {
                    return Err(ApiError::new(
                        ErrorCode::RequestTooLarge,
                        "The file exceeds this conversation's attachment limit",
                    )
                    .with("limit", limit));
                }
                Some(attachments::describe(ctx, &meta, &input.name).await?)
            } else {
                None
            };
            // Reuse the guarded producer with the original selected witness after supplier IO.
            let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
            ordinary(&current, &mut tx, guard.principal()).await?;
            guard.check_deadline(&current)?;
            before.check()?;
            sqlx::query("DELETE FROM thread_attachment_uploads WHERE expires_at<$1")
                .bind(current.now)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
            attachments::retain(
                &current,
                &mut tx,
                guard.principal(),
                &attachments::Taken {
                    meta,
                    name: input.name.clone(),
                    supplied_bytes: input.direct(),
                },
            )
            .await?;
            let owned: Option<String> = if let Some(path) = &input.replace {
                if !path.starts_with(".orch/attachments/") {
                    return Err(ApiError::invalid(
                        "replace",
                        "an edit names its private attachment",
                    ));
                }
                sqlx::query_scalar("SELECT name FROM thread_attachment_uploads WHERE thread_id=$1 AND path=$2 AND created_by=$3 AND blob IS NOT NULL FOR UPDATE").bind(value.witness.thread).bind(path).bind(guard.user().id()).fetch_optional(&mut *tx).await.map_err(DbError::from)?
            } else {
                None
            };
            let name = if let Some(name) = owned {
                sqlx::query("UPDATE thread_attachment_uploads SET blob=$3,expires_at=$4 WHERE thread_id=$1 AND path=$2").bind(value.witness.thread).bind(&input.replace).bind(meta.hash.as_bytes().as_slice()).bind(current.now+uploads::SESSION_LIFE).execute(&mut *tx).await.map_err(DbError::from)?;
                name
            } else {
                let used = orch_engine::paste::attachment_names(&mut tx, value.witness.thread)
                    .await
                    .map_err(ApiError::internal)?;
                let name = orch_engine::paste::available_name(
                    &crate::media::safe_name(&input.name),
                    &used,
                );
                sqlx::query("INSERT INTO thread_attachment_uploads(thread_id,path,name,created_by,blob,expires_at) VALUES($1,$2,$3,$4,$5,$6)").bind(value.witness.thread).bind(format!("{ATTACHMENTS_DIR}/{name}")).bind(&name).bind(guard.user().id()).bind(meta.hash.as_bytes().as_slice()).bind(current.now+uploads::SESSION_LIFE).execute(&mut *tx).await.map_err(DbError::from)?;
                name
            };
            if let Some(described) = described {
                orch_db::queries::ask::attach(
                    &mut tx,
                    value.witness.thread,
                    &meta.hash,
                    &name,
                    &described.media_type,
                    meta.size,
                    current.now,
                )
                .await?;
            }
            orch_db::idempotency::creation::observe_with(&mut tx,Some(guard.user().id()),"thread_attachment",value.witness.thread,
                serde_json::json!({"path":format!("{ATTACHMENTS_DIR}/{name}"),"blob":meta.hash,"size":meta.size})).await?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(native::effects::Outcome::Applied(
                ThreadAttachmentCreateReply {
                    path: format!("{ATTACHMENTS_DIR}/{name}"),
                    name,
                    size: meta.size,
                    blob: meta.hash,
                },
            ))
        })
    }
}
