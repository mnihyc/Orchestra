//! Stored native preview subjects and the existing private receiver page.

use super::*;

use crate::key_operations::native;

use crate::routes::nodes::read_sources::Sources;

use crate::routes::environments;

use crate::routes::resources;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::PageQuery;

use orch_api_contract::environment_sources::RECIPIENTS;

use orch_api_contract::environment_sources::RecipientSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::jobs::forks;

use orch_db::queries::jobs::forks::PreflightRow;

use orch_db::queries::notifications::operations;

use orch_types::Id;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) struct Recipients;

pub(super) struct Prepared {
    record: PreflightRow,
    kind: String,
    receivers: BTreeSet<Id>,
    visible: BTreeSet<Id>,
    page: Vec<Id>,
    next_cursor: Option<String>,
    hidden: i64,
    total: i64,
}

/// Native saved-plan polymorphism, not an HTTP endpoint or permission selector.
fn subjects(record: &PreflightRow) -> ApiResult<Vec<ObjectRef>> {
    Ok(match record.kind.as_str() {
        "loss" => {
            vec![orch_engine::loss::recipient_subject(&record.plan).map_err(ApiError::engine)?]
        }
        "host_delete" => vec![resources::host_delete::recipient_subject(&record.plan)?],
        "provider_update" => vec![resources::provider_recovery::recipient_subject(
            &record.plan,
        )?],
        "environment_move" => vec![ObjectRef {
            kind: ObjectKind::Environment,
            id: record
                .environment_id
                .ok_or_else(|| ApiError::internal("the native move preview has no Environment"))?,
        }],
        "move_batch" => environments::moves::recipient_subjects(&record.plan)?,
        "recover_batch" => environments::recovery::recipient_subjects(&record.plan)?,
        forks::BATCH_PLAN_KIND => {
            let native: orch_engine::fork_plan::StoredBatch =
                serde_json::from_value(record.plan.clone()).map_err(ApiError::internal)?;
            native.recipient_subjects()
        }
        _ => {
            return Err(ApiError::internal(
                "the native audience preview has no subject provider",
            ));
        }
    })
}

impl native::Selected for Recipients {
    type Source = Source;
    type Query = PageQuery;
    type Value = Prepared;
    type Reply = api::OperationRecipients;
    const DECLARATION: &'static Declaration<Source> = &RECIPIENTS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let record = forks::preflight(conn, ctx.id("id")?)
                .await?
                .filter(|row| {
                    row.created_by == caller.user() && row.expires_at > ctx.app.clock.now()
                })
                .ok_or_else(ApiError::not_found)?;
            let (ids, kind): (Vec<Id>, String) = sqlx::query_as(
                "SELECT users,notification_kind FROM operation_recipient_previews WHERE id=$1",
            )
            .bind(record.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(ApiError::not_found)?;
            let mut facts = Vec::new();
            if native::modern(caller) {
                let actual = Sources::from_objects(subjects(&record)?)
                    .identity_targets(conn)
                    .await?;
                let mut groups = BTreeMap::<_, Vec<_>>::new();
                for (object, target) in actual {
                    groups
                        .entry(KeyTargetKind::Object(object.kind))
                        .or_default()
                        .push(target);
                }
                facts.push(native::Facts::collection(
                    Source::Subjects,
                    groups.into_iter().collect(),
                ));
            }
            let plan = native::bind_known(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let ids = operations::receivers(conn, &ids, &kind).await?;
            let visible = operations::visible_names(conn, principal.user(), &ids).await?;
            let after = query
                .cursor
                .as_deref()
                .map(|text| {
                    text.parse::<Id>()
                        .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))
                })
                .transpose()?;
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let mut page = visible
                .iter()
                .copied()
                .filter(|id| after.is_none_or(|after| *id > after))
                .take(limit + 1)
                .collect::<Vec<_>>();
            let next_cursor = if page.len() > limit {
                page.get(limit - 1).map(ToString::to_string)
            } else {
                None
            };
            page.truncate(limit);
            Ok((
                Prepared {
                    record,
                    kind,
                    hidden: (ids.len() - visible.len()) as i64,
                    total: ids.len() as i64,
                    receivers: ids.into_iter().collect(),
                    visible: visible.into_iter().collect(),
                    page,
                    next_cursor,
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::not_found());
            }
            forks::preflight(conn, ctx.id("id")?)
                .await?
                .filter(|row| {
                    row.created_by == principal.user() && row.expires_at > ctx.app.clock.now()
                })
                .ok_or_else(ApiError::not_found)?;
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<api::OperationRecipients>> {
        Box::pin(async move {
            let users =
                crate::middleware::authorization::refusals::user_refs(conn, &value.page).await?;
            let items = value
                .page
                .iter()
                .filter_map(|id| users.get(id).cloned())
                .collect();
            Ok(api::OperationRecipients {
                people: orch_api_contract::common::Page {
                    items,
                    next_cursor: value.next_cursor.clone(),
                },
                hidden: value.hidden,
                total: value.total,
            })
        })
    }

    fn check_opened<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a PageQuery,
        opened: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.record.expires_at > ctx.app.clock.now()
                && opened.record.id == fresh.record.id
                && opened.record.kind == fresh.record.kind
                && opened.record.created_by == fresh.record.created_by
                && opened.record.created_at == fresh.record.created_at
                && opened.record.plan == fresh.record.plan
                && opened.record.node_id == fresh.record.node_id
                && opened.record.environment_id == fresh.record.environment_id
                && opened.kind == fresh.kind
                && opened.receivers.is_subset(&fresh.receivers)
                && opened.page.iter().all(|id| fresh.visible.contains(id)))
        })
    }
}
