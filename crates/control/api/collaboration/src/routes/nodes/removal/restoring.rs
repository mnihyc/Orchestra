//! Exact native retained restoration operands and independent completed Node readback.

use super::*;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use crate::routes::nodes::content_sources;

use crate::routes::nodes::read_sources::Sources;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::ownership::NodeRestoreBody;

use orch_api_contract::ownership::NodeRestoreReceipt;

use orch_api_contract::ownership::NodeRestoreResponse;

use orch_api_contract::ownership_sources as sources;

use orch_api_contract::ownership_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) struct Restore;

pub(super) struct Prepared {
    row: DeletedRow,
    restoration: hierarchy::PreparedRestoration,
    resources: orch_engine::membership::PreparedHomeRestoration,
    kept: Option<Vec<hierarchy::KeptArchive>>,
    current: Option<Id>,
    undo: Option<Id>,
    required: Vec<native::Facts<Source>>,
}

pub(super) struct Outcome {
    receipt: NodeRestoreReceipt,
    required: Vec<native::Facts<Source>>,
}

async fn may_restore(conn: &mut Conn, principal: &Principal, row: &DeletedRow) -> ApiResult<()> {
    let result = match row.parent_id {
        None => {
            let owner = access::owns_page(conn, row.id, principal.user()).await?;
            if principal.caller().permits(Permission::AccessOwn)
                && principal.caller().reaches(Some(&[row.id]))
                && (owner || principal.is_system_admin())
            {
                Ok(())
            } else {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "a deleted root org is brought back by one of its owners or a system admin",
                )
                .with("required", "access.own")
                .with("object", json!({"kind":"org","id":row.id}))
                .with("requestable", false)
                .with("override", false)
                .with("missing", Vec::<serde_json::Value>::new()))
            }
        }
        Some(parent) => {
            orch_db::queries::trace::read_scope(
                &[parent],
                authorization::require_projected(
                    conn,
                    principal,
                    parent,
                    MANAGE,
                    Act::Delete,
                    Policy::default(),
                    native::modern(principal.caller()),
                ),
            )
            .await
        }
    };
    result.map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}

async fn authority(conn: &mut Conn, row: &DeletedRow) -> ApiResult<native::Facts<Source>> {
    let target = targets::node_identity(conn, row.parent_id.unwrap_or(row.id)).await?;
    Ok(native::Facts::one(
        Source::RestorationAuthority,
        target.object.kind,
        target,
    ))
}

async fn validate(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &DeletedRow,
    body: &NodeRestoreBody,
) -> ApiResult<(Option<Id>, Option<Vec<hierarchy::KeptArchive>>)> {
    if purge_at(row.deleted_at, ctx.app.config.retention.soft_delete_days) <= ctx.now {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "it was deleted so long ago that it is gone for good",
        ));
    }
    let current = orch_db::queries::trace::node_deletion(conn, row.id).await?;
    if body.expected_deletion_id.is_some() && body.expected_deletion_id != current {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the selected deletion is no longer current",
        )
        .with("current_deletion_id", current));
    }
    if let Some(parent) = row.parent_id
        && hierarchy::deleted_node(conn, parent).await?.is_some()
    {
        let refs = view::refs(conn, principal, &[parent], Policy::default()).await?;
        return Err(ApiError::new(
            ErrorCode::NodeRestoreParentFirst,
            "bring back the parent under its own restoration authority first",
        )
        .with("parent", refs.get(&parent)));
    }
    let kept = match body.undo_delete {
        None => None,
        Some(group) => {
            if current != Some(group) {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the deletion this undoes is no longer the current one",
                )
                .with("current_deletion_id", current));
            }
            Some(hierarchy::kept_archive_states(conn, group).await?)
        }
    };
    Ok((current, kept))
}

async fn required_facts(
    conn: &mut Conn,
    prepared: &Prepared,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let objects = prepared
        .resources
        .resources()
        .iter()
        .map(|row| {
            Ok(ObjectRef {
                kind: row.kind.parse().map_err(ApiError::internal)?,
                id: row.id,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let identities = Sources::from_objects(objects.iter().copied())
        .identity_targets(conn)
        .await?;
    let mut resources = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for object in objects {
        resources
            .entry(KeyTargetKind::Object(object.kind))
            .or_default()
            .push(
                identities
                    .get(&object)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?,
            );
    }
    let reopened = prepared
        .kept
        .iter()
        .flatten()
        .filter(|one| one.archived_at.is_none() && one.kind != "org")
        .map(|one| one.id)
        .collect::<Vec<_>>();
    let graph = orch_authz::engine::Graph::load(conn, &reopened)
        .await
        .map_err(ApiError::internal)?;
    let mut unarchive = BTreeMap::<KeyTargetKind, Vec<_>>::new();
    for id in reopened {
        let target = orch_engine::operation_scope::targets::from_node_identity(&graph, id)
            .ok_or_else(ApiError::not_found)?;
        unarchive
            .entry(target.object.kind)
            .or_default()
            .push(target);
    }
    Ok(vec![
        authority(conn, &prepared.row).await?,
        native::Facts::collection(
            Source::RestorationResources,
            resources.into_iter().collect(),
        ),
        native::Facts::collection(
            Source::RestorationUnarchive,
            unarchive.into_iter().collect(),
        ),
    ])
}

impl effects::Effect for Restore {
    type Source = Source;
    type Query = ();
    type Body = NodeRestoreBody;
    type Value = Prepared;
    type Output = Outcome;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &sources::RESTORE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: NodeRestoreBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let initial = hierarchy::deleted_node(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let known =
                native::bind_known(ctx, &sources::RESTORE, &[authority(conn, &initial).await?])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            may_restore(conn, &principal, &initial).await?;
            validate(ctx, conn, &principal, &initial, &body).await?;
            ctx.app
                .engine
                .lock_archive_in(conn, id)
                .await
                .map_err(ApiError::engine)?;
            let rows = hierarchy::with_ancestors(conn, &[id]).await?;
            for id in rows.into_iter().map(|row| row.id).collect::<BTreeSet<_>>() {
                hierarchy::lock(conn, id).await?;
            }
            let row = hierarchy::deleted_node(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let (current, kept) = validate(ctx, conn, &principal, &row, &body).await?;
            let restoration = hierarchy::prepare_restoration(conn, id).await?;
            if restoration.group != current {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the selected deletion changed while preparing restoration",
                ));
            }
            let restored = restoration
                .nodes
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>();
            let resources = ctx
                .app
                .engine
                .prepare_home_restoration_in(conn, id, current, &restored)
                .await
                .map_err(ApiError::internal)?;
            let mut prepared = Prepared {
                row,
                restoration,
                resources,
                kept,
                current,
                undo: body.undo_delete,
                required: Vec::new(),
            };
            let required = required_facts(conn, &prepared).await?;
            prepared.required = required.clone();
            Ok((prepared, required))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(may_restore(conn, principal, &value.row))
    }
}

impl effects::Apply for Restore {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Outcome>>> {
        Box::pin(async move {
            hierarchy::restore_prepared_subtree_before(
                conn,
                &value.restoration,
                principal.user(),
                ctx.now,
                move || before.check(),
            )
            .await?;
            ctx.app
                .engine
                .restore_prepared_home_resources_in(conn, &value.resources, principal.now())
                .await
                .map_err(ApiError::internal)?;
            let reopened = value
                .kept
                .iter()
                .flatten()
                .filter(|one| one.archived_at.is_none())
                .filter_map(|one| {
                    refusals::node_kind(&one.kind).map(|kind| (one.id, kind, &one.name))
                })
                .filter(|(_, kind, _)| *kind != NodeKind::Org)
                .collect::<Vec<_>>();
            for (id, _, _) in &reopened {
                super::super::may_archive(conn, principal, *id, Act::Unarchive).await?;
            }
            if let Some(kept) = &value.kept {
                hierarchy::put_back_archive_states(conn, kept).await?;
            }
            epoch::bump(conn).await.map_err(ApiError::internal)?;
            let (place, _) = placed(conn, value.row.id).await?;
            let who = actor_name(ctx)?;
            record(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::NodeRestored),
                place,
                &format!("{who} brought back {}", value.row.name),
                json!({"deleted_at":value.row.deleted_at,"undo":value.kept.is_some()}),
            )
            .await?;
            for (id, _, name) in &reopened {
                let (place, _) = placed(conn, *id).await?;
                record(
                    ctx,
                    conn,
                    AuditAction::NodeUnarchived,
                    place,
                    &format!("{who} brought back {name} as it stood before it was deleted"),
                    json!({"undo":value.undo}),
                )
                .await?;
            }
            hierarchy::stamp_subtree(conn, value.row.id).await?;
            let receipt = NodeRestoreReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                node_id: value.row.id,
                kind: refusals::node_kind(&value.row.kind).ok_or_else(ApiError::not_found)?,
                deletion_id: value.current,
                restored_nodes: value.restoration.nodes.iter().map(|node| node.id).collect(),
                restored_resources: value
                    .resources
                    .resources()
                    .iter()
                    .map(|resource| resource.id)
                    .collect(),
            };
            Ok(effects::Outcome::Applied(Outcome {
                receipt,
                required: value.required.clone(),
            }))
        })
    }
}

pub(super) async fn run(ctx: Ctx, _body: NodeRestoreBody) -> ApiResult<NodeRestoreResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply::<Restore>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&completed.ctx.app)?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = completed.output.receipt.clone();
    publication::receipt(&completed.ctx, &receipt)?;
    let full =
        reply::render::<Restored>(&completed.ctx, &completed.accepted, completed.output).await?;
    Ok(full.map_or_else(
        || NodeRestoreResponse::Receipt(receipt),
        NodeRestoreResponse::full,
    ))
}

struct Restored;

impl reply::Projection for Restored {
    type Source = Source;
    type Input = Outcome;
    type Subject = hierarchy::NodeRow;
    type Value = content_sources::Prepared;
    type Reply = Node;
    const READ_SOURCE: Source = Source::RestoredNode;
    fn declaration(_input: &Outcome) -> &'static Declaration<Source> {
        &sources::RESTORE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Outcome,
    ) -> BoxFuture<'a, ApiResult<(hierarchy::NodeRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = input.receipt.node_id;
            let row = hierarchy::with_ancestors(conn, &[id])
                .await?
                .into_iter()
                .find(|row| row.id == id && row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, id).await?;
            let mut facts = input.required.clone();
            facts.push(native::Facts::one(
                Source::RestoredNode,
                target.object.kind,
                target,
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Outcome,
        row: &'a hierarchy::NodeRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if row.id != input.receipt.node_id
                || refusals::node_kind(&row.kind) != Some(input.receipt.kind)
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the restored Node response changed identity",
                ));
            }
            // The original post-restore shown wrapper adds no new View prerequisite.
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Outcome,
        row: &'a hierarchy::NodeRow,
    ) -> BoxFuture<'a, ApiResult<(content_sources::Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = content_sources::prepare(conn, principal, row.id).await?;
            let identities = Sources::from_objects(
                value
                    .objects
                    .iter()
                    .map(|&(kind, id)| ObjectRef { kind, id }),
            )
            .identity_targets(conn)
            .await?;
            let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
            for &(kind, id) in &value.objects {
                groups.entry(KeyTargetKind::Object(kind)).or_default().push(
                    identities
                        .get(&ObjectRef { kind, id })
                        .cloned()
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            Ok((
                value,
                vec![native::Facts::collection(
                    Source::RestoredContent,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Outcome,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Node>> {
        Box::pin(content_sources::render(
            ctx,
            conn,
            principal,
            input.receipt.node_id,
            &value.value,
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Outcome,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.id == fresh.subject.id && opened.value.preserves(&fresh.value))
        })
    }
}
