//! Native stored channel namespaces, ordinary authority and the removal effect boundary.

use super::*;
use crate::key_operations::{native, targets};
use futures::future::BoxFuture;
use orch_api_contract::notification_sources::{self as sources, Source};
use orch_api_contract::operation_sources::Declaration;

pub(crate) struct StoredChannel;

pub(super) enum MutationError {
    Database(DbError),
    Before(ApiError),
}
impl From<DbError> for MutationError {
    fn from(error: DbError) -> Self {
        Self::Database(error)
    }
}
impl MutationError {
    pub(super) fn into_api(self) -> ApiError {
        match self {
            Self::Database(error) => error.into(),
            Self::Before(error) => error,
        }
    }
}

pub(super) async fn owner(
    conn: &mut Conn,
    row: &ChannelRow,
) -> ApiResult<(
    orch_access_contract::key_scope::KeyTargetKind,
    orch_access_contract::key_scope::KeyResolvedTarget,
)> {
    let target = match (row.org_id, row.user_id) {
        (Some(org), _) => targets::node(conn, org).await?,
        (None, Some(user)) => targets::account(conn, user).await?,
        (None, None) => targets::system(),
    };
    Ok((target.object.kind, target))
}

pub(super) async fn require_change_private(
    conn: &mut Conn,
    principal: &Principal,
    row: &ChannelRow,
) -> ApiResult<()> {
    require_change(conn, principal, row).await.map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}

impl native::Stored for StoredChannel {
    type Source = Source;
    type Value = ChannelRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::CHANNEL_DELETE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(ChannelRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = channel_of(conn, ctx).await?;
            let (family, owner) = owner(conn, &row).await?;
            let facts = vec![native::Facts::one(Source::ChannelOwner, family, owner)];
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a ChannelRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_change_private(conn, principal, value))
    }
}
