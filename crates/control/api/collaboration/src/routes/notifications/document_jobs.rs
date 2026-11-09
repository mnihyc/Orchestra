//! Owned document notice rows and native job projection sources through the shared read adapter.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::routes::environments::jobs as projection;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::notification_sources as sources;

use orch_api_contract::notification_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::workspace::Job;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::jobs as rows;

use orch_db::queries::jobs::JobRow;

use orch_types::Id;

use std::collections::HashMap;
pub(super) struct NoticeJobs;

pub(super) struct Selected {
    rows: Vec<JobRow>,
    next_cursor: Option<String>,
    names: Option<HashMap<Id, String>>,
}

impl native::Selected for NoticeJobs {
    type Source = Source;
    type Query = PageQuery;
    type Value = Selected;
    type Reply = Page<Job>;
    const DECLARATION: &'static Declaration<Source> = &sources::DOCUMENT_JOBS;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a PageQuery,
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let document = ctx.id("id")?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id: document,
                },
            )
            .await?;
            let document_fact = native::Facts::one(
                Source::NoticeDocument,
                KeyTargetKind::Object(ObjectKind::Document),
                target,
            );
            let known =
                native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&document_fact))?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            authorization::require_document(
                conn,
                &principal,
                document,
                Permission::DocView,
                Policy::default(),
            )
            .await?;
            let after = query
                .cursor
                .as_deref()
                .map(str::parse::<Id>)
                .transpose()
                .map_err(|_| ApiError::invalid("cursor", "is not a notification job cursor"))?;
            let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
            let mut rows = rows::document_notices_page(
                conn,
                caller.user(),
                document,
                after,
                i64::try_from(limit + 1).map_err(ApiError::internal)?,
            )
            .await?;
            let more = rows.len() > limit;
            rows.truncate(limit);
            let next_cursor = more
                .then(|| rows.last().map(|row| row.id.to_string()))
                .flatten();
            let mut names = projection::machine_name_sources(conn, &rows).await?;
            let ids = names.keys().copied().collect::<Vec<_>>();
            let mut found = targets::machine_subjects(conn, &ids).await?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let machine = KeyTargetKind::Object(ObjectKind::Machine);
            let mut selected = Vec::new();
            for id in ids {
                let Some(target) = found.remove(&id) else {
                    names.remove(&id);
                    continue;
                };
                if !native::modern(caller)
                    || native::permits_usage(
                        &plan,
                        Self::DECLARATION,
                        caller,
                        &Source::NoticeMachines,
                        FacetUsage::ResponseContent,
                        machine,
                        &target,
                    )?
                {
                    selected.push(target);
                } else {
                    names.remove(&id);
                }
            }
            let groups = if selected.is_empty() {
                Vec::new()
            } else {
                vec![(machine, selected)]
            };
            Ok((
                Selected {
                    rows,
                    next_cursor,
                    names: native::modern(caller).then_some(names),
                },
                vec![
                    document_fact,
                    native::Facts::collection(Source::NoticeMachines, groups),
                ],
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require_document(
                conn,
                principal,
                ctx.id("id")?,
                Permission::DocView,
                Policy::default(),
            )
            .await
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Page<Job>>> {
        Box::pin(async move {
            let mut items = Vec::with_capacity(value.rows.len());
            for row in &value.rows {
                items.push(
                    projection::view_with_machine_sources(conn, row, value.names.as_ref()).await?,
                );
            }
            Ok(Page {
                items,
                next_cursor: value.next_cursor.clone(),
            })
        })
    }
}
