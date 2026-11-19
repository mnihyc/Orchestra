//! Native account-owned chat state, with actual selected Chat subjects before rendering.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::personal_sources::{self as sources, Source};
use orch_authz::Caller;
use orch_db::queries::chats::ChatRow;

const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

pub(crate) struct States;

impl native::Selected for States {
    type Source = Source;
    type Query = ();
    type Value = Vec<ChatRow>;
    type Reply = Vec<ChatState>;
    const DECLARATION: &'static Declaration<Source> = &sources::CHAT_STATES;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Vec<ChatRow>, Vec<native::Facts<Source>>)>> {
        Box::pin(Self::select_for(
            ctx,
            conn,
            caller,
            query,
            Self::DECLARATION,
        ))
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the chat state reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Vec<ChatRow>,
    ) -> BoxFuture<'a, ApiResult<Vec<ChatState>>> {
        Box::pin(async move {
            Ok(super::prepare_state_rows(ctx, conn, principal, value)
                .await?
                .into_iter()
                .map(|row| row.state)
                .collect())
        })
    }
}

impl States {
    /// The original native readable-chat and own-state selector, also used by shell composition.
    pub(crate) async fn select_for(
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        _query: &(),
        declaration: &'static Declaration<Source>,
    ) -> ApiResult<(Vec<ChatRow>, Vec<native::Facts<Source>>)> {
        let rows = super::state_candidates(conn, caller.user(), ctx.now).await?;
        if !native::modern(caller) {
            return Ok((rows, Vec::new()));
        }
        let own = native::Facts::one(
            Source::Account,
            ACCOUNT,
            targets::account(conn, caller.user()).await?,
        );
        let subjects = targets::from_chat_rows(conn, &rows).await?;
        let plan = OperationPlan::new(ctx.route, declaration.operation)?;
        let mut candidates = Vec::new();
        let mut proposed = Vec::new();
        for row in rows {
            let Some(target) = subjects.get(&row.id) else {
                continue;
            };
            if native::permits_usage(
                &plan,
                declaration,
                caller,
                &Source::Chats,
                crate::routes::personal::native::source_usage(declaration, Source::Chats)?,
                CHAT,
                target,
            )? {
                candidates.push(row);
                proposed.push(target.clone());
            }
        }
        let proposed = if proposed.is_empty() {
            Vec::new()
        } else {
            vec![(CHAT, proposed)]
        };
        let known_facts = [
            own.clone(),
            native::Facts::collection(Source::Chats, proposed),
        ];
        let known = native::bind_known(ctx, declaration, &known_facts)?;
        let principal = known.preparation_principal(ctx, conn, caller).await?;
        let loaded = super::prepare_state_rows(ctx, conn, &principal, &candidates).await?;
        let mut selected = Vec::new();
        let mut facts = Vec::new();
        for native in loaded {
            facts.push(
                subjects
                    .get(&native.chat.id)
                    .ok_or_else(ApiError::not_found)?
                    .clone(),
            );
            selected.push(native.chat);
        }
        let groups = if facts.is_empty() {
            Vec::new()
        } else {
            vec![(CHAT, facts)]
        };
        Ok((
            selected,
            vec![own, native::Facts::collection(Source::Chats, groups)],
        ))
    }
}
