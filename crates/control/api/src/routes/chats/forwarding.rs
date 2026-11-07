//! The existing atomic forward, with current source/destination facts and typed response projection.

use super::*;
use crate::key_operations::{blob_sources, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;
use orch_engine::sharing::ForwardInputSource;

pub(super) struct PreparedForward {
    pub carried: Carried,
    pub chats: Vec<forward::PlannedDestination>,
    pub threads: Vec<Id>,
    pub source: ForwardInputSource,
    pub blobs: Vec<(orch_types::Hash, following::BlobOrigin)>,
}
fn slot(plan: &OperationPlan, role: &'static str) -> ApiResult<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
        .ok_or_else(|| ApiError::internal("the forward collection is not declared"))
}
fn bind(
    plan: &mut OperationPlan,
    role: &'static str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = slot(plan, role)?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}
fn empty_copy_sources(plan: &mut OperationPlan) -> ApiResult<()> {
    bind(
        plan,
        "copy_source_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &[],
    )?;
    bind(
        plan,
        "copy_source_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
        &[],
    )
}
async fn source_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    body: &ForwardCreateBody,
) -> ApiResult<(OperationPlan, Id, ForwardInputSource)> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    match &body.from {
        ForwardSource::Chat(from) => {
            let row = rows::chat(conn, from.chat_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = targets::from_chat(conn, &row).await?;
            bind(
                &mut plan,
                "copy_source_chats",
                KeyTargetKind::Identity(KeyIdentityKind::Chat),
                &[target],
            )?;
            bind(
                &mut plan,
                "copy_source_threads",
                KeyTargetKind::Object(ObjectKind::Thread),
                &[],
            )?;
            Ok((
                plan,
                row.org_id,
                ForwardInputSource::Chat {
                    chat_id: from.chat_id,
                    message_ids: from.message_ids.clone(),
                },
            ))
        }
        ForwardSource::Thread(from) => {
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: from.thread_id,
                },
            )
            .await?;
            let found = hierarchy::with_ancestors(conn, &[from.thread_id]).await?;
            let node = found
                .iter()
                .find(|node| node.id == from.thread_id && node.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            bind(
                &mut plan,
                "copy_source_chats",
                KeyTargetKind::Identity(KeyIdentityKind::Chat),
                &[],
            )?;
            bind(
                &mut plan,
                "copy_source_threads",
                KeyTargetKind::Object(ObjectKind::Thread),
                &[target],
            )?;
            Ok((
                plan,
                node.org_id,
                ForwardInputSource::Thread {
                    thread_id: from.thread_id,
                    item_ids: from.item_ids.clone(),
                },
            ))
        }
    }
}
async fn bind_destinations(
    plan: &mut OperationPlan,
    conn: &mut Conn,
    chats: &[forward::PlannedDestination],
    threads: &[Id],
) -> ApiResult<()> {
    let mut existing = Vec::new();
    let mut parents = Vec::new();
    for destination in chats {
        match destination.state {
            forward::DestinationState::ExistingChat(id) => existing.push(
                targets::chat(conn, id)
                    .await
                    .map_err(|error| error.with("object", destination.requested.named()))?,
            ),
            forward::DestinationState::NewDirect { root, .. } => parents.push(
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Org,
                        id: root,
                    },
                )
                .await
                .map_err(|error| error.with("object", destination.requested.named()))?,
            ),
        }
    }
    let mut inputs = Vec::new();
    for id in threads {
        inputs.push(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: *id,
                },
            )
            .await
            .map_err(|error| error.with("object", Destination::Thread(*id).named()))?,
        );
    }
    bind(
        plan,
        "forward_destination_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &existing,
    )?;
    bind(
        plan,
        "forward_new_chat_parents",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &parents,
    )?;
    bind(
        plan,
        "forward_destination_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
        &inputs,
    )?;
    if parents.is_empty() {
        bind(
            plan,
            "response_chats",
            KeyTargetKind::Identity(KeyIdentityKind::Chat),
            &existing,
        )?;
    }
    // A new direct chat's exact response identity is resolved only after the ordinary writer creates it.
    bind(
        plan,
        "response_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
        &inputs,
    )
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ForwardCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let to: Vec<_> = body.to.iter().map(destination_of).collect();
        if to.is_empty()
            || to.len() > usize::try_from(FORWARD_MAX_DESTINATIONS).unwrap_or(usize::MAX)
        {
            return Err(ApiError::invalid(
                "to",
                &format!("name 1 to {FORWARD_MAX_DESTINATIONS} destinations"),
            ));
        }
        let (mut plan, root, source) = source_plan(ctx, conn, &body).await?;
        let initial = plan.preparation_principal(ctx, conn, caller).await?;
        let (chats_to, threads_to): (Vec<_>, Vec<_>) = to
            .into_iter()
            .partition(|to| !matches!(to, Destination::Thread(_)));
        let threads: Vec<_> = threads_to
            .into_iter()
            .filter_map(|to| match to {
                Destination::Thread(id) => Some(id),
                _ => None,
            })
            .collect();
        let chats = if chats_to.is_empty() {
            Vec::new()
        } else {
            forward::destination_plan(conn, caller.user(), root, &chats_to)
                .await
                .map_err(|e| ApiError::internal(e))?
        };
        bind_destinations(&mut plan, conn, &chats, &threads).await?;
        let principal = plan
            .preparation_principal(ctx, conn, initial.caller())
            .await?;
        let work = Work {
            projection_ctx: ctx.clone(),
            origin: origin(ctx),
            now: principal.now().get(),
            principal,
        };
        let (carried, naming) = carried(ctx, conn, caller, &plan, &work, &body.from, root).await?;
        operation::bind_names(&mut plan, &naming)?;
        // The copied parts already have an authorized source; retained hashes still select a current
        // complete readable witness, through the same shared helper used for direct input/posting.
        let copies_needed = !threads.is_empty()
            || chats.iter().any(|to| match (&carried.quotes, &to.state) {
                (Some((_, own)), forward::DestinationState::ExistingChat(chat)) => chat != own,
                _ => true,
            });
        // A live quote in the source thread's own chat posts item IDs, not copied blobs.
        let hashes = if copies_needed {
            orch_chat::attachments::blobs_in(&carried.copies)
        } else {
            Vec::new()
        };
        let blobs = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
        blob_sources::bind(&mut plan, &blobs.bindings)?;
        Ok(plan.with_data(PreparedForward {
            carried,
            chats,
            threads,
            source,
            blobs: blobs.selected,
        }))
    })
}

async fn carried(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    work: &Work,
    from: &ForwardSource,
    root: Id,
) -> ApiResult<(Carried, Vec<(names::NameSource, KeyResolvedTarget)>)> {
    match from {
        ForwardSource::Chat(from) => {
            let prepared =
                forward::prepare(conn, &work.acting(), from.chat_id, &from.message_ids).await;
            let prepared = work.refuse(conn, Some(from.chat_id), prepared).await?;
            let rows: Vec<_> = prepared.messages.iter().collect();
            let candidates =
                names::candidate_sources(conn, &rows, &[], Some(&prepared.chat.row)).await?;
            let allowed = operation::allowed_names(conn, caller, plan, candidates).await?;
            let mut naming = plan.clone();
            operation::bind_names(&mut naming, &allowed)?;
            let reader = naming.preparation_principal(ctx, conn, caller).await?;
            let allowed_set: names::AllowSet = allowed.iter().map(|(source, _)| *source).collect();
            let captured = names::capture(
                conn,
                &reader,
                &rows,
                &[],
                Some(&prepared.chat.row),
                Some(&allowed_set),
            )
            .await?;
            let source =
                names::chat_reference(conn, &reader, &prepared.chat.row, Some(&allowed_set))
                    .await?;
            let materialized: names::AllowSet =
                captured.sources.union(&source.sources).copied().collect();
            let bindings = allowed
                .into_iter()
                .filter(|(source, _)| materialized.contains(source))
                .collect();
            Ok((
                Carried {
                    root,
                    copies: forward::copies_of(&prepared, &source.reference, &captured.lookups),
                    quotes: None,
                },
                bindings,
            ))
        }
        ForwardSource::Thread(from) => {
            match nodes::check(
                conn,
                &work.principal,
                from.thread_id,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await
            {
                Ok(Decision::Allowed { .. }) => {}
                Ok(Decision::Refused(refusal)) => {
                    let denied = refusals::denied(conn, refusal).await?;
                    return Err(if ctx.key_receipt_required() {
                        authorization::problem_without_content(denied)
                    } else {
                        denied
                    });
                }
                Err(error) => {
                    let failed = refusals::problem(conn, error).await;
                    return Err(if ctx.key_receipt_required() {
                        authorization::problem_without_content(failed)
                    } else {
                        failed
                    });
                }
            }
            let own = rows::chat_of_node(conn, from.thread_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let quotes = forward::quote_items(conn, from.thread_id, &from.item_ids).await;
            let quotes = work.refuse(conn, Some(own.id), quotes).await?;
            let parts = written::forward_parts(conn, from.thread_id, &from.item_ids)
                .await
                .map_err(ApiError::internal)?;
            if parts.len() > usize::try_from(FORWARD_MAX_PARTS).unwrap_or(usize::MAX) {
                return Err(ApiError::invalid(
                    "from.item_ids",
                    &format!("come to more than the {FORWARD_MAX_PARTS} parts of a forward"),
                ));
            }
            let sources =
                std::collections::BTreeSet::from([names::NameSource::Node(from.thread_id)]);
            let allowed = operation::allowed_names(conn, caller, plan, sources).await?;
            let allowed_ids = allowed
                .iter()
                .filter_map(|(source, _)| match source {
                    names::NameSource::Node(id) => Some(*id),
                    _ => None,
                })
                .collect();
            let refs = view::refs_with_name_sources(
                conn,
                &work.principal,
                &[from.thread_id],
                Policy::default(),
                &allowed_ids,
            )
            .await?;
            let reference = refs
                .get(&from.thread_id)
                .cloned()
                .ok_or_else(ApiError::not_found)?;
            let bindings = if reference.name.is_some() || reference.thread_display.is_some() {
                allowed
            } else {
                Vec::new()
            };
            let copies: Vec<_> = parts
                .into_iter()
                .filter(|part| !part.text.trim().is_empty() || !part.images.is_empty())
                .map(|part| {
                    ChatPart::Forwarded(Box::new(Forwarded {
                        r#type: orch_api_contract::threads::ForwardedType::Forwarded,
                        from: ForwardedFrom::Thread {
                            thread: reference.clone(),
                        },
                        source_id: part.source,
                        author: match (part.person, part.agent) {
                            (Some(person), _) => Some(ForwardedAuthor::User(person)),
                            (None, Some(agent)) => {
                                Some(ForwardedAuthor::Agent(AgentName { agent }))
                            }
                            _ => None,
                        },
                        at: part.at,
                        text: part.text,
                        images: part.images,
                        attachments: Vec::new(),
                    }))
                })
                .collect();
            if copies.is_empty() {
                return Err(ApiError::invalid("from.item_ids", "say nothing to forward"));
            }
            Ok((
                Carried {
                    root,
                    copies,
                    quotes: Some((quotes, own.id)),
                },
                bindings,
            ))
        }
    }
}

async fn response_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    chats: &[Id],
    threads: &[Id],
) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    empty_copy_sources(&mut plan)?;
    bind(
        &mut plan,
        "forward_destination_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &[],
    )?;
    bind(
        &mut plan,
        "forward_destination_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
        &[],
    )?;
    bind(
        &mut plan,
        "forward_new_chat_parents",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &[],
    )?;
    blob_sources::bind(&mut plan, &[])?;
    let mut chat_targets = Vec::new();
    for id in chats {
        chat_targets.push(targets::chat(conn, *id).await?);
    }
    let mut thread_targets = Vec::new();
    for id in threads {
        thread_targets.push(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Thread,
                    id: *id,
                },
            )
            .await?,
        );
    }
    bind(
        &mut plan,
        "response_chats",
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &chat_targets,
    )?;
    bind(
        &mut plan,
        "response_threads",
        KeyTargetKind::Object(ObjectKind::Thread),
        &thread_targets,
    )?;
    Ok(plan)
}

pub(super) async fn read_response(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    sent: &[forward::Sent],
    inputs: &[ForwardInputReceipt],
) -> ApiResult<Option<ForwardCreateReply>> {
    let chat_ids: Vec<_> = sent.iter().map(|s| s.chat).collect();
    let thread_ids: Vec<_> = inputs.iter().map(|i| i.thread_id).collect();
    // First admit the actual response resources, including direct chats whose IDs did not exist
    // until the ordinary writer ran. No destination content is built before this succeeds.
    let mut initial = response_plan(ctx, conn, &chat_ids, &thread_ids).await?;
    operation::bind_names(&mut initial, &[])?;
    let Some(reader) = initial
        .read_response(ctx, conn, current.caller(), |_, reader| {
            Box::pin(async move { Ok(reader.clone()) })
        })
        .await?
    else {
        return Ok(None);
    };
    let mut candidates = names::AllowSet::new();
    for each in sent {
        let replies =
            messages::replies_of(conn, each.chat, std::slice::from_ref(&each.written.message))
                .await
                .map_err(ApiError::internal)?;
        let all: Vec<_> = std::iter::once(&each.written.message)
            .chain(replies.values())
            .collect();
        candidates.extend(names::candidate_sources(conn, &all, &[], None).await?);
    }
    let base = response_plan(ctx, conn, &chat_ids, &thread_ids).await?;
    let allowed = operation::allowed_names(conn, reader.caller(), &base, candidates).await?;
    let set: names::AllowSet = allowed.iter().map(|(source, _)| *source).collect();
    let mut materialized = names::AllowSet::new();
    for each in sent {
        materialized.extend(
            names::messages(
                conn,
                &reader,
                each.chat,
                std::slice::from_ref(&each.written.message),
                Some(&set),
            )
            .await?
            .sources,
        );
    }
    let mut final_read = response_plan(ctx, conn, &chat_ids, &thread_ids).await?;
    let actual = operation::allowed_names(conn, reader.caller(), &final_read, materialized).await?;
    operation::bind_names(&mut final_read, &actual)?;
    let allowed: names::AllowSet = actual.into_iter().map(|(source, _)| source).collect();
    let sent = sent.to_vec();
    let inputs = inputs.to_vec();
    let read = final_read
        .read_response(ctx, conn, reader.caller(), move |conn, reader| {
            Box::pin(async move {
                let origin = orch_db::audit::Origin::default();
                let acting = Acting {
                    principal: reader,
                    now: reader.now().get(),
                    origin: &origin,
                };
                let mut chats_written = Vec::new();
                for each in sent {
                    match chats::load(conn, &acting, each.chat, Mode::Read).await {
                        Ok(_) => {}
                        Err(
                            ChatError::NotFound
                            | ChatError::Lacks { .. }
                            | ChatError::Suspended { .. },
                        ) => return Ok(None),
                        Err(error) => return Err(ApiError::internal(error)),
                    }
                    let mut shown = names::messages(
                        conn,
                        reader,
                        each.chat,
                        std::slice::from_ref(&each.written.message),
                        Some(&allowed),
                    )
                    .await?
                    .messages;
                    let message = shown.pop().ok_or_else(|| {
                        ApiError::internal("an accepted forward message was not projected")
                    })?;
                    chats_written.push(ForwardChatMessage {
                        chat_id: each.chat,
                        message,
                    });
                }
                let mut threads_written = Vec::new();
                for receipt in inputs {
                    // Readback is through the ordinary current thread view, not through source ownership.
                    let can_read = nodes::check(
                        conn,
                        reader,
                        receipt.thread_id,
                        Permission::ThreadView,
                        Act::Use,
                        Policy::default(),
                    )
                    .await;
                    match can_read {
                        Ok(Decision::Allowed { .. }) => {}
                        Ok(Decision::Refused(_))
                        | Err(
                            orch_authz::Error::NotFound(_) | orch_authz::Error::Suspended { .. },
                        ) => return Ok(None),
                        Err(error) => return Err(ApiError::internal(error)),
                    }
                    let row = orch_db::queries::threads::input(conn, receipt.input_id)
                        .await?
                        .filter(|row| row.thread_id == receipt.thread_id)
                        .ok_or_else(ApiError::not_found)?;
                    let people = orch_engine::views::People::load(
                        conn,
                        row.author_id.into_iter().chain(row.run_as),
                    )
                    .await
                    .map_err(ApiError::internal)?
                    .with_approvals(conn, row.approval_id)
                    .await
                    .map_err(ApiError::internal)?;
                    let input = orch_engine::views::input_view(&row, &people)
                        .map_err(ApiError::internal)?;
                    threads_written.push(ForwardThreadInput {
                        thread_id: receipt.thread_id,
                        input,
                    });
                }
                Ok(Some(ForwardCreateReply {
                    chats: chats_written,
                    threads: threads_written,
                }))
            })
        })
        .await?;
    Ok(read.flatten())
}
