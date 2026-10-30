//! The narrow answers allowed for a thread, as its Parameters list them, and taking one back.
//!
//! Whoever manages the thread lists them, newest first, those that no longer answer with why, and takes one back:
//! the next undecided request asks again, and an action already allowed goes on. Taking back is serialized with
//! deciding on the thread's lock.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_api_contract::approvals::{
    ThreadApprovalRule, ThreadApprovalRuleDelete, ThreadApprovalRuleList,
};
use orch_api_contract::common::{Page, PageQuery};
use orch_engine::approval_rules::RuleError;
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadApprovalRuleList, list)
        .on(ThreadApprovalRuleDelete, revoke);
}

async fn failed(ctx: &Ctx, error: RuleError) -> ApiError {
    match error {
        RuleError::NotFound => ApiError::not_found(),
        RuleError::Denied(refusal) => match ctx.app.db.acquire().await {
            Ok(mut conn) => refusals::denied(&mut conn, *refusal)
                .await
                .unwrap_or_else(|error| error),
            Err(error) => error.into(),
        },
        RuleError::Internal(error) => ApiError::engine(error),
    }
}

/// The rules of a thread, newest first, a page at a time.
async fn list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<ThreadApprovalRule>> {
    let thread = ctx.id("id")?;
    let before: Option<Id> = query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let limit = i64::from(query.limit());
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    drop(conn);
    let mut items = match ctx
        .app
        .engine
        .approval_rules(&principal, thread, before, limit + 1)
        .await
    {
        Ok(items) => items,
        Err(error) => return Err(failed(&ctx, error).await),
    };
    let more = i64::try_from(items.len()).is_ok_and(|count| count > limit);
    items.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok(Page {
        next_cursor: more
            .then(|| items.last().map(|rule| rule.id.to_string()))
            .flatten(),
        items,
    })
}

/// Takes back a rule of a thread.
async fn revoke(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let thread = ctx.id("id")?;
    let rule = ctx.id("rule_id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .revoke_approval_rule_guarded(tx, guard.into_principal(), thread, rule)
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => Err(failed(&ctx, error).await),
    }
}
