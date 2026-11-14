//! Rebind every accepted input origin to the actual stored input at this transaction checkpoint.

use super::authority::InputAuthority;
use super::{InputCallers, InputError, InputRequest, operation};
use crate::EngineError;
use crate::operation_scope::{BindingError, Prepared, blob_sources, targets};
use crate::unit::Unit;
use orch_access_contract::key_scope::{
    KeyAction, KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::{self as api, ContentPart, Delivery};
use orch_authz::{Caller, Principal};
use orch_db::queries::threads::{InputRow, ThreadRow};
use orch_types::{Clock, Id, Json};

/// Only current input operands, never a stored permission or target-path snapshot.
struct Materialized {
    content: Json,
    native_action: Option<Json>,
    delivery: String,
    limit_bypass: Option<Json>,
    answers: Option<Id>,
    accepted_bypass: bool,
}

impl Materialized {
    fn stored(input: &InputRow) -> Self {
        Self {
            content: input.content.clone(),
            native_action: input.native_action.clone(),
            delivery: input.delivery.clone(),
            limit_bypass: input.limit_bypass.clone(),
            answers: input.answers(),
            accepted_bypass: true,
        }
    }

    fn requested(
        request: &InputRequest,
        selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
        actor: Id,
    ) -> Result<Self, EngineError> {
        let limit_bypass = request.bypass.as_ref().map(|bypass| {
            serde_json::json!([{
                "by": actor, "caps": bypass.caps.as_deref().unwrap_or(&[]),
                "provider_limit": bypass.provider_limit == Some(true)
            }])
        });
        Ok(Self {
            content: serde_json::to_value(&request.content)?,
            native_action: selection.map(serde_json::to_value).transpose()?,
            delivery: super::delivery_word(request.delivery).into(),
            limit_bypass,
            answers: Some(actor),
            accepted_bypass: false,
        })
    }
}

fn bound<T>(result: Result<T, BindingError>) -> Result<Option<T>, EngineError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(BindingError::Refused) => Ok(None),
        Err(error) => Err(EngineError::unreadable(error)),
    }
}

fn target_result<T>(result: Result<T, targets::TargetError>) -> Result<Option<T>, EngineError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(targets::TargetError::Missing) => Ok(None),
        Err(targets::TargetError::Authz(
            orch_authz::Error::NotFound(_)
            | orch_authz::Error::Archived { .. }
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::NoSuchUser(_),
        )) => Ok(None),
        Err(targets::TargetError::Authz(error)) => Err(error.into()),
        Err(targets::TargetError::Db(error)) => Err(error.into()),
    }
}

fn source_result<T>(result: Result<T, blob_sources::Error>) -> Result<Option<T>, EngineError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(
            blob_sources::Error::Missing | blob_sources::Error::Source(orch_chat::Error::NotFound),
        ) => Ok(None),
        Err(blob_sources::Error::Binding(error)) => bound(Err(error)),
        Err(blob_sources::Error::Target(error)) => target_result(Err(error)),
        Err(blob_sources::Error::Source(orch_chat::Error::Db(error))) => Err(error.into()),
        Err(error) => Err(EngineError::unreadable(error)),
    }
}

fn ordinary_input_origin(plan: &Prepared) -> bool {
    if plan.matches_route(&orch_api_contract::approvals::ApprovalDecide::ROUTE) {
        return matches!(
            plan.variant().name.as_str(),
            "plan_interact" | "plan_manage" | "input"
        );
    }
    [
        api::ThreadInputCreate::ROUTE,
        api::ThreadInputUpdate::ROUTE,
        api::ThreadInputRun::ROUTE,
        api::ThreadInputRetry::ROUTE,
        api::ThreadTurnRetry::ROUTE,
        api::ThreadPlanDecide::ROUTE,
        api::ThreadPromptVersionCreate::ROUTE,
        api::ThreadContinuationActivate::ROUTE,
        orch_api_contract::goals::ThreadGoalCreate::ROUTE,
        orch_api_contract::goals::ThreadGoalUpdate::ROUTE,
    ]
    .iter()
    .any(|route| plan.matches_route(route))
}

struct MaterializedContext<'a> {
    thread: &'a ThreadRow,
    input: &'a Materialized,
    selection: &'a str,
    target: &'a KeyResolvedTarget,
    scheduled: Option<super::ScheduledInputOrigin>,
    chat: Option<super::ChatInputOrigin>,
    forward: Option<&'a super::ForwardInputOrigin>,
    snapshot_comment: Option<super::SnapshotCommentOrigin>,
    complete: bool,
}

async fn materialized_origin(
    unit: &mut Unit<'_>,
    caller: &Caller,
    context: MaterializedContext<'_>,
    clock: &dyn Clock,
) -> Result<Option<Caller>, EngineError> {
    let MaterializedContext {
        thread,
        input,
        selection,
        target,
        scheduled,
        chat,
        forward,
        snapshot_comment,
        complete,
    } = context;
    // A restored browser has no additional credential policy to admit. Keep its existing
    // ordinary input/native checks in their original order; do not turn stale native state
    // into a new browser authorization refusal merely to compute a restriction it lacks.
    if caller.grant().is_none() {
        return Ok(Some(caller.clone()));
    }
    let Some(mut plan) = bound(Prepared::for_selection(selection))? else {
        return Ok(None);
    };
    if bound(plan.compatible(caller))?.is_none() {
        return Ok(None);
    }
    if caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_none())
    {
        // The original permission/subtree/lifetime bounds remain on this same restored caller;
        // ordinary producers enforce them. Legacy work gains no new endpoint compatibility.
        return Ok(Some(caller.clone()));
    }
    // Stable schedule/run references describe producer intent, never stored target ancestry.
    let schedule_origin = plan
        .matches_route(&orch_api_contract::input_schedules::TopicInputScheduleCreate::ROUTE)
        || (plan.matches_route(&orch_api_contract::input_schedules::InputScheduleUpdate::ROUTE)
            && plan.variant().name == "definition");
    let chat_origin = plan.matches_route(&orch_api_contract::chats::ChatMessageCreate::ROUTE);
    let forward_origin = plan.matches_route(&orch_api_contract::chats::ForwardCreate::ROUTE);
    let snapshot_origin = plan.matches_route(&orch_api_contract::share::ShareAgentCreate::ROUTE)
        && plan.variant().name == "snapshot_comment";
    let mut scheduled_destination = None;
    let mut creator_destination = None;
    let creator_origin = matches!(selection, "ThreadCreate" | "TopicCreate.with_thread");
    if snapshot_origin {
        let Some(reference) =
            snapshot_comment.filter(|reference| reference.thread_id == thread.node_id)
        else {
            return Ok(None);
        };
        let Some(anchor) = orch_db::queries::threads::input(unit.conn, reference.input_id).await?
        else {
            return Ok(None);
        };
        let Some(accepted) = super::saved_authority(anchor.submission_authority.as_ref())? else {
            return Ok(None);
        };
        if anchor.thread_id != thread.node_id
            || anchor.author_id != Some(caller.user())
            || accepted.snapshot_comment() != Some(reference)
            || !accepted.has_snapshot_comment_origin(caller.user())
        {
            return Ok(None);
        }
        if let Some(copy) = orch_db::queries::shares::copy(unit.conn, reference.copy_id).await?
            && (copy.thread_id != reference.thread_id
                || copy.created_by != caller.user()
                || super::SnapshotCommentOrigin::source_of(&copy)? != reference.source)
        {
            return Ok(None);
        }
        // The normal copy producer completed source reads and parcel capture before accepting
        // the comment. Neither source deletion nor copy withdrawal reinterprets that past read;
        // parcel delivery keeps its own existing state checks.
        for role in [
            "snapshot_source_orgs",
            "snapshot_source_projects",
            "snapshot_source_topics",
            "snapshot_source_threads",
            "snapshot_source_documents",
            "snapshot_source_document_containers",
            "snapshot_source_environments",
            "snapshot_source_chats",
        ] {
            let Some(index) = plan
                .variant()
                .facets
                .iter()
                .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            else {
                return Ok(None);
            };
            if bound(plan.bind_empty(index))?.is_none() {
                return Ok(None);
            }
        }
        let Some(index) = plan.variant().facets.iter().position(|facet| {
            facet.target == TargetResolver::ResolvedCollection("snapshot_recipient_threads")
        }) else {
            return Ok(None);
        };
        if bound(plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(target),
        ))?
        .is_none()
        {
            return Ok(None);
        }
    } else if forward_origin {
        let Some(reference) = forward.filter(|reference| reference.thread_id == thread.node_id)
        else {
            return Ok(None);
        };
        let selected = match &reference.source {
            super::ForwardInputSource::Chat { message_ids, .. } => !message_ids.is_empty(),
            super::ForwardInputSource::Thread { item_ids, .. } => !item_ids.is_empty(),
        };
        if !selected {
            return Ok(None);
        }
        // The accepting ordinary producer verified source IDs before copying. The original input
        // anchor corroborates that accepted source/receiver/actor relationship; derived Continue
        // inputs keep the same anchor. No copied part, client_id, or label becomes authority.
        let Some(anchor) = orch_db::queries::threads::input(unit.conn, reference.input_id).await?
        else {
            return Ok(None);
        };
        let Some(accepted) = super::saved_authority(anchor.submission_authority.as_ref())? else {
            return Ok(None);
        };
        if anchor.thread_id != thread.node_id
            || anchor.author_id != Some(caller.user())
            || accepted.forward() != Some(reference)
            || !accepted.has_forward_origin(caller.user())
        {
            return Ok(None);
        }
        for role in [
            "copy_source_chats",
            "copy_source_threads",
            "forward_destination_chats",
            "forward_new_chat_parents",
        ] {
            let Some(index) = plan
                .variant()
                .facets
                .iter()
                .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            else {
                return Ok(None);
            };
            if bound(plan.bind_empty(index))?.is_none() {
                return Ok(None);
            }
        }
        let Some(index) = plan.variant().facets.iter().position(|facet| {
            facet.target == TargetResolver::ResolvedCollection("forward_destination_threads")
        }) else {
            return Ok(None);
        };
        if bound(plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(target),
        ))?
        .is_none()
        {
            return Ok(None);
        }
    } else {
        let (subject, family) = if chat_origin {
            let Some(reference) = chat.filter(|reference| reference.thread_id == thread.node_id)
            else {
                return Ok(None);
            };
            let row = orch_db::queries::chats::message_origin(
                unit.conn,
                reference.chat_id,
                reference.message_id,
            )
            .await?;
            match row {
                Some(row) => {
                    if row.id != reference.message_id
                        || row.chat_id != reference.chat_id
                        || row.author_thread_id.is_some()
                        || row.author_id.is_some_and(|author| author != caller.user())
                        || (row.deleted_at.is_none()
                            && (row.kind != "message" || row.author_id != Some(caller.user())))
                    {
                        return Ok(None);
                    }
                    // Deletion clears display/association fields, but does not undo a completed copy.
                    // Live retained messages must still name the exact initially allocated input.
                    if row.deleted_at.is_none()
                        && !row.asked_agents.as_array().is_some_and(|asked| {
                            asked.iter().any(|item| {
                                item.get("thread_id")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(|id| id.parse::<Id>().ok())
                                    == Some(reference.thread_id)
                                    && item
                                        .get("input_id")
                                        .and_then(serde_json::Value::as_str)
                                        .and_then(|id| id.parse::<Id>().ok())
                                        == Some(reference.input_id)
                            })
                        })
                    {
                        return Ok(None);
                    }
                }
                None => {
                    // If message retention removed its row, verify the same accepted association from
                    // its retained input rather than reconstructing message text or parsing client_id.
                    let Some(anchor) =
                        orch_db::queries::threads::input(unit.conn, reference.input_id).await?
                    else {
                        return Ok(None);
                    };
                    let accepted = super::saved_authority(anchor.submission_authority.as_ref())?;
                    if anchor.thread_id != reference.thread_id
                        || anchor.author_id != Some(caller.user())
                        || accepted.as_ref().and_then(|authority| authority.chat())
                            != Some(reference)
                    {
                        return Ok(None);
                    }
                }
            }
            let Some(source) =
                target_result(targets::chat_identity(unit.conn, reference.chat_id).await)?
            else {
                return Ok(None);
            };
            (source, KeyTargetKind::Identity(KeyIdentityKind::Chat))
        } else if schedule_origin {
            let Some(reference) = scheduled else {
                return Ok(None);
            };
            let Some(row) = orch_db::queries::input_schedules::schedule(
                unit.conn,
                reference.schedule_id,
                false,
            )
            .await?
            else {
                return Ok(None);
            };
            let Some(run) =
                orch_db::queries::input_schedules::run_by_id(unit.conn, reference.run_id).await?
            else {
                return Ok(None);
            };
            if row.author_id != caller.user()
                || run.schedule_id != row.id
                || run.thread_id.is_some_and(|id| id != thread.node_id)
            {
                return Ok(None);
            }
            let destination = match reference.destination {
                super::ScheduledInputDestination::NewThread => {
                    let expected = crate::ids::derived(
                        "input-schedule.thread",
                        &[row.id.as_bytes(), run.occurrence.as_bytes()],
                    );
                    if thread.node_id != expected {
                        return Ok(None);
                    }
                    let Some(parent) =
                        target_result(targets::node(unit.conn, thread.topic_id).await)?
                    else {
                        return Ok(None);
                    };
                    parent
                }
                super::ScheduledInputDestination::ExistingThread => {
                    if run.thread_id.is_none() {
                        let requested: orch_api_contract::input_schedules::InputScheduleTarget =
                            serde_json::from_value(row.target.clone())?;
                        if !matches!(requested, orch_api_contract::input_schedules::InputScheduleTarget::Thread { thread_id } if thread_id == thread.node_id)
                        {
                            return Ok(None);
                        }
                    }
                    target.clone()
                }
            };
            let Some(mut owner) = target_result(targets::node(unit.conn, row.topic_id).await)?
            else {
                return Ok(None);
            };
            if plan.matches_route(&orch_api_contract::input_schedules::InputScheduleUpdate::ROUTE) {
                owner.object = KeyObjectRef {
                    kind: KeyTargetKind::Identity(KeyIdentityKind::Schedule),
                    id: Some(row.id),
                };
            }
            scheduled_destination = Some(destination);
            (owner, KeyTargetKind::Identity(KeyIdentityKind::Schedule))
        } else if ordinary_input_origin(&plan) {
            (target.clone(), KeyTargetKind::Object(ObjectKind::Thread))
        } else if plan.matches_route(&orch_api_contract::nodes::TopicCreate::ROUTE)
            && plan.variant().name == "with_thread"
        {
            let Some(topic) = target_result(targets::node(unit.conn, thread.topic_id).await)?
            else {
                return Ok(None);
            };
            let Some(parent) = topic
                .node_ancestors
                .iter()
                .rev()
                .find(|ancestor| ancestor.kind == KeyTargetKind::Object(ObjectKind::Project))
                .and_then(|ancestor| ancestor.id)
            else {
                return Ok(None);
            };
            let Some(parent) = target_result(targets::node(unit.conn, parent).await)? else {
                return Ok(None);
            };
            creator_destination = Some(topic);
            (parent, KeyTargetKind::Object(ObjectKind::Topic))
        } else if plan.matches_route(&orch_api_contract::nodes::ThreadCreate::ROUTE) {
            let Some(parent) = target_result(targets::node(unit.conn, thread.topic_id).await)?
            else {
                return Ok(None);
            };
            (parent, KeyTargetKind::Object(ObjectKind::Thread))
        } else if plan.matches_route(&api::ThreadSideCreate::ROUTE) {
            let Some(parent) = thread.side_of else {
                return Ok(None);
            };
            let Some(parent) = target_result(targets::node(unit.conn, parent).await)? else {
                return Ok(None);
            };
            (parent, KeyTargetKind::Object(ObjectKind::Thread))
        } else {
            // A nearby thread never substitutes for an opaque share/quote/copy source.
            return Ok(None);
        };
        let subjects: Vec<_> = plan
            .variant()
            .facets
            .iter()
            .enumerate()
            .filter_map(|(index, facet)| {
                (matches!(
                    facet.target,
                    TargetResolver::Path("id") | TargetResolver::PermissionObject
                ) || (plan.matches_route(&orch_api_contract::approvals::ApprovalDecide::ROUTE)
                    && matches!(
                        plan.variant().name.as_str(),
                        "plan_interact" | "plan_manage" | "input"
                    )
                    && facet.target == TargetResolver::Resolved("approval_subject")))
                .then_some(index)
            })
            .collect();
        for index in subjects {
            if bound(plan.bind(index, family, std::slice::from_ref(&subject)))?.is_none() {
                return Ok(None);
            }
        }
        if let Some(destination) = &creator_destination {
            let Some(index) = plan.variant().facets.iter().position(|facet| {
                facet.target == TargetResolver::Resolved("first_thread_destination")
            }) else {
                return Ok(None);
            };
            if bound(plan.bind(
                index,
                KeyTargetKind::Object(ObjectKind::Thread),
                std::slice::from_ref(destination),
            ))?
            .is_none()
            {
                return Ok(None);
            }
        }
        if let Some(destination) = &scheduled_destination {
            let Some(index) = plan.variant().facets.iter().position(|facet| {
                facet.target == TargetResolver::Resolved("scheduled_destinations")
            }) else {
                return Ok(None);
            };
            if bound(plan.bind(
                index,
                KeyTargetKind::Object(ObjectKind::Thread),
                std::slice::from_ref(destination),
            ))?
            .is_none()
            {
                return Ok(None);
            }
        }
    }

    if chat_origin {
        let Some(index) = plan.variant().facets.iter().position(|facet| {
            facet.target == TargetResolver::ResolvedCollection("message_agent_threads")
        }) else {
            return Ok(None);
        };
        if bound(plan.bind(
            index,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(target),
        ))?
        .is_none()
        {
            return Ok(None);
        }
    }
    if chat_origin || forward_origin || snapshot_origin {
        // Names and explicit text/quote snapshots were copied by the accepting chat producer.
        // No new name lookup occurs during input delivery. Blob retrieval below still needs a
        // complete current readable witness for every materialized hash and every origin.
        let names: Vec<_> = plan
            .variant()
            .facets
            .iter()
            .enumerate()
            .filter_map(|(index, facet)| {
                matches!(
                    facet.target,
                    TargetResolver::ResolvedCollection(
                        "message_name_orgs"
                            | "message_name_projects"
                            | "message_name_topics"
                            | "message_name_threads"
                            | "message_name_groups"
                            | "share_name_orgs"
                            | "share_name_projects"
                            | "share_name_topics"
                            | "share_name_threads"
                    )
                )
                .then_some(index)
            })
            .collect();
        for index in names {
            if bound(plan.bind_empty(index))?.is_none() {
                return Ok(None);
            }
        }
    }

    if creator_origin {
        // Creation, its accepted privilege gift and source-job scheduling already committed.
        // Input delivery performs none of those effects again. Its live Thread parent and
        // current retained byte witnesses remain above/below; file jobs restore their own
        // residual creator footprint rather than replaying these completed mutations.
        for role in [
            "creator_new_environment_homes",
            "creator_initial_file_threads",
            "creator_initial_file_environments",
            "creator_file_environments",
            "creator_file_sources",
            "creator_agent_projects",
            "creator_agent_topics",
            "creator_agent_threads",
        ] {
            let Some(index) = plan
                .variant()
                .facets
                .iter()
                .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            else {
                return Ok(None);
            };
            if bound(plan.bind_empty(index))?.is_none() {
                return Ok(None);
            }
        }
    }
    if creator_origin || schedule_origin {
        // The human audience gift belongs to birth, never to later input delivery.
        if let Some(index) = plan.variant().facets.iter().position(|facet| {
            facet.target == TargetResolver::ResolvedCollection("creator_reader_environments")
        }) && bound(plan.bind_empty(index))?.is_none()
        {
            return Ok(None);
        }
    }
    if !complete {
        return bound(plan.preparation_caller(caller));
    }

    if schedule_origin {
        // Input materialization performs no source-file preparation. The schedule writer
        // admits its actual creation effects, and the independent source job restores
        // every origin through the accepted input before its own issue/wire checkpoints.
        for role in ["creator_file_environments", "creator_file_sources"] {
            let Some(index) = plan
                .variant()
                .facets
                .iter()
                .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            else {
                return Ok(None);
            };
            if bound(plan.bind_empty(index))?.is_none() {
                return Ok(None);
            }
        }
    }

    let native_name = if schedule_origin {
        "scheduled_management"
    } else {
        "input_native_manage"
    };
    let native_role = plan
        .variant()
        .facets
        .iter()
        .any(|facet| facet.target == TargetResolver::ResolvedCollection(native_name));
    let permission = if let Some(raw) = &input.native_action {
        let selected = serde_json::from_value(raw.clone())?;
        let delivery = match input.delivery.as_str() {
            "steer" => Delivery::Steer,
            "interrupt" => Delivery::Interrupt,
            _ => Delivery::Queue,
        };
        match crate::native_actions::selected_action(unit.conn, thread.node_id, &selected, delivery)
            .await
        {
            Ok(action) => Some(action.permission),
            Err(crate::native_actions::NativeError::Internal(error)) => return Err(error),
            Err(_) => return Ok(None),
        }
    } else {
        None
    };
    if permission == Some(orch_api_contract::native::NativePermission::Manage)
        && !native_role
        && !plan.variant().facets.iter().any(|facet| {
            facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Manage
                && facet
                    .resources
                    .contains(&KeyTargetKind::Object(ObjectKind::Thread))
        })
    {
        return Ok(None);
    }
    if native_role
        && bound(operation::bind_native_collection(
            &mut plan,
            native_name,
            scheduled_destination.as_ref().unwrap_or(target),
            permission,
        ))?
        .is_none()
    {
        return Ok(None);
    }

    let bypass_role =
        plan.variant().facets.iter().any(|facet| {
            facet.target == TargetResolver::ResolvedCollection("input_bypass_providers")
        });
    if bypass_role {
        let homes = if input.accepted_bypass {
            // This row already records an accepted bypass grant. Its giver's CURRENT
            // ordinary right is checked by native caps.passed_by; a separate key of the
            // same account cannot lend/demand Manage from the original input operation.
            Vec::new()
        } else {
            let entries: Vec<_> = crate::caps::entries_of(input.limit_bypass.as_ref())
                .into_iter()
                .filter(|entry| entry.by == caller.user())
                .collect();
            let named: Vec<_> = entries
                .iter()
                .flat_map(|entry| entry.caps.iter().copied())
                .collect();
            let actor = input.answers.unwrap_or(caller.user());
            let reached = operation::caps_to_pass(unit, thread, actor, &named)
                .await
                .map_err(|error| match error {
                    InputError::Internal(error) => error,
                    other => EngineError::unreadable(other),
                })?;
            let homes: Vec<_> = reached.iter().map(crate::caps::home_of).collect();
            homes
        };
        match operation::bind_bypass(&mut plan, unit.conn, &homes).await {
            Ok(()) => {}
            Err(operation::InputOperationError::Binding(error)) => {
                if bound::<()>(Err(error))?.is_none() {
                    return Ok(None);
                }
            }
            Err(operation::InputOperationError::Target(error)) => {
                if target_result::<()>(Err(error))?.is_none() {
                    return Ok(None);
                }
            }
        }
    }

    let has_sources = plan.variant().facets.iter().any(|facet| {
        blob_sources::SOURCE_KINDS
            .iter()
            .any(|source| facet.target == TargetResolver::ResolvedCollection(source.role()))
    });
    if has_sources {
        let content: Vec<ContentPart> = serde_json::from_value(input.content.clone())?;
        let hashes = orch_chat::following::content_blobs(&content);
        let Some(preparing) = bound(plan.preparation_caller(caller))? else {
            return Ok(None);
        };
        let principal = match Principal::load(unit.conn, preparing, clock.time()).await {
            Ok(principal) => principal,
            Err(orch_authz::Error::NoSuchUser(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // Identical retained hashes permit any complete ordinary readable witness. Each origin
        // independently proves the whole requested hash; partial authorities are never combined.
        let Some(sources) =
            source_result(blob_sources::prepare(unit.conn, &principal, &plan, &hashes, &[]).await)?
        else {
            return Ok(None);
        };
        if source_result(blob_sources::bind(&mut plan, &sources.bindings))?.is_none() {
            return Ok(None);
        }
    }
    // Response content is not emitted by input execution. Unknown required source/effect slots
    // still fail here; they are never relabeled as empty or borrowed from another origin.
    bound(plan.admit(caller, false))
}

async fn restore_for_input(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &Materialized,
    actor: Id,
    authority: &InputAuthority,
    complete: bool,
    clock: &dyn Clock,
) -> Result<Option<(super::authority::RestoredInputAuthority, Vec<Caller>)>, EngineError> {
    let Some(restored) = authority.restore_current(unit.conn, actor, clock).await? else {
        return Ok(None);
    };
    let target = if restored
        .origins
        .iter()
        .any(|origin| origin.operation.is_some())
    {
        let Some(target) = target_result(targets::node(unit.conn, thread.node_id).await)? else {
            return Ok(None);
        };
        Some(target)
    } else {
        None
    };
    let mut callers = Vec::with_capacity(restored.origins.len());
    for origin in &restored.origins {
        let caller = if let (Some(selection), Some(target)) = (&origin.operation, &target) {
            let Some(caller) = materialized_origin(
                unit,
                &origin.caller,
                MaterializedContext {
                    thread,
                    input,
                    selection,
                    target,
                    scheduled: restored.captured.scheduled(),
                    chat: restored.captured.chat(),
                    forward: restored.captured.forward(),
                    snapshot_comment: restored.captured.snapshot_comment(),
                    complete,
                },
                clock,
            )
            .await?
            else {
                return Ok(None);
            };
            caller
        } else {
            origin.caller.clone()
        };
        callers.push(caller);
    }
    if callers
        .iter()
        .any(|caller| !caller.key_deadline_allows(clock))
    {
        return Ok(None);
    }
    Ok(Some((restored, callers)))
}

pub(super) async fn for_input(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    if input.thread_id != thread.node_id {
        return Ok(None);
    }
    if let Some(parent) = authority.capability_parent()
        && (!parent.owns_input(unit.conn, input.id).await?
            || !parent
                .current(unit.conn, thread, &input.content, actor, clock)
                .await?)
    {
        return Ok(None);
    }
    let materialized = Materialized::stored(input);
    let Some((restored, callers)) =
        restore_for_input(unit, thread, &materialized, actor, authority, true, clock).await?
    else {
        return Ok(None);
    };
    InputCallers::admitted(restored, callers).map(Some)
}

/// Effect-free current-person checks keep native catalogue liveness in its existing hold phase.
/// These callers are not execution authority and are never returned as InputCallers.
pub(super) async fn for_preflight(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<Vec<Caller>>, EngineError> {
    if input.thread_id != thread.node_id {
        return Ok(None);
    }
    if let Some(parent) = authority.capability_parent()
        && (!parent.owns_input(unit.conn, input.id).await?
            || !parent
                .current(unit.conn, thread, &input.content, actor, clock)
                .await?)
    {
        return Ok(None);
    }
    let materialized = Materialized::stored(input);
    Ok(
        restore_for_input(unit, thread, &materialized, actor, authority, false, clock)
            .await?
            .map(|(_, callers)| callers),
    )
}

/// A normal derived producer supplies its actual request before the one shared input writer runs.
pub(super) async fn for_request(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    request: &InputRequest,
    selection: Option<&orch_api_contract::input_schedules::NativeInvocation>,
    actor: Id,
    authority: &InputAuthority,
    clock: &dyn Clock,
) -> Result<Option<InputCallers>, EngineError> {
    if let Some(parent) = authority.capability_parent()
        && !parent
            .current(
                unit.conn,
                thread,
                &serde_json::to_value(&request.content)?,
                actor,
                clock,
            )
            .await?
    {
        return Ok(None);
    }
    let materialized = Materialized::requested(request, selection, actor)?;
    let Some((restored, callers)) =
        restore_for_input(unit, thread, &materialized, actor, authority, true, clock).await?
    else {
        return Ok(None);
    };
    InputCallers::admitted(restored, callers).map(Some)
}
