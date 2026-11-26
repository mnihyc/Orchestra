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
    include_external: bool,
    next_cursor: Option<String>,
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
    type Reply = orch_api_contract::users::UserSearchReply;
    const DECLARATION: &'static Declaration<Source> = &sources::USER_SEARCH;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a UserSearchQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let text = query.q.trim();
            if query.include_external.unwrap_or(false) && query.org_id.is_none() {
                return Err(invalid("include_external", "requires org_id"));
            }
            if !query.picker.unwrap_or(false)
                && text.chars().count() < usize::try_from(USER_SEARCH_MIN_CHARS).unwrap_or(2)
            {
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
            let limit = usize::try_from(query.limit.unwrap_or(USER_SEARCH_DEFAULT_RESULTS))
                .map_err(ApiError::internal)?;
            if limit == 0 {
                return Err(invalid("limit", "must be greater than zero"));
            }
            let recent = if query.picker.unwrap_or(false) && text.is_empty() {
                orch_api_base::picker_recents::read(conn, caller.user())
                    .await?
                    .choices
                    .remove("user")
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|key| key.parse::<Id>().ok())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let mut ranks = std::collections::HashMap::new();
            for (index, id) in recent.iter().enumerate() {
                ranks
                    .entry(*id)
                    .or_insert(i64::try_from(index).map_err(ApiError::internal)? + 1);
            }
            let rank = |row: &identity::UserSearchRow| {
                ranks.get(&row.user.id).copied().unwrap_or(i64::MAX)
            };
            let page_query = orch_api_contract::common::CollectionQuery {
                q: Some(query.q.clone()),
                limit: query.limit,
                cursor: query.cursor.clone(),
            };
            let recent_revision =
                orch_types::Hash::of(&serde_json::to_vec(&recent).map_err(ApiError::internal)?);
            let source = serde_json::to_string(&(
                "people",
                caller.user(),
                query.org_id,
                query.picker.unwrap_or(false),
                query.include_external.unwrap_or(false),
                text,
                recent_revision,
            ))
            .map_err(ApiError::internal)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            if !native::modern(caller) {
                let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
                require_org(ctx, conn, &principal, query.org_id).await?;
            }
            let mut after =
                orch_api_base::collection::after::<(i64, String, Id)>(&page_query, &source)?;
            let mut selected = Vec::new();
            let mut selected_targets = Vec::new();
            loop {
                let rows = identity::search_user_page_rows(
                    conn,
                    &identity::UserSearchPage {
                        caller: caller.user(),
                        text,
                        org: query.org_id,
                        limit: i64::try_from(limit + 1).map_err(ApiError::internal)?,
                        after: after
                            .as_ref()
                            .map(|(rank, name, id)| (*rank, name.as_str(), *id)),
                        recent: &recent,
                        include_external: query.include_external.unwrap_or(false),
                    },
                )
                .await?;
                let exhausted = rows.len() < limit + 1;
                let next = rows
                    .last()
                    .map(|row| (rank(row), row.user.username.clone(), row.user.id));
                if !native::modern(caller) {
                    selected = rows;
                    break;
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
                if selected.len() > limit || exhausted {
                    break;
                }
                after = next;
            }
            let more = selected.len() > limit;
            selected.truncate(limit);
            selected_targets.truncate(limit);
            let groups = if selected_targets.is_empty() {
                Vec::new()
            } else {
                vec![(ACCOUNT, selected_targets)]
            };
            let next_cursor = if more {
                selected
                    .last()
                    .map(|row| {
                        orch_api_base::collection::cursor(
                            &page_query,
                            &source,
                            (rank(row), row.user.username.clone(), row.user.id),
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            Ok((
                Selected {
                    rows: selected,
                    org: query.org_id,
                    include_external: query.include_external.unwrap_or(false),
                    next_cursor,
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
    ) -> BoxFuture<'a, ApiResult<orch_api_contract::users::UserSearchReply>> {
        Box::pin(async move {
            require_org(ctx, conn, principal, value.org).await?;
            Ok(orch_api_contract::users::UserSearchReply {
                items: value
                    .rows
                    .iter()
                    .map(|row| refusals::user_ref(&row.user))
                    .collect(),
                next_cursor: value.next_cursor.clone(),
                org_member_ids: value.org.filter(|_| value.include_external).map(|_| {
                    value
                        .rows
                        .iter()
                        .filter(|row| row.in_org)
                        .map(|row| row.user.id)
                        .collect()
                }),
            })
        })
    }
}
