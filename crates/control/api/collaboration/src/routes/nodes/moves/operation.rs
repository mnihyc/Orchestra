//! The native move's actual effects and optional result/review source bounds.

use super::NodeKind;

use super::permission_to_receive;

use crate::key_operations::resolver::Disclosure;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::Guard;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameSources;

use crate::routes::documents::build::ProjectedDocumentRef;

use crate::routes::documents::build::WrittenLabelSource;

use crate::routes::nodes::read_sources;

use crate::routes::nodes::view;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::nodes::MovePreviewBody;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeMoveBody;

use orch_api_contract::nodes::NodeMovePlan;

use orch_api_contract::nodes::NodeMovePreviewResponse;

use orch_api_contract::nodes::NodeMoveReceipt;

use orch_api_contract::nodes::NodeMoveReviewReceipt;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::operations::TargetResolver;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::engine::Graph;

use orch_db::queries::documents;

use orch_db::Conn;

use orch_db::Tx;

use std::collections::BTreeSet;

use std::collections::HashMap;
const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "move_names_orgs"),
        (ObjectKind::Project, "move_names_projects"),
        (ObjectKind::Topic, "move_names_topics"),
        (ObjectKind::Thread, "move_names_threads"),
    ],
    containers: None,
};

/// Immutable facts from the very guard that admitted the move before placement changed.
struct Effects {
    kind: NodeKind,
    source: KeyResolvedTarget,
    destination: KeyResolvedTarget,
    receives: bool,
}

fn business(kind: NodeKind) -> ObjectKind {
    match kind {
        NodeKind::Org => ObjectKind::Org,
        NodeKind::Project => ObjectKind::Project,
        NodeKind::Topic => ObjectKind::Topic,
        NodeKind::Thread => ObjectKind::Thread,
    }
}

fn slot(plan: &OperationPlan, role: &str) -> ApiResult<usize> {
    plan.variant().facets.iter().position(|facet|matches!(facet.target,TargetResolver::Resolved(found)|TargetResolver::ResolvedCollection(found) if found==role))
        .ok_or_else(||ApiError::internal("a move source role is undeclared"))
}

fn bind(
    plan: &mut OperationPlan,
    role: &str,
    kind: KeyTargetKind,
    rows: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let slot = slot(plan, role)?;
    if rows.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, rows)
    }
}

fn base(ctx: &Ctx, effects: &Effects) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let family = KeyTargetKind::Object(business(effects.kind));
    bind(
        &mut plan,
        "move_source",
        family,
        std::slice::from_ref(&effects.source),
    )?;
    bind(
        &mut plan,
        "move_destinations",
        family,
        if effects.receives {
            std::slice::from_ref(&effects.destination)
        } else {
            &[]
        },
    )?;
    Ok(plan)
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    kind: NodeKind,
    parent: orch_types::Id,
) -> ApiResult<OperationPlan> {
    let graph = Graph::load(conn, &[ctx.id("id")?, parent])
        .await
        .map_err(ApiError::internal)?;
    let source = graph
        .site_of(ctx.id("id")?)
        .filter(|site| site.kind == business(kind))
        .ok_or_else(ApiError::not_found)?;
    let destination = graph.site_of(parent).ok_or_else(ApiError::not_found)?;
    let receives = permission_to_receive(kind, Some(destination.kind.as_str())).is_some();
    let effects = Effects {
        kind,
        source: targets::from_site(&graph, &source)?,
        destination: targets::from_site(&graph, &destination)?,
        receives,
    };
    Ok(base(ctx, &effects)?.with_data(effects))
}

macro_rules! resolver {
    ($name:ident,$kind:expr,$body:ty) => {
        pub(super) fn $name<'a>(
            ctx: &'a Ctx,
            conn: &'a mut Conn,
            _caller: &'a Caller,
            (): (),
            body: $body,
        ) -> OperationFuture<'a> {
            Box::pin(prepare(ctx, conn, $kind, body.parent_id))
        }
    };
}

resolver!(org_preview, NodeKind::Org, MovePreviewBody);

resolver!(project_preview, NodeKind::Project, MovePreviewBody);

resolver!(topic_preview, NodeKind::Topic, MovePreviewBody);

resolver!(org_move, NodeKind::Org, NodeMoveBody);

resolver!(project_move, NodeKind::Project, NodeMoveBody);

resolver!(topic_move, NodeKind::Topic, NodeMoveBody);

pub(super) async fn action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Guard)> {
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let current = ctx
        .for_operation_checkpoint(guard.user().clone(), guard.now().get())
        .with_admitted_caller(guard.principal().caller().clone())?;
    Ok((current, tx, guard))
}

fn modern(principal: &Principal) -> bool {
    principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

fn place_name(sources: &mut NameSources, place: &NodeOrSystem) {
    if let NodeOrSystem::Node(node) = place {
        sources.nodes.insert(node.id);
    }
}

pub(super) fn review_receipt(plan: &NodeMovePlan, kind: NodeKind) -> NodeMoveReviewReceipt {
    NodeMoveReviewReceipt {
        projection: ReceiptProjection::Receipt,
        node_id: plan.node.id,
        parent_id: plan.to.id,
        kind,
        preflight_id: plan.preflight_id,
        stamp: plan.stamp.clone(),
        revision: plan.revision,
        expires_at: plan.expires_at,
        blocker_codes: plan
            .blockers
            .iter()
            .map(|blocker| blocker.code.clone())
            .collect(),
    }
}

/// Canonical review data has already been stamped and retained. Only its public
/// projection changes here; the same user's other credential sees the same stamp.
pub(super) async fn review(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    guard: &Guard,
    canonical: NodeMovePlan,
) -> ApiResult<NodeMovePreviewResponse> {
    let effects = guard.operation_data::<Effects>()?;
    let receipt = review_receipt(&canonical, effects.kind);
    if !modern(principal) {
        return Ok(NodeMovePreviewResponse::Full(Box::new(canonical)));
    }
    let result = async {
        let mut plan = base(ctx, &effects)?;
        let root = targets::node(conn, canonical.node.id).await?;
        bind(
            &mut plan,
            "move_result",
            root.object.kind,
            std::slice::from_ref(&root),
        )?;
        let to = targets::node(conn, canonical.to.id).await?;
        bind(
            &mut plan,
            "move_review_destinations",
            to.object.kind,
            std::slice::from_ref(&to),
        )?;
        let mut sources = NameSources::default();
        sources
            .nodes
            .extend([canonical.node.id, canonical.from.id, canonical.to.id]);
        for setting in &canonical.settings {
            if let Some(place) = &setting.locked_by {
                place_name(&mut sources, place);
            }
        }
        let bounds = projection_names(conn, principal.caller(), &mut plan, sources, &NAMES).await?;
        // Act errors may also return a current review, but it contains no full
        // node's document/environment payloads.
        if !ctx.route.name.ends_with("Preview") {
            bind(
                &mut plan,
                "move_full_documents",
                KeyTargetKind::Object(ObjectKind::Document),
                &[],
            )?;
            bind(
                &mut plan,
                "move_full_environments",
                KeyTargetKind::Object(ObjectKind::Environment),
                &[],
            )?;
        }
        plan.read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let mut public = canonical;
                let mut ids = vec![public.node.id, public.from.id, public.to.id];
                ids.extend(public.settings.iter().filter_map(|setting| {
                    setting.locked_by.as_ref().and_then(|place| match place {
                        NodeOrSystem::Node(node) => Some(node.id),
                        _ => None,
                    })
                }));
                let refs = bounds.refs(conn, reader, &ids).await?;
                public.node = refs
                    .get(&public.node.id)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                public.from = refs
                    .get(&public.from.id)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                public.to = refs
                    .get(&public.to.id)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?;
                for setting in &mut public.settings {
                    if let Some(NodeOrSystem::Node(node)) = &mut setting.locked_by {
                        *node = refs
                            .get(&node.id)
                            .cloned()
                            .ok_or_else(ApiError::not_found)?;
                    }
                }
                Ok(public)
            })
        })
        .await
    }
    .await;
    match result {
        Ok(Some(plan)) if !ctx.key_receipt_required() => {
            Ok(NodeMovePreviewResponse::Full(Box::new(plan)))
        }
        Ok(_) => Ok(NodeMovePreviewResponse::Receipt(receipt)),
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(NodeMovePreviewResponse::Receipt(receipt))
        }
        Err(error) => Err(error),
    }
}

/// Required effects retain their pre-placement facts. Only optional result Read
/// resolves the actual current placement, so losing old-subtree Read cannot veto
/// an admitted move and newly reached destination Read can disclose its result.
pub(super) async fn node(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    guard: &Guard,
    id: orch_types::Id,
) -> ApiResult<Option<Node>> {
    if !modern(principal) {
        return crate::routes::nodes::shown(ctx, conn, principal, id)
            .await
            .map(Some);
    }
    let effects = guard.operation_data::<Effects>()?;
    let result = async {
        let mut plan = base(ctx, &effects)?;
        let root = targets::node(conn, id).await?;
        let result_slot = slot(&plan, "move_result")?;
        if !plan.permits_binding(
            principal.caller(),
            result_slot,
            root.object.kind,
            std::slice::from_ref(&root),
        )? {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        }
        bind(
            &mut plan,
            "move_result",
            root.object.kind,
            std::slice::from_ref(&root),
        )?;
        bind(
            &mut plan,
            "move_review_destinations",
            KeyTargetKind::Object(ObjectKind::Org),
            &[],
        )?;
        let mut names = NameSources::default();
        names.nodes.insert(id);
        names
            .nodes
            .extend(root.node_ancestors.iter().filter_map(|node| node.id));
        let mut readmes = BTreeSet::new();
        let rows = documents::readmes_of(conn, &[id]).await?;
        let readable: HashMap<_, _> = orch_authz::documents::listed(
            conn,
            principal,
            &rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|row| row.holds(Permission::DocView))
        .map(|row| (row.document.id, row))
        .collect();
        let doc_slot = slot(&plan, "move_full_documents")?;
        for row in rows {
            let Some(listed) = readable.get(&row.id) else {
                continue;
            };
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Document,
                    id: row.id,
                },
            )
            .await?;
            if !plan.permits_binding(
                principal.caller(),
                doc_slot,
                target.object.kind,
                std::slice::from_ref(&target),
            )? {
                continue;
            }
            readmes.insert(row.id);
            for source in ProjectedDocumentRef::candidates(conn, &row, listed).await? {
                if let WrittenLabelSource::Node(id) = source {
                    names.nodes.insert(id);
                } else {
                    return Err(ApiError::internal(
                        "a move README has an undeclared name source",
                    ));
                }
            }
        }
        let names = projection_names(conn, principal.caller(), &mut plan, names, &NAMES).await?;
        let native = view::one_with_read_sources(
            conn,
            principal,
            id,
            principal.now().get(),
            Policy::default(),
            &names.nodes,
            None,
            &readmes,
        )
        .await?
        .ok_or_else(ApiError::not_found)?;
        let sources = read_sources::Sources::of(&native)?;
        let resolved = sources.targets(conn).await?;
        for (kind, role) in [
            (ObjectKind::Document, "move_full_documents"),
            (ObjectKind::Environment, "move_full_environments"),
        ] {
            let values = sources
                .objects()
                .filter(|object| object.kind == kind)
                .map(|object| {
                    resolved
                        .get(&object)
                        .cloned()
                        .ok_or_else(ApiError::not_found)
                })
                .collect::<ApiResult<Vec<_>>>()?;
            bind(&mut plan, role, KeyTargetKind::Object(kind), &values)?;
        }
        if sources
            .objects()
            .any(|object| object.kind.node().is_some() && !names.nodes.contains(&object.id))
        {
            return Err(ApiError::internal(
                "a native move result has an unbounded named node",
            ));
        }
        if sources.objects().any(|object| {
            object.kind.node().is_none()
                && !matches!(object.kind, ObjectKind::Document | ObjectKind::Environment)
        }) {
            return Err(ApiError::internal(
                "a native move result has an undeclared resource source",
            ));
        }
        plan.read_response(ctx, conn, principal.caller(), move |_conn, _reader| {
            Box::pin(async move { Ok(native) })
        })
        .await
    }
    .await;
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

pub(super) async fn receipt(
    conn: &mut Conn,
    id: orch_types::Id,
    parent: orch_types::Id,
    kind: NodeKind,
) -> ApiResult<NodeMoveReceipt> {
    let revision = orch_db::queries::hierarchy::with_ancestors(conn, &[id])
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?
        .revision;
    Ok(NodeMoveReceipt {
        projection: ReceiptProjection::Receipt,
        node_id: id,
        parent_id: parent,
        kind,
        revision,
    })
}

pub(super) enum Moved {
    Full(Box<Node>),
    Receipt(NodeMoveReceipt),
}
