//! Current chat-operation preparation, protected sources and controlled destinations.

use super::*;
use crate::key_operations::{blob_sources, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{Caller, CapturedCaller};
use orch_chat::following::BlobOrigin;
use orch_types::Hash;

pub(super) async fn current_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<Caller> {
    CapturedCaller::capture(caller)
        .restore(conn, orch_types::Time::new(ctx.app.clock.now()))
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the credential is no longer valid",
            )
        })
}

fn modern(principal: &Principal) -> bool {
    principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}

/// Reuse the native refusal projector with the current principal. Chat Read does
/// not by itself authorize live names of another resource in a rich refusal.
pub(super) async fn refuse<T>(
    work: &Work,
    conn: &mut Conn,
    chat: Id,
    result: Result<T, ChatError>,
) -> ApiResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => Err(failed_projected(
            conn,
            &work.principal,
            Some(chat),
            error,
            modern(&work.principal) || work.projection_ctx.key_receipt_required(),
        )
        .await),
    }
}

pub(super) async fn prepare_work(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
) -> ApiResult<Work> {
    let caller = current_caller(ctx, conn, caller).await?;
    let principal = plan.preparation_principal(ctx, conn, &caller).await?;
    Ok(Work {
        projection_ctx: ctx.clone(),
        now: principal.now().get(),
        principal,
        origin: origin(ctx),
    })
}

pub(super) async fn project_current_result<T>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    result: ApiResult<T>,
) -> ApiResult<T> {
    match result {
        Ok(plan) => Ok(plan),
        // A typed database conflict is already content-free. Keep its canonical
        // retry marker and do not query an aborted snapshot while projecting it.
        Err(error) if error.is_database_retry() => Err(error),
        Err(error) => {
            // An ordinary lookup or trial may have waited across a credential-policy change.
            // Project using its current mode, retaining the original caller's ceiling.
            let caller = current_caller(ctx, conn, caller).await?;
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                Err(authorization::problem_without_content(error))
            } else {
                Err(error)
            }
        }
    }
}

/// Checked operands of this exact guarded preparation, never client-supplied authority.
pub(super) struct MessageEffects {
    pub replayed: Option<Id>,
    pub blobs: Vec<(Hash, BlobOrigin)>,
    pub names: names::AllowSet,
}

pub(super) fn destination(ctx: &Ctx, target: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let write = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Write
                && matches!(
                    facet.target,
                    TargetResolver::Path("id")
                        | TargetResolver::TypedPath {
                            parameter: "id",
                            kind: KeyTargetKind::Identity(KeyIdentityKind::Chat),
                        }
                )
        })
        .ok_or_else(|| ApiError::internal("the chat destination is not declared"))?;
    plan.bind(
        write,
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        std::slice::from_ref(target),
    )?;
    let response = plan
        .variant()
        .facets
        .iter()
        .position(|facet| facet.usage == FacetUsage::ResponseContent)
        .ok_or_else(|| ApiError::internal("the chat disclosure is not declared"))?;
    plan.bind(
        response,
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        std::slice::from_ref(target),
    )?;
    Ok(plan)
}
fn agents(plan: &mut OperationPlan, targets: &[KeyResolvedTarget]) -> ApiResult<()> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            facet.target == TargetResolver::ResolvedCollection("message_agent_threads")
        })
        .ok_or_else(|| ApiError::internal("the agent destinations are not declared"))?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, KeyTargetKind::Object(ObjectKind::Thread), targets)
    }
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ChatMessageCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let row = rows::chat(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = targets::from_chat(conn, &row).await?;
        let primary = destination(ctx, &target)?;
        let principal = primary.preparation_principal(ctx, conn, caller).await?;
        let now = principal.now().get();
        let work = Work {
            projection_ctx: ctx.clone(),
            principal,
            origin: origin(ctx),
            now,
        };
        let loaded = load(conn, &work, id, Mode::Read).await?;
        work.refuse(conn, Some(id), loaded.ensure_send()).await?;
        // Replayed acceptance performs none of the replacement body's sources or agent effects.
        if let Some(previous) =
            rows::message_by_client(conn, id, caller.user(), body.client_id.trim()).await?
        {
            let mut plan = destination(ctx, &target)?;
            agents(&mut plan, &[])?;
            blob_sources::bind(&mut plan, &[])?;
            bind_names(&mut plan, &[])?;
            return Ok(plan.with_data(MessageEffects {
                replayed: Some(previous.id),
                blobs: Vec::new(),
                names: names::AllowSet::new(),
            }));
        }
        work.refuse(conn, Some(id), messages::check_parts(&body.parts, None))
            .await?;
        let mut required = Vec::new();
        let quoted = orch_chat::quotes::source_of(conn, &row, &body.parts).await;
        if let Some(quoted) = work.refuse(conn, Some(id), quoted).await? {
            required.push(blob_sources::Binding {
                source: blob_sources::SourceKind::Thread,
                target: targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Thread,
                        id: quoted.thread,
                    },
                )
                .await?,
            });
        }
        let receivers = asking::targets(&body.parts)?;
        let mut receiver_targets = Vec::new();
        if !receivers.is_empty() {
            // The existing forward copies source-chat content and attribution. Model/provider use
            // remains the ordinary input prerequisite, not another key read/manage action.
            required.push(blob_sources::Binding {
                source: blob_sources::SourceKind::Chat,
                target: target.clone(),
            });
            for receiver in receivers {
                receiver_targets.push(
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Thread,
                            id: receiver,
                        },
                    )
                    .await?,
                );
            }
        }
        let mut plan = destination(ctx, &target)?;
        agents(&mut plan, &receiver_targets)?;
        let hashes = orch_chat::attachments::blobs_in(&body.parts);
        let blobs = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &required).await?;
        blob_sources::bind(&mut plan, &blobs.bindings)?;
        let (name_bindings, name_sources) = if receiver_targets.is_empty() {
            (Vec::new(), names::AllowSet::new())
        } else {
            // The normal forwarding source is already scope-admitted before reading its reply rows.
            let reader = plan.preparation_principal(ctx, conn, caller).await?;
            let reply = match body.reply_to {
                Some(reply) => rows::message(conn, id, reply).await?,
                None => None,
            };
            let source_rows: Vec<_> = reply.iter().collect();
            let candidates =
                names::candidate_sources(conn, &source_rows, &body.parts, Some(&row)).await?;
            let allowed = allowed_names(conn, caller, &plan, candidates).await?;
            let mut naming = plan.clone();
            bind_names(&mut naming, &allowed)?;
            let reader = naming
                .preparation_principal(ctx, conn, reader.caller())
                .await?;
            let permitted: names::AllowSet = allowed.iter().map(|(source, _)| *source).collect();
            let captured = names::capture(
                conn,
                &reader,
                &source_rows,
                &body.parts,
                Some(&row),
                Some(&permitted),
            )
            .await?;
            let reference = names::chat_reference(conn, &reader, &row, Some(&permitted)).await?;
            let actual: names::AllowSet = captured
                .sources
                .union(&reference.sources)
                .copied()
                .collect();
            let bindings = allowed
                .into_iter()
                .filter(|(source, _)| actual.contains(source))
                .collect();
            (bindings, actual)
        };
        bind_names(&mut plan, &name_bindings)?;
        Ok(plan.with_data(MessageEffects {
            replayed: None,
            blobs: blobs.selected,
            names: name_sources,
        }))
    })
}

const NAME_KINDS: &[(KeyTargetKind, &str)] = &[
    (KeyTargetKind::Object(ObjectKind::Org), "message_name_orgs"),
    (
        KeyTargetKind::Object(ObjectKind::Project),
        "message_name_projects",
    ),
    (
        KeyTargetKind::Object(ObjectKind::Topic),
        "message_name_topics",
    ),
    (
        KeyTargetKind::Object(ObjectKind::Thread),
        "message_name_threads",
    ),
    (
        KeyTargetKind::Identity(KeyIdentityKind::Group),
        "message_name_groups",
    ),
];
fn name_slot(plan: &OperationPlan, kind: KeyTargetKind) -> ApiResult<usize> {
    let role = NAME_KINDS
        .iter()
        .find_map(|(wanted, role)| (*wanted == kind).then_some(*role))
        .ok_or_else(|| ApiError::internal("the protected name family is not declared"))?;
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the protected name collection is not declared"))
}
pub(super) fn bind_names(
    plan: &mut OperationPlan,
    names: &[(names::NameSource, KeyResolvedTarget)],
) -> ApiResult<()> {
    for (kind, _) in NAME_KINDS {
        let slot = name_slot(plan, *kind)?;
        let targets: Vec<_> = names
            .iter()
            .filter(|(_, target)| target.object.kind == *kind)
            .map(|(_, target)| target.clone())
            .collect();
        if targets.is_empty() {
            plan.bind_empty(slot)?;
        } else {
            plan.bind(slot, *kind, &targets)?;
        }
    }
    Ok(())
}
pub(super) async fn allowed_names(
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    candidates: names::AllowSet,
) -> ApiResult<Vec<(names::NameSource, KeyResolvedTarget)>> {
    // The picker can supply an unbounded set of current candidates. Reuse the
    // same canonical Group and Graph facts in batches, retaining candidate order
    // and the original group-page liveness rule before the unchanged scope test.
    let groups: Vec<_> = candidates
        .iter()
        .filter_map(|source| match source {
            names::NameSource::Group(id) => Some(*id),
            _ => None,
        })
        .collect();
    let groups = crate::routes::access::operations::group_targets(conn, &groups).await?;
    let seeds: Vec<_> = candidates
        .iter()
        .filter_map(|source| match source {
            names::NameSource::Node(id) => Some(*id),
            _ => None,
        })
        .chain(
            groups
                .iter()
                .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id)),
        )
        .collect();
    let graph = orch_authz::engine::Graph::load(conn, &seeds)
        .await
        .map_err(ApiError::internal)?;
    let mut allowed = Vec::new();
    for source in candidates {
        let resolved = match source {
            names::NameSource::Node(id) => {
                orch_engine::operation_scope::targets::from_node_identity(&graph, id)
            }
            names::NameSource::Group(id) => groups
                .iter()
                .find(|target| target.object.id == Some(id))
                .filter(|target| {
                    target
                        .node_ancestors
                        .last()
                        .and_then(|node| node.id)
                        .is_some_and(|home| graph.site_of(home).is_some())
                })
                .cloned(),
        };
        let Some(target) = resolved else { continue };
        if plan.permits_binding(
            caller,
            name_slot(plan, target.object.kind)?,
            target.object.kind,
            std::slice::from_ref(&target),
        )? {
            allowed.push((source, target));
        }
    }
    Ok(allowed)
}

/// Stored chat material stays readable under Chat Read, while newly resolved protected names have
/// their own same-operation source bound. This also covers both idempotent replay branches.
pub(super) async fn read_message(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    row: &MessageRow,
    seq: orch_types::Seq,
) -> ApiResult<Option<ChatMessageCreateReply>> {
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let target = targets::chat(conn, row.chat_id).await?;
    let mut base = destination(ctx, &target)?;
    agents(&mut base, &[])?;
    blob_sources::bind(&mut base, &[])?;
    read_message_with_plan(ctx, conn, current, row, seq, base).await
}

/// Project the accepted row with this exact operation's Read and live-name sources.
/// Each writer supplies its own canonical effect plan; the response reader never
/// borrows the post operation or infers a different action from its wire shape.
pub(super) async fn read_message_with_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    row: &MessageRow,
    seq: orch_types::Seq,
    mut base: OperationPlan,
) -> ApiResult<Option<ChatMessageCreateReply>> {
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let replies = messages::replies_of(conn, row.chat_id, std::slice::from_ref(row))
        .await
        .map_err(ApiError::internal)?;
    let rows: Vec<_> = std::iter::once(row).chain(replies.values()).collect();
    let candidates = names::candidate_sources(conn, &rows, &[], None).await?;
    let permitted = allowed_names(conn, current.caller(), &base, candidates).await?;
    let mut preparation = base.clone();
    bind_names(&mut preparation, &permitted)?;
    let reader = preparation
        .preparation_principal(ctx, conn, current.caller())
        .await?;
    let allowed: names::AllowSet = permitted.iter().map(|(source, _)| *source).collect();
    let projected = names::messages(
        conn,
        &reader,
        row.chat_id,
        std::slice::from_ref(row),
        Some(&allowed),
    )
    .await?;
    let actual = allowed_names(conn, current.caller(), &base, projected.sources).await?;
    bind_names(&mut base, &actual)?;
    let allowed: names::AllowSet = actual.into_iter().map(|(source, _)| source).collect();
    let row = row.clone();
    let result = base
        .read_response(ctx, conn, current.caller(), move |conn, reader| {
            Box::pin(async move {
                let audit = orch_db::audit::Origin::default();
                let acting = Acting {
                    principal: reader,
                    now: reader.now().get(),
                    origin: &audit,
                };
                match chats::load(conn, &acting, row.chat_id, Mode::Read).await {
                    Ok(_) => {}
                    Err(
                        ChatError::NotFound | ChatError::Lacks { .. } | ChatError::Suspended { .. },
                    ) => return Ok(None),
                    Err(error) => return Err(ApiError::internal(error)),
                }
                let mut result = names::messages(
                    conn,
                    reader,
                    row.chat_id,
                    std::slice::from_ref(&row),
                    Some(&allowed),
                )
                .await?
                .messages;
                let message = result
                    .pop()
                    .ok_or_else(|| ApiError::internal("the accepted message is not projected"))?;
                Ok(Some(ChatMessageCreateReply { message, seq }))
            })
        })
        .await?;
    Ok(result.flatten())
}
