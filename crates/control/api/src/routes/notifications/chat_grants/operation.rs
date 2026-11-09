//! One native compound Chat grant and optional copy, on its actual guarded transaction.

use super::*;
use crate::key_operations::native::{self, effects};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_api_contract::notification_sources::{CHAT_GRANT, Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use orch_worker_core::person_notices::{self, Operation, Subject};
use std::sync::Mutex;

pub(super) struct Grant;
pub(super) struct Value {
    pub chat: rows::ChatRow,
    pub user: Id,
    pub body: ChatPersonGrantBody,
}

async fn facts(conn: &mut Conn, chat: Id, notify: bool) -> ApiResult<Vec<native::Facts<Source>>> {
    match person_notices::operation_facts(conn, Operation::ChatGrant, Subject::Chat(chat), notify)
        .await
    {
        Ok(facts) => Ok(facts),
        Err(orch_authz::key_targets::TargetError::Missing) => Err(ApiError::not_found()),
        Err(orch_authz::key_targets::TargetError::Db(error)) => Err(error.into()),
        Err(orch_authz::key_targets::TargetError::Authz(error)) => {
            Err(refusals::problem(conn, error).await)
        }
    }
}

impl effects::Effect for Grant {
    type Source = Source;
    type Query = ();
    type Body = ChatPersonGrantBody;
    type Value = Value;
    type Output = ();
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        &CHAT_GRANT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ChatPersonGrantBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let earlier = rows::chat(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let mut known = facts(conn, id, false).await?;
            // The ordinary source check precedes the compound copy footprint.
            // An unresolved notice source is never marked empty to enter a preparer.
            known.retain(|fact| fact.source != Source::NoticeChats);
            let plan: OperationPlan = native::bind_known(ctx, &CHAT_GRANT, &known)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            if !person::reads(conn, &principal, &person::Target::Chat(earlier.clone())).await? {
                return Err(ApiError::not_found());
            }
            person::lock_sender(conn, caller.user()).await?;
            let user = ctx.id("user_id")?;
            let ready = facts(conn, id, body.notify).await?;
            let plan = native::bind_required(ctx, &CHAT_GRANT, &ready)?;
            let _current = native::preparation_principal(ctx, conn, caller, &plan).await?;
            // Ordinary node-page writers take Node before Chat; preserve that order.
            if let Some(node) = earlier.node_id
                && let Err(error) = pages::lock_change_target(conn, Target::Live(node)).await
            {
                return Err(refusals::problem(conn, error).await);
            }
            let chat = rows::lock(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if chat.node_id != earlier.node_id
                || chat.org_id != earlier.org_id
                || chat.kind != earlier.kind
            {
                return Err(DbError::Retry.into());
            }
            let facts = facts(conn, id, body.notify).await?;
            Ok((Value { chat, user, body }, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if !person::reads(conn, principal, &person::Target::Chat(value.chat.clone())).await? {
                return Err(ApiError::not_found());
            }
            // Recipient availability belongs to the native apply prefix. Completed
            // header retries return their empty acknowledgement before this writer.
            Ok(())
        })
    }
}

/// One compound apply's actual first effect, shared by grant, notice and idempotency.
pub(super) struct Boundary<'a> {
    first: Mutex<Option<effects::FirstEffect<'a>>>,
    deadline: effects::Deadline<'a>,
}
impl<'a> Boundary<'a> {
    fn new(first: effects::FirstEffect<'a>) -> Self {
        Self {
            deadline: first.deadline(),
            first: Mutex::new(Some(first)),
        }
    }
    pub fn check(&self) -> ApiResult<()> {
        self.deadline.check()?;
        if let Some(first) = self
            .first
            .lock()
            .map_err(|_| ApiError::internal("the native grant boundary is poisoned"))?
            .take()
        {
            first.check()?;
        }
        Ok(())
    }
    fn changed(&self) -> ApiResult<bool> {
        Ok(self
            .first
            .lock()
            .map_err(|_| ApiError::internal("the native grant boundary is poisoned"))?
            .is_none())
    }
}

impl effects::Apply for Grant {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<()>>> {
        Box::pin(async move {
            let boundary = Boundary::new(before);
            crate::routes::access::link_origin::check(conn, principal, value.body.link_origin)
                .await?;
            orch_db::audit::scope_link_origin(
                value.body.link_origin,
                principal.user(),
                super::grant_on(ctx, conn, principal, value, &boundary),
            )
            .await?;
            boundary.deadline.check()?;
            Ok(if boundary.changed()? {
                effects::Outcome::Applied(())
            } else {
                effects::Outcome::Unchanged(())
            })
        })
    }
}
