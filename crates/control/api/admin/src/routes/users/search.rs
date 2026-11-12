//! Native directory search, with account scope filtering before its original result limit.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::registry::OperationPlan;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::account_sources as sources;

use orch_api_contract::account_sources::Source;

use orch_api_contract::operation_sources::Declaration;
const ACCOUNT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Account);

pub(super) struct Accounts;

pub(super) struct Selected {
    rows: Vec<identity::UserSearchRow>,
    org: Option<Id>,
}

async fn require_org(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    org: Option<Id>,
) -> ApiResult<()> {
    if let Some(org) = org {
        let (_, kind) = placed(conn, org).await?;
        if kind != NodeKind::Org {
            return Err(invalid("org_id", "is no org"));
        }
        authorization::require_projected(
            conn,
            principal,
            org,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
            native::modern(principal.caller()),
        )
        .await?;
    }
    let _ = ctx;
    Ok(())
}

impl native::Selected for Accounts {
    type Source = Source;
    type Query = UserSearchQuery;
    type Value = Selected;
    type Reply = Vec<UserRef>;
    const DECLARATION: &'static Declaration<Source> = &sources::USER_SEARCH;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a UserSearchQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let text = query.q.trim();
            if text.chars().count() < usize::try_from(USER_SEARCH_MIN_CHARS).unwrap_or(2) {
                return Err(invalid(
                    "q",
                    &format!("must be at least {USER_SEARCH_MIN_CHARS} characters"),
                ));
            }
            if let Some(org) = query.org_id
                && placed(conn, org).await?.1 != NodeKind::Org
            {
                return Err(invalid("org_id", "is no org"));
            }
            let limit = usize::try_from(USER_SEARCH_MAX_RESULTS).map_err(ApiError::internal)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            if !native::modern(caller) {
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                require_org(ctx, conn, &principal, query.org_id).await?;
            }
            let mut after: Option<(String, Id)> = None;
            let mut selected = Vec::new();
            let mut selected_targets = Vec::new();
            loop {
                let rows = identity::search_user_rows(
                    conn,
                    caller.user(),
                    text,
                    query.org_id,
                    i64::try_from(limit).map_err(ApiError::internal)?,
                    after.as_ref().map(|(name, id)| (name.as_str(), *id)),
                )
                .await?;
                let exhausted = rows.len() < limit;
                let next = rows
                    .last()
                    .map(|row| (row.user.username.clone(), row.user.id));
                if !native::modern(caller) {
                    return Ok((
                        Selected {
                            rows,
                            org: query.org_id,
                        },
                        Vec::new(),
                    ));
                }
                let identities = rows
                    .iter()
                    .map(|row| (row.user.id, row.user.kind.as_str(), row.service_org_id))
                    .collect::<Vec<_>>();
                let accounts = targets::account_identities(conn, &identities).await?;
                let mut batch = Vec::new();
                let mut facts = Vec::new();
                for row in rows {
                    let target = accounts.get(&row.user.id).ok_or_else(ApiError::not_found)?;
                    if native::permits(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::SearchAccounts,
                        ACCOUNT,
                        target,
                    )? {
                        facts.push(target.clone());
                        batch.push(row);
                    }
                }
                let groups = if facts.is_empty() {
                    Vec::new()
                } else {
                    vec![(ACCOUNT, facts.clone())]
                };
                let known = native::bind_known(
                    ctx,
                    Self::DECLARATION,
                    &[native::Facts::collection(Source::SearchAccounts, groups)],
                )?;
                let principal = known.preparation_principal(ctx, conn, caller).await?;
                require_org(ctx, conn, &principal, query.org_id).await?;
                selected.extend(batch);
                selected_targets.extend(facts);
                if selected.len() >= limit || exhausted {
                    break;
                }
                after = next;
            }
            selected.truncate(limit);
            selected_targets.truncate(limit);
            let groups = if selected_targets.is_empty() {
                Vec::new()
            } else {
                vec![(ACCOUNT, selected_targets)]
            };
            Ok((
                Selected {
                    rows: selected,
                    org: query.org_id,
                },
                vec![native::Facts::collection(Source::SearchAccounts, groups)],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the account search changed actor"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Vec<UserRef>>> {
        Box::pin(async move {
            require_org(ctx, conn, principal, value.org).await?;
            Ok(value
                .rows
                .iter()
                .map(|row| refusals::user_ref(&row.user))
                .collect())
        })
    }
}
