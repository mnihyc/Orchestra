//! One native session creation for retained uploads and explicitly targeted workspace uploads.

use super::*;
use crate::key_operations::native::effects;
use crate::key_operations::{native, targets};
use crate::routes::workspace::{door, paths};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    upload_sources::{self, Source},
};
use orch_authz::{Caller, Principal};
use orch_db::queries::uploads::targets::{self as destinations, Destination};
use orch_settings_contract::registry::keys;

pub struct Begin;
pub struct Prepared {
    name: String,
    media: Option<String>,
    size: i64,
    idle: Duration,
    to: Option<Destination>,
}

impl effects::Effect for Begin {
    type Source = Source;
    type Query = ();
    type Body = UploadSessionCreateBody;
    type Value = Prepared;
    type Output = UploadSessionCreateReply;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &upload_sources::SESSION_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut orch_db::Conn,
        caller: &'a Caller,
        (): (),
        body: UploadSessionCreateBody,
    ) -> native::SourceFuture<'a, Prepared, Source> {
        Box::pin(async move {
            let name = clean_name("name", &body.name)?;
            if body.size < 0 {
                return Err(ApiError::invalid("size", "a size is not negative"));
            }
            let media = media_type(&body)?;
            let namespace = targets::account(conn, caller.user()).await?;
            let mut facts = vec![native::Facts::one(
                Source::SessionNamespace,
                KeyTargetKind::Identity(KeyIdentityKind::Upload),
                namespace,
            )];
            let mut to = None;
            let mut selected = Vec::new();
            if let Some(asked) = &body.to {
                let path = paths::clean("to.path", &asked.path, false)?;
                let env = orch_db::queries::environments::workspace::environment(
                    conn,
                    asked.environment_id,
                )
                .await?
                .ok_or_else(ApiError::not_found)?;
                selected.push(
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: env.id,
                        },
                    )
                    .await?,
                );
                let relayed =
                    orch_engine::transfers::relay_on(conn, Place::Environment(env.id)).await;
                to = Some(Destination {
                    environment_id: env.id,
                    machine_id: env.machine_id,
                    generation: env.generation,
                    workspace_root: env.workspace_root,
                    path,
                    thread_id: asked.thread,
                    relayed,
                });
            }
            facts.push(native::Facts::collection(
                Source::SessionEnvironment,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Environment), selected)]
                },
            ));
            let plan = native::bind(ctx, &upload_sources::SESSION_CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            if let Some(to) = &to {
                check_destination(ctx, conn, &principal, to).await?;
                // This is the actual mutable location footprint, held before the first session row.
                sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                    .bind(to.environment_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            if !to.as_ref().is_some_and(|to| to.relayed) {
                let maximum =
                    orch_settings::get::<i64>(conn, Place::System, keys::STORAGE_MAX_BLOB_BYTES)
                        .await
                        .map_err(ApiError::internal)?
                        .ok_or_else(|| ApiError::internal("the stored-file default is missing"))?;
                if body.size > maximum {
                    return Err(too_large(maximum));
                }
            }
            let hours =
                orch_settings::get::<i64>(conn, Place::System, keys::STORAGE_UPLOAD_SESSION_HOURS)
                    .await
                    .map_err(ApiError::internal)?
                    .ok_or_else(|| ApiError::internal("the upload idle default is missing"))?;
            let idle = Duration::try_hours(hours).ok_or_else(|| {
                ApiError::invalid(
                    "storage.upload_session_hours",
                    "the duration cannot be represented",
                )
            })?;
            Ok((
                Prepared {
                    name,
                    media,
                    size: body.size,
                    idle,
                    to,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut orch_db::Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Some(to) = &value.to {
                check_destination(ctx, conn, principal, to).await?;
            }
            Ok(())
        })
    }
}
impl effects::ApplyOwned for Begin {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<UploadSessionCreateReply>>> {
        Box::pin(async move {
            let now = ctx.app.clock.now();
            let expires_at = now
                .checked_add_signed(value.idle)
                .unwrap_or(DateTime::<Utc>::MAX_UTC);
            let session = SessionRow {
                id: new_id(),
                user_id: principal.user(),
                name: value.name.clone(),
                media_type: value.media.clone(),
                size: value.size,
                idle_secs: value.idle.num_seconds(),
                received: 0,
                created_at: now,
                updated_at: now,
                expires_at,
            };
            before.check()?;
            storage::begin_session(&mut tx, &session).await?;
            if let Some(to) = &value.to {
                destinations::insert(&mut tx, session.id, to).await?;
            }
            orch_db::idempotency::creation::observe_with(&mut tx, Some(session.user_id), "upload_session", session.id,
                serde_json::json!({"chunk":chunk_bytes(),"expires":orch_types::Time::new(expires_at)})).await?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(UploadSessionCreateReply {
                upload_id: session.id,
                chunk_bytes: chunk_bytes(),
                expires_at: orch_types::Time::new(expires_at),
            }))
        })
    }
}

pub(super) async fn check_destination(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &Principal,
    to: &Destination,
) -> ApiResult<()> {
    let door = door::writable(
        ctx,
        conn,
        to.environment_id,
        to.thread_id,
        principal.clone(),
    )
    .await?;
    if to.relayed {
        let machine = crate::routes::workspace::live::require_live(ctx, conn, &door.env).await?;
        if Some(machine) != to.machine_id
            || door.env.generation != to.generation
            || door.env.workspace_root != to.workspace_root
        {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the environment changed before its upload was continued",
            ));
        }
    }
    Ok(())
}

fn media_type(body: &UploadSessionCreateBody) -> ApiResult<Option<String>> {
    let Some(text) = &body.media_type else {
        return Ok(None);
    };
    let text = text.trim();
    if text.len() > MEDIA_TYPE_MAX_CHARS
        || !text.split_once('/').is_some_and(|(kind, subtype)| {
            !kind.is_empty()
                && !subtype.is_empty()
                && !text.chars().any(|c| c.is_control() || c.is_whitespace())
        })
    {
        return Err(ApiError::invalid(
            "media_type",
            "this is a type and a subtype",
        ));
    }
    Ok(Some(text.to_owned()))
}
