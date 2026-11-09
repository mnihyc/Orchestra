//! Stored unfinished uploads use their native identity and owner before their writer locks.

use super::check_session_for;
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::upload_sources::{self as sources, Source};
use orch_authz::Principal;
use orch_db::Conn;
use orch_db::queries::storage::{self, SessionRow};

pub(crate) struct Sessions;

impl native::Stored for Sessions {
    type Source = Source;
    type Value = SessionRow;
    const ROWS: &'static [Declaration<Source>] = &[
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
            let row = storage::session(conn, ctx.id("upload_id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            check_session_for(ctx, &row, ctx.user()?.id(), ctx.now)?;
            let target = targets::from_upload_session(conn, &row).await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Session,
                    KeyTargetKind::Identity(KeyIdentityKind::Upload),
                    target,
                )],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a SessionRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(
            async move { check_session_for(ctx, row, principal.user(), principal.now().get()) },
        )
    }
}
