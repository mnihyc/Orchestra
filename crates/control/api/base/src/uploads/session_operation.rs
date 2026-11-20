//! Stored unfinished uploads use their native identity and owner before their writer locks.

use super::check_session_for;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::routes::Endpoint;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::upload_sources as sources;

use orch_api_contract::upload_sources::Source;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::storage;

use orch_db::queries::storage::SessionRow;
pub struct Sessions;

impl native::Stored for Sessions {
    type Source = Source;
    type Value = SessionRow;
    const ROWS: &'static [Declaration<Source>] = &[
        sources::SESSION_GET,
        sources::SESSION_DELETE,
        sources::SESSION_COMPLETE,
        sources::SESSION_PUT,
    ];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(SessionRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("upload_id")?;
            let retained_cleanup = ctx.route.name
                == orch_api_contract::threads::UploadSessionDelete::ROUTE.name
                && orch_db::queries::uploads::targets::of(conn, id)
                    .await?
                    .is_some_and(|target| target.destination.relayed);
            let row = if retained_cleanup {
                orch_db::queries::uploads::targets::state(conn, id, false)
                    .await?
                    .map(|state| state.session)
            } else if matches!(ctx.route.name, "UploadSessionGet" | "UploadSessionComplete") {
                storage::session_with_receipt(conn, id, false).await?
            } else {
                storage::session(conn, id).await?
            }
            .ok_or_else(ApiError::not_found)?;
            check_lifetime(
                ctx,
                conn,
                &row,
                ctx.user()?.id(),
                orch_types::Time::new(ctx.app.clock.now()),
            )
            .await?;
            let target = targets::from_upload_session(conn, &row).await?;
            let mut facts = vec![native::Facts::one(
                Source::Session,
                KeyTargetKind::Identity(KeyIdentityKind::Upload),
                target,
            )];
            if !matches!(ctx.route.name, "UploadSessionGet" | "UploadSessionDelete") {
                let actual = orch_db::queries::uploads::targets::of(conn, row.id).await?;
                let selected = match actual {
                    Some(target) => vec![(
                        KeyTargetKind::Object(ObjectKind::Environment),
                        vec![
                            targets::page_object(
                                conn,
                                ObjectRef {
                                    kind: ObjectKind::Environment,
                                    id: target.destination.environment_id,
                                },
                            )
                            .await?,
                        ],
                    )],
                    None => Vec::new(),
                };
                facts.push(native::Facts::collection(
                    Source::SessionEnvironment,
                    selected,
                ));
            }
            Ok((row, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a SessionRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            check_lifetime(ctx, conn, row, principal.user(), principal.now()).await?;
            if ctx.route.name != orch_api_contract::threads::UploadSessionDelete::ROUTE.name
                && let Some(target) = orch_db::queries::uploads::targets::of(conn, row.id).await?
            {
                if ctx.route.name == orch_api_contract::threads::UploadSessionPut::ROUTE.name
                    && (target.verified_hash.is_some() || target.written_job.is_some())
                {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::Gone,
                        "this upload has already finished receiving",
                    ));
                }
                super::creation::check_destination(ctx, conn, principal, &target.destination)
                    .await?;
            }
            Ok(())
        })
    }
}

/// Idle retention does not end verification which was actually admitted before the idle cutoff.
pub(super) async fn check_lifetime(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &SessionRow,
    user: orch_types::Id,
    now: orch_types::Time,
) -> ApiResult<()> {
    match check_session_for(ctx, row, user, now.get()) {
        Ok(()) => Ok(()),
        Err(error)
            if error.code() == orch_api_contract::common::ErrorCode::Gone
                && matches!(ctx.route.name, "UploadSessionComplete" | "UploadSessionGet") =>
        {
            let state = orch_db::queries::uploads::targets::state(conn, row.id, false).await?;
            let target = orch_db::queries::uploads::targets::of(conn, row.id).await?;
            if let (Some(state), Some(target)) = (state, target)
                && orch_engine::operation_scope::upload_session::verification_active(
                    conn, &state, &target, now,
                )
                .await?
            {
                return Ok(());
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

impl native::StoredRead for Sessions {
    type Reply = orch_api_contract::threads::UploadSessionStatus;
    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        row: &'a SessionRow,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let native = orch_db::queries::uploads::targets::of(conn, row.id)
                .await?
                .filter(|target| target.destination.relayed);
            let completed = match &native {
                Some(target) => target
                    .verified()?
                    .map(|hash| super::native::verified_file(row, hash)),
                None => super::completion(row)?,
            };
            Ok(Self::Reply {
                received: row.received,
                ranges: if native.is_some() {
                    (row.received > 0)
                        .then_some(orch_api_contract::threads::UploadRange {
                            start: 0,
                            len: row.received,
                        })
                        .into_iter()
                        .collect()
                } else {
                    storage::session_ranges(conn, row.id).await?
                },
                policy: storage::session_policy(conn, row.id).await?,
                completed,
            })
        })
    }
}
