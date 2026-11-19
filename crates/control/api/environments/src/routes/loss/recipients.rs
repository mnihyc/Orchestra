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
        "resource_stop" => vec![
            serde_json::from_value(
                record
                    .plan
                    .get("governor")
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the resource review has no subject"))?,
            )
            .map_err(ApiError::internal)?,
        ],
        "loss" => {
            vec![orch_engine::loss::recipient_subject(&record.plan).map_err(ApiError::engine)?]
        }
        "host_delete" => vec![resources::host_delete::recipient_subject(&record.plan)?],
        "mount_copy" => {
            let review: orch_engine::mount_copies::location::Review =
                serde_json::from_value(record.plan["review"].clone())
                    .map_err(ApiError::internal)?;
            vec![ObjectRef {
                kind: ObjectKind::Mount,
                id: review.source().mount,
            }]
        }
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
        forks::PLAN_KIND => {
            let native: orch_engine::fork_plan::StoredPlan =
                serde_json::from_value(record.plan.clone()).map_err(ApiError::internal)?;
            vec![
                native
                    .move_recipient_subject()
                    .ok_or_else(ApiError::not_found)?,
            ]
        }
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

/// A profile may be public while the work associating its owner with this review is private.
async fn resource_names(
    conn: &mut Conn,
    principal: &Principal,
    plan: &crate::registry::OperationPlan,
    record: &PreflightRow,
    names: &mut Vec<Id>,
) -> ApiResult<()> {
    use orch_api_contract::resource_facts::FactTarget;
    use orch_engine::resource_facts;
    if record.kind != "resource_stop" || names.is_empty() {
        return Ok(());
    }
    let object: ObjectRef =
        serde_json::from_value(record.plan["object"].clone()).map_err(ApiError::internal)?;
    let model_id =
        serde_json::from_value(record.plan["model_id"].clone()).map_err(ApiError::internal)?;
    let admitted: BTreeSet<Id> =
        serde_json::from_value(record.plan["nameable"].clone()).map_err(ApiError::internal)?;
    let people: Vec<orch_engine::notifications::OperationRecipient> =
        serde_json::from_value(record.plan["audience"].clone()).map_err(ApiError::internal)?;
    let raw = [resource_facts::target_dependents(
        conn,
        &FactTarget { object, model_id },
        principal.now(),
    )
    .await
    .map_err(ApiError::engine)?];
    let mut visibility =
        resource_facts::prepare_projection(conn, principal, Default::default(), &raw)
            .await
            .map_err(ApiError::engine)?;
    if native::modern(principal.caller()) {
        let mut allowed = std::collections::HashSet::new();
        for (object, target) in
            Sources::from_objects(visibility.sources().filter(|object| {
                matches!(object.kind, ObjectKind::Thread | ObjectKind::Environment)
            }))
            .identity_targets(conn)
            .await?
        {
            if native::permits(
                plan,
                &RECIPIENTS,
                principal.caller(),
                &Source::Subjects,
                KeyTargetKind::Object(object.kind),
                &target,
            )? {
                allowed.insert((object.kind, object.id));
            }
        }
        visibility.restrict_to(&allowed);
    }
    let dependencies: BTreeSet<_> = visibility
        .named_edges(&raw)
        .filter_map(|(_, edge)| edge.object.map(|object| object.id))
        .collect();
    let current: BTreeSet<_> = people
        .iter()
        .filter(|person| {
            !person.threads.is_disjoint(&dependencies)
                || object.kind == ObjectKind::Mount
                    && !person.environments.is_disjoint(&dependencies)
        })
        .map(|person| person.user)
        .collect();
    names.retain(|id| admitted.contains(id) && current.contains(id));
    Ok(())
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
            let mut visible = operations::visible_names(conn, principal.user(), &ids).await?;
            resource_names(conn, &principal, &plan, &record, &mut visible).await?;
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
            let record = forks::preflight(conn, ctx.id("id")?)
                .await?
                .filter(|row| {
                    row.created_by == principal.user() && row.expires_at > ctx.app.clock.now()
                })
                .ok_or_else(ApiError::not_found)?;
            if record.kind == forks::PLAN_KIND {
                for subject in subjects(&record)? {
                    crate::middleware::authorization::require(
                        conn,
                        principal,
                        subject.id,
                        crate::middleware::authorization::VIEW,
                        orch_authz::Act::Use,
                        orch_authz::Policy::default(),
                    )
                    .await?;
                }
            }
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
