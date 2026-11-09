//! Factual card sources, shared by initial admission and the held response boundary.
use super::*;
use crate::key_operations::native::{self, Facts};
use crate::registry::{OperationFuture, OperationPlan};
use crate::routes::access::page::NameBounds;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_api_contract::reference_sources::{self as sources, Source};
use orch_authz::Caller;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct Bound {
    pub target: KeyResolvedTarget,
    pub names: NameBounds,
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ReferencePreviewQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (plan, value) = prepare(ctx, conn, caller, &query).await?;
        Ok(plan.with_data(value))
    })
}
async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &ReferencePreviewQuery,
) -> ApiResult<(OperationPlan, Bound)> {
    use crate::key_operations::targets;
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    let target = match query.kind {
        InlineRefKind::Document => super::super::read_operation::identity(conn, query.id).await?,
        InlineRefKind::Node => targets::node_identity(conn, query.id).await?,
        InlineRefKind::Chat => targets::chat_identity(conn, query.id).await?,
        InlineRefKind::EnvironmentFile => {
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: query.id,
                },
            )
            .await?
        }
        InlineRefKind::File => {
            let document: Option<Id> =
                sqlx::query_scalar("SELECT document_id FROM document_files WHERE id=$1")
                    .bind(query.id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            if let Some(document) = document {
                super::super::read_operation::identity(conn, document).await?
            } else {
                let chat: Id =
                    sqlx::query_scalar("SELECT chat_id FROM chat_file_references WHERE id=$1")
                        .bind(query.id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(DbError::from)?
                        .ok_or_else(ApiError::not_found)?;
                targets::chat_identity(conn, chat).await?
            }
        }
    };
    let facts = vec![Facts::one(
        Source::Target,
        target.object.kind,
        target.clone(),
    )];
    let mut plan = native::bind_known(ctx, &sources::PREVIEW, &facts)?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    // Only native identities are selected here. The card's ordinary reader decides content.
    let mut ids = if query.kind == InlineRefKind::EnvironmentFile {
        BTreeSet::new()
    } else {
        target
            .node_ancestors
            .iter()
            .filter_map(|node| node.id)
            .collect::<BTreeSet<_>>()
    };
    if target.object.kind
        == orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Document)
        && let Some(row) =
            rows::get(conn, target.object.id.ok_or_else(ApiError::not_found)?).await?
    {
        if let Ok(listed) = build::standing(conn, &principal, row.id, Policy::default()).await {
            ids.extend(
                build::ProjectedDocumentRef::candidates(conn, &row, &listed)
                    .await?
                    .into_iter()
                    .filter_map(|source| match source {
                        build::WrittenLabelSource::Node(id) => Some(id),
                        _ => None,
                    }),
            );
        }
        if let Some(block) = query.block {
            let selected = rows::live_blocks(conn, row.id)
                .await?
                .into_iter()
                .filter(|row| row.id == block)
                .collect::<Vec<_>>();
            ids.extend(
                build::ProjectedBlocks::candidates(conn, &selected)
                    .await?
                    .into_iter()
                    .filter_map(|source| match source {
                        build::WrittenLabelSource::Node(id) => Some(id),
                        _ => None,
                    }),
            );
        }
    }
    if let Some(number) = query.message.filter(|_| query.kind == InlineRefKind::Chat)
        && let Some(chat) = target.object.id
    {
        ids.extend(
            share_rows::chat_messages_numbered(conn, chat, number, number)
                .await?
                .into_iter()
                .filter_map(|item| item.row.author_thread_id),
        );
    }
    let roles = crate::routes::access::operations::NameRoles {
        groups: None,
        containers: None,
        nodes: [
            (ObjectKind::Org, "reference_name_orgs"),
            (ObjectKind::Project, "reference_name_projects"),
            (ObjectKind::Topic, "reference_name_topics"),
            (ObjectKind::Thread, "reference_name_threads"),
        ],
    };
    let names = crate::routes::access::operations::projection_names(
        conn,
        principal.caller(),
        &mut plan,
        crate::routes::access::page::NameSources {
            nodes: ids,
            ..Default::default()
        },
        &roles,
    )
    .await?;
    Ok((plan, Bound { target, names }))
}

pub(super) fn names(ctx: &Ctx) -> ApiResult<NameBounds> {
    Ok(ctx.operation_data::<Bound>()?.names.clone())
}
pub(super) async fn retain(
    ctx: &Ctx,
    query: &ReferencePreviewQuery,
    bound: &Bound,
    content: bool,
) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let source = (query.clone(), bound.clone(), content);
    validate(ctx, &mut tx, &principal, &source).await?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::retain(
        ctx,
        principal.caller(),
        source,
        |ctx, conn, current, source| Box::pin(validate(ctx, conn, current, source)),
    )
}
async fn validate(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: &(ReferencePreviewQuery, Bound, bool),
) -> ApiResult<()> {
    let (query, old, content) = source;
    let principal = current_principal(ctx, conn, principal, query, old).await?;
    if *content {
        match query.kind {
            InlineRefKind::Document => allowed(
                orch_authz::documents::check(
                    conn,
                    &principal,
                    query.id,
                    Permission::DocView,
                    Policy::default(),
                )
                .await,
            )?,
            InlineRefKind::Node => {
                allowed(
                    orch_authz::nodes::check_named(
                        conn,
                        &principal,
                        query.id,
                        PermissionName::OfNode(NodePermission::View),
                        Act::Use,
                        Policy::default(),
                    )
                    .await,
                )?;
                if query.turn.is_some() {
                    allowed(
                        orch_authz::nodes::check(
                            conn,
                            &principal,
                            query.id,
                            Permission::ThreadView,
                            Act::Use,
                            Policy::default(),
                        )
                        .await,
                    )?;
                }
            }
            InlineRefKind::Chat => {
                let row = chat_rows::chat(conn, query.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if !orch_authz::chats::listed_access(conn, &principal, &[row])
                    .await
                    .map_err(ApiError::internal)?
                    .contains_key(&query.id)
                {
                    return Err(ApiError::not_found());
                }
            }
            InlineRefKind::EnvironmentFile => {
                environment_access_on(conn, &principal, query).await?;
            }
            InlineRefKind::File => {} // the independently retained exact attachment witness owns this byte boundary
        }
    }
    Ok(())
}

/// Restore the complete current read admission before checking a frozen native source.
/// Publication restores credential ceilings, never a historical operation's proof.
pub(super) async fn current_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
    old: &Bound,
) -> ApiResult<Principal> {
    let (plan, fresh) = prepare(ctx, conn, principal.caller(), query).await?;
    if fresh.target != old.target || !old.names.nodes.is_subset(&fresh.names.nodes) {
        return Err(ApiError::not_found());
    }
    let admitted = plan.admit(principal.caller())?;
    let principal = crate::middleware::authorization::load_principal(
        conn,
        admitted.caller,
        ctx.app.clock.time(),
    )
    .await?;
    crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
}
