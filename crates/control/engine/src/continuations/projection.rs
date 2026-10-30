//! Bounded prompt/version metadata shares one current-reader decision without native probes or per-row walks.

use super::{Error, admission};
use crate::{engine::Engine, error::EngineError, unit::Unit, views::People};
use orch_api_contract::threads::{
    PromptEditAvailability, PromptEditReason, ThreadContinuationProjection,
    ThreadContinuationStatus, ThreadPromptSummary, ThreadPromptVersion,
    ThreadPromptVersionListQuery, ThreadPromptVersionsPage,
};
use orch_authz::Principal;
use orch_db::queries::{thread_continuations as rows, threads};
use orch_types::{Id, Seq, Time};
use std::collections::BTreeSet;

pub fn scope(projection: &rows::Projection) -> ThreadContinuationProjection {
    ThreadContinuationProjection {
        viewed_continuation_id: projection.viewed_continuation_id,
        active_continuation_id: projection.active_continuation_id,
        execution_revision: projection.execution_revision,
        projection_revision: projection.projection_revision,
        follows_active: projection.follows_active,
    }
}

struct Context {
    viewer: Id,
    permission: bool,
    support: bool,
    sources: BTreeSet<Id>,
    changing: bool,
    archived: bool,
}

impl Context {
    async fn load(
        engine: &Engine,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        thread: &threads::ThreadRow,
        views: &[Id],
    ) -> Result<Self, Error> {
        let mut unit = Unit::new(conn, principal.now().get());
        let permission = match engine
            .check_sender(&mut unit, principal.caller(), thread, true)
            .await
        {
            Ok(()) => true,
            Err(crate::inputs::InputError::Internal(error)) => return Err(error.into()),
            Err(_) => false,
        };
        let support = match thread.agent_id {
            Some(id) => {
                match orch_db::queries::resources::thread_engine::agent(unit.conn, id).await? {
                    // An invalid persisted profile declares no boundary to continue at, as it
                    // declares no control in the thread's view: reading the thread goes on, and
                    // admitting an edit still refuses the profile.
                    Some(agent) => match crate::harnesses::descriptor_of(&agent) {
                        Ok(descriptor) => {
                            descriptor
                                .and_then(|descriptor| descriptor.capabilities.native_input_prefix)
                                == Some(
                                    orch_agent_contract::descriptor::NativeInputPrefixSupport::TurnStart,
                                )
                        }
                        Err(_) => {
                            tracing::warn!(agent = %agent.id, "an invalid profile descriptor declares no continuation boundary");
                            false
                        }
                    },
                    None => false,
                }
            }
            None => false,
        };
        let sources:Vec<Id>=sqlx::query_scalar("SELECT c.id FROM thread_continuations c
            JOIN segments s ON s.id=c.current_segment_id AND s.thread_id=c.thread_id AND s.continuation_id=c.id
            JOIN threads t ON t.node_id=c.thread_id
            JOIN environments e ON e.id=s.environment_id AND e.deleted_at IS NULL AND e.generation=s.env_generation
            JOIN machines m ON m.id=s.machine_id WHERE c.thread_id=$1 AND c.id=ANY($2) AND c.status='ready'
              AND s.agent_id=t.agent_id AND s.provider_id=t.provider_id AND s.environment_id=t.environment_id
              AND s.arrangement=t.arrangement
              AND s.native_session_ref IS NOT NULL AND s.native_session_ref<>''
              AND m.status='active' AND m.online AND 'agent'=ANY(m.roles)")
            .bind(thread.node_id).bind(views).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let changing = crate::locks::held_by(unit.conn, thread.node_id)
            .await?
            .is_some();
        Ok(Self {
            viewer: principal.caller().user(),
            permission,
            support,
            sources: sources.into_iter().collect(),
            changing,
            archived: thread.archived_at.is_some() || thread.deleted_at.is_some(),
        })
    }

    fn edit(&self, row: &rows::PromptRow, source_view: Id) -> PromptEditAvailability {
        use PromptEditReason as R;
        let reason = if row.source != "user" {
            Some((
                R::NotUserInput,
                "This input was not sent as a person's ordinary prompt.",
            ))
        } else if row.author_id != Some(self.viewer) {
            Some((
                R::NotAuthor,
                "Only the person who sent these words can edit them.",
            ))
        } else if row.state != "delivered" {
            Some((
                R::PendingInput,
                "Unsent words use their pending-input edit action.",
            ))
        } else if self.archived {
            Some((R::Archived, "The thread or a place above it is archived."))
        } else if !self.permission {
            Some((
                R::AccessRequired,
                "Editing requires current access to interact and to use this provider, model and effort.",
            ))
        } else if self.changing {
            Some((
                R::TransitionInProgress,
                "An accepted execution change holds this thread.",
            ))
        } else if !row.first_of_turn || !self.support {
            Some((
                R::NativeBoundaryUnsupported,
                "This input has no declared exact turn-start continuation boundary.",
            ))
        } else if !row.has_native_item || !self.sources.contains(&source_view) {
            Some((
                R::NativeSourceUnavailable,
                "The exact retained native input or its machine is unavailable.",
            ))
        } else {
            None
        };
        PromptEditAvailability {
            available: reason.is_none(),
            reason: reason.map(|(reason, _)| reason),
            detail: reason.map(|(_, detail)| detail.into()),
        }
    }
}

/// Actual item input IDs plus bounded changed group IDs; never delivered-input bodies.
pub async fn summaries(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    principal: &Principal,
    thread: Id,
    view: Id,
    ids: impl IntoIterator<Item = Id>,
) -> Result<Vec<ThreadPromptSummary>, Error> {
    let ids: Vec<_> = ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let thread = threads::thread(conn, thread)
        .await?
        .ok_or(Error::NotFound)?;
    let context = Context::load(engine, conn, principal, &thread, &[view]).await?;
    let prompt = rows::prompt_summaries(conn, thread.node_id, &ids).await?;
    let people = People::load(conn, prompt.iter().filter_map(|prompt| prompt.author_id)).await?;
    prompt
        .into_iter()
        .map(|row| {
            Ok(ThreadPromptSummary {
                input_id: row.input_id,
                group_id: row.group_id,
                continuation_id: row.continuation_id,
                ordinal: row.ordinal,
                total: row.total,
                author: serde_json::from_value(people.named_or_null(row.author_id))?,
                edit: context.edit(&row, view),
            })
        })
        .collect()
}

pub struct PromptMetadata {
    pub summaries: Vec<ThreadPromptSummary>,
    pub viewed_prompt: Option<ThreadPromptVersion>,
}

/// One indexed viewed-version lookup joins the same summary/authority batch as visible prompts.
/// It never loads another continuation's body or asks its native program.
pub async fn metadata(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    principal: &Principal,
    thread: Id,
    view: Id,
    ids: impl IntoIterator<Item = Id>,
) -> Result<PromptMetadata, Error> {
    let requested: BTreeSet<_> = ids.into_iter().collect();
    let viewed = rows::viewed_version(conn, thread, view).await?;
    let mut wanted = requested.clone();
    if let Some(viewed) = &viewed {
        wanted.insert(viewed.input_id);
    }
    let mut all = summaries(engine, conn, principal, thread, view, wanted).await?;
    let anchors = rows::prompt_anchors(
        conn,
        thread,
        &viewed
            .iter()
            .map(|row| (row.input_id, view))
            .collect::<Vec<_>>(),
    )
    .await?;
    let viewed_prompt = viewed
        .map(|row| {
            let summary = all
                .iter()
                .find(|summary| summary.input_id == row.input_id)
                .ok_or_else(|| {
                    EngineError::unreadable(
                        "The viewed prompt version has no retained input metadata.",
                    )
                })?;
            version(&row, summary, anchors.get(&row.input_id).copied())
        })
        .transpose()?;
    all.retain(|summary| requested.contains(&summary.input_id));
    Ok(PromptMetadata {
        summaries: all,
        viewed_prompt,
    })
}

fn version(
    row: &rows::VersionRow,
    summary: &ThreadPromptSummary,
    item_id: Option<Id>,
) -> Result<ThreadPromptVersion, Error> {
    Ok(ThreadPromptVersion {
        group_id: row.group_id,
        total: summary.total,
        id: row.input_id,
        input_id: row.input_id,
        item_id,
        continuation_id: row.continuation_id,
        ordinal: row.ordinal,
        author: summary.author.clone(),
        created_at: Time::new(row.created_at),
        status: match row.status.as_str() {
            "preparing" => ThreadContinuationStatus::Preparing,
            "ready" => ThreadContinuationStatus::Ready,
            "failed" => ThreadContinuationStatus::Failed,
            _ => {
                return Err(EngineError::unreadable(
                    "The continuation has an invalid native preparation state.",
                )
                .into());
            }
        },
        job: row
            .job_snapshot
            .clone()
            .map(serde_json::from_value)
            .transpose()?,
        edit: summary.edit.clone(),
    })
}

pub async fn versions(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    principal: &Principal,
    thread: Id,
    input: Id,
    query: &ThreadPromptVersionListQuery,
) -> Result<ThreadPromptVersionsPage, Error> {
    if query.before.is_some() && query.after.is_some()
        || query.before.is_some_and(|n| n < 1)
        || query.after.is_some_and(|n| n < 1)
        || query.limit == Some(0)
    {
        return Err(Error::Invalid(
            "Choose one positive version cursor and a nonzero page size.",
        ));
    }
    admission::require_view(conn, principal, thread).await?;
    let thread = threads::thread(conn, thread)
        .await?
        .ok_or(Error::NotFound)?;
    let view = rows::projection(conn, thread.node_id, query.continuation_id)
        .await?
        .ok_or(Error::NotFound)?;
    if !view.accepts_cursor(
        query.before.is_some() || query.after.is_some(),
        query.projection_revision,
    ) {
        return Err(Error::Conflict(
            "The viewed continuation changed; reload its prompt versions before using this cursor.",
        ));
    }
    if !rows::reading::contains_input(conn, thread.node_id, view.viewed_continuation_id, input)
        .await?
    {
        return Err(Error::NotFound);
    }
    let selected = rows::prompt_summaries(conn, thread.node_id, &[input])
        .await?
        .pop()
        .ok_or(Error::NotFound)?;
    let limit = i64::from(
        query
            .limit
            .unwrap_or(orch_api_contract::threads::ITEMS_DEFAULT_LIMIT)
            .min(orch_api_contract::threads::ITEMS_MAX_LIMIT),
    );
    let forward = query.after.is_some();
    let mut page = rows::versions(
        conn,
        thread.node_id,
        selected.group_id,
        query.after.or(query.before),
        forward,
        limit + 1,
    )
    .await?;
    if selected.total == 1
        && page.is_empty()
        && query.before.is_none_or(|n| n > 1)
        && query.after.is_none_or(|n| n < 1)
    {
        page.push(rows::VersionRow {
            input_id: selected.input_id,
            group_id: selected.group_id,
            continuation_id: selected.continuation_id,
            ordinal: 1,
            author_id: selected.author_id,
            created_at: selected.created_at,
            status: "ready".into(),
            job_id: None,
            job_snapshot: None,
        });
    }
    let extra = i64::try_from(page.len()).unwrap_or(i64::MAX) > limit;
    if extra {
        if forward {
            page.pop();
        } else {
            page.remove(0);
        }
    }
    let sources: Vec<_> = page
        .iter()
        .map(|row| row.continuation_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let context = Context::load(engine, conn, principal, &thread, &sources).await?;
    let metadata = rows::prompt_summaries(
        conn,
        thread.node_id,
        &page.iter().map(|row| row.input_id).collect::<Vec<_>>(),
    )
    .await?;
    let anchors = rows::prompt_anchors(
        conn,
        thread.node_id,
        &page
            .iter()
            .map(|row| (row.input_id, row.continuation_id))
            .collect::<Vec<_>>(),
    )
    .await?;
    let people = People::load(conn, page.iter().filter_map(|row| row.author_id)).await?;
    let mut result = Vec::with_capacity(page.len());
    for row in page {
        let meta = metadata
            .iter()
            .find(|meta| meta.input_id == row.input_id)
            .ok_or_else(|| {
                EngineError::unreadable("A retained prompt version has no actual input.")
            })?;
        result.push(ThreadPromptVersion {
            group_id: row.group_id,
            total: meta.total,
            id: row.input_id,
            input_id: row.input_id,
            continuation_id: row.continuation_id,
            ordinal: row.ordinal,
            item_id: anchors.get(&row.input_id).copied(),
            author: serde_json::from_value(people.named_or_null(row.author_id))?,
            created_at: Time::new(row.created_at),
            status: match row.status.as_str() {
                "preparing" => ThreadContinuationStatus::Preparing,
                "ready" => ThreadContinuationStatus::Ready,
                "failed" => ThreadContinuationStatus::Failed,
                _ => {
                    return Err(EngineError::unreadable(
                        "The continuation has an invalid native preparation state.",
                    )
                    .into());
                }
            },
            job: row.job_snapshot.map(serde_json::from_value).transpose()?,
            edit: context.edit(meta, row.continuation_id),
        });
    }
    let next_before = result
        .first()
        .filter(|row| row.ordinal > 1)
        .map(|row| row.ordinal);
    let next_after = result
        .last()
        .filter(|row| row.ordinal < selected.total)
        .map(|row| row.ordinal);
    Ok(ThreadPromptVersionsPage {
        projection: Some(scope(&view)),
        group_id: selected.group_id,
        rows: result,
        total: selected.total,
        selected_version: selected.ordinal,
        active_continuation_id: thread.active_continuation_id,
        next_before,
        next_after,
        seq: thread.seq(),
    })
}

/// The existing user-message identity is the only timeline-to-prompt join.
pub fn item_inputs(items: &[orch_api_contract::threads::Item]) -> Vec<Id> {
    items
        .iter()
        .filter_map(|item| match &item.payload {
            orch_api_contract::threads::ItemPayload::UserMessage(message) => message.input_id,
            _ => None,
        })
        .collect()
}

pub async fn changed_ids(
    conn: &mut orch_db::Conn,
    thread: Id,
    view: Id,
    since: Option<Seq>,
    limit: i64,
) -> Result<Vec<Id>, Error> {
    match since {
        Some(since) => {
            Ok(rows::reading::changed_prompt_inputs(conn, thread, view, since, limit).await?)
        }
        None => Ok(Vec::new()),
    }
}
