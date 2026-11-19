//! Bounded key history through the existing ownership, org-management and operation doors.

use crate::key_operations::targets;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationPlan, Registry};
use chrono::{DateTime, Utc};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::{Permission, PermissionName};
use orch_api_contract::auth::{
    ApiTokenActivity, ApiTokenActivityQuery, MeTokenActivityList, OrgTokenActivityList,
};
use orch_api_contract::common::{DEFAULT_LIMIT, Page};
use orch_authz::{Act, Caller, Policy};
use orch_db::queries::key_activity;
use orch_types::Id;

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(MeTokenActivityList, personal_operation, personal)
        .on_with_operation(OrgTokenActivityList, org_operation, organization);
}

/// The same resolver runs at the shared preparation/current-admission boundary.
/// It proves the retained key/account identity, not an owner hint or legacy node scope.
fn personal_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    caller: &'a Caller,
    _query: ApiTokenActivityQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let target = targets::retained_key_of_account(conn, ctx.id("id")?, caller.user()).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, KeyTargetKind::Identity(KeyIdentityKind::Key), &[target])?;
        Ok(plan)
    })
}

/// The historical org slice is governed by that real org, not the key's owner
/// memberships or its legacy node restriction. Ordinary Members Manage remains separate.
fn org_operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    _caller: &'a Caller,
    _query: ApiTokenActivityQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let target = targets::node(conn, ctx.id("id")?).await?;
        if target.object.kind
            != KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org)
        {
            return Err(ApiError::not_found());
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, KeyTargetKind::Identity(KeyIdentityKind::Key), &[target])?;
        Ok(plan)
    })
}

/// Bound to the actual key and org slice. The watermark is private bookkeeping,
/// so reads that log themselves cannot introduce new rows into subsequent pages.
struct Cursor {
    snapshot: i64,
    after: (DateTime<Utc>, Id),
}

impl Cursor {
    fn parse(text: &str, token: Id, org: Option<Id>) -> ApiResult<Self> {
        let bad = || ApiError::invalid("cursor", "is not a cursor of this list");
        let parts: Vec<_> = text.split('|').collect();
        if parts.len() != 6
            || parts[0] != "1"
            || parts[1] != token.to_string()
            || parts[2] != org.map_or_else(|| "-".to_owned(), |id| id.to_string())
        {
            return Err(bad());
        }
        let snapshot = parts[3].parse::<i64>().map_err(|_| bad())?;
        if snapshot < 0 {
            return Err(bad());
        }
        let at = parts[4]
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?;
        let id = parts[5].parse().map_err(|_| bad())?;
        Ok(Self {
            snapshot,
            after: (at, id),
        })
    }

    fn text(&self, token: Id, org: Option<Id>) -> String {
        format!(
            "1|{token}|{}|{}|{}|{}",
            org.map_or_else(|| "-".to_owned(), |id| id.to_string()),
            self.snapshot,
            self.after.0.timestamp_micros(),
            self.after.1,
        )
    }
}

async fn personal(
    ctx: Ctx,
    query: ApiTokenActivityQuery,
    (): (),
) -> ApiResult<Page<ApiTokenActivity>> {
    let token = ctx.id("id")?;
    let account = ctx.user()?.id();
    let mut conn = ctx.app.db.acquire().await?;
    // The ordinary owner door applies to every authentication form, independently
    // of whether this credential has operation restrictions.
    if !key_activity::owned(&mut conn, token, account).await? {
        return Err(ApiError::not_found());
    }
    let principal = ctx.operation_principal(&mut conn).await?;
    recent(&ctx, &mut conn, &principal, token, None, query, true).await
}

async fn organization(
    ctx: Ctx,
    query: ApiTokenActivityQuery,
    (): (),
) -> ApiResult<Page<ApiTokenActivity>> {
    let org = ctx.id("id")?;
    let token = ctx.id("token_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    authorization::require(
        &mut conn,
        &principal,
        org,
        PermissionName::Exact(Permission::MembersManage),
        Act::Use,
        Policy::default(),
    )
    .await?;
    // IP has the existing org audit privacy boundary, independent of members management.
    let ip = !orch_authz::nodes::holding(
        &mut conn,
        &principal,
        &[org],
        PermissionName::Exact(Permission::AuditView),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_empty();
    recent(&ctx, &mut conn, &principal, token, Some(org), query, ip).await
}

async fn recent(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    token: Id,
    org: Option<Id>,
    query: ApiTokenActivityQuery,
    show_ip: bool,
) -> ApiResult<Page<ApiTokenActivity>> {
    let maximum = u32::try_from(key_activity::KEPT_CALLS).map_err(ApiError::internal)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, maximum);
    let cursor = query
        .cursor
        .as_deref()
        .map(|text| Cursor::parse(text, token, org))
        .transpose()?;
    let snapshot = match &cursor {
        Some(cursor) => cursor.snapshot,
        None => key_activity::snapshot(conn, token).await?,
    };
    let mut rows = key_activity::page(
        conn,
        token,
        org,
        ctx.app.clock.now(),
        snapshot,
        cursor.map(|cursor| cursor.after),
        i64::from(limit) + 1,
    )
    .await?;
    let size = usize::try_from(limit).map_err(ApiError::internal)?;
    let more = rows.len() > size;
    rows.truncate(size);
    let next_cursor = if more {
        rows.last().map(|row| {
            Cursor {
                snapshot,
                after: (row.at, row.id),
            }
            .text(token, org)
        })
    } else {
        None
    };
    let items =
        crate::key_operations::activity_store::project(conn, principal, rows, org, show_ip).await?;
    Ok(Page { items, next_cursor })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;

    #[test]
    fn history_cursors_are_bound_to_the_actual_key_and_org_slice() {
        let key = orch_types::new_id();
        let org = orch_types::new_id();
        let at = DateTime::from_timestamp_micros(1_000).expect("a valid timestamp");
        let cursor = Cursor {
            snapshot: 7,
            after: (at, orch_types::new_id()),
        };
        let text = cursor.text(key, Some(org));
        let parsed = Cursor::parse(&text, key, Some(org)).expect("its own cursor");
        assert_eq!(parsed.snapshot, 7);
        assert_eq!(parsed.after, cursor.after);
        assert!(Cursor::parse(&text, orch_types::new_id(), Some(org)).is_err());
        assert!(Cursor::parse(&text, key, None).is_err());
        assert!(Cursor::parse(&text, key, Some(orch_types::new_id())).is_err());
        assert!(Cursor::parse("1|bad", key, None).is_err());
    }
}
