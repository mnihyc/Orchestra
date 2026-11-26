//! Accepted session decisions and current safe names; never a skill archive or manifest.

use super::read_operation;
use crate::middleware::authorization::{refusals, restoration_error};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{DEFAULT_LIMIT, Page};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::settings::{SettingScope, SettingSource};
use orch_api_contract::skills::{
    SessionSkillDecision, SessionSkillEntry, SessionSkillRef, SessionSkillSource,
    SessionSkillVersion, SessionSkillsSummary, SkillSkip, ThreadSessionSkills,
    ThreadSessionSkillsGet, ThreadSessionSkillsQuery, ThreadSkillFrom,
};
use orch_authz::Policy;
use orch_db::{
    Conn,
    queries::{resources::skills, session_skills, thread_continuations, thread_history, threads},
};
use orch_types::{Id, Time};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_operation(ThreadSessionSkillsGet, read_operation::prepare, get);
}

#[derive(Deserialize)]
struct Entry {
    skill_id: Id,
    contributor: Id,
    #[serde(rename = "from")]
    from: ThreadSkillFrom,
    version_id: Option<Id>,
    number: Option<i64>,
    reason: Option<SkillSkip>,
}

async fn project(
    ctx: &Ctx,
    conn: &mut Conn,
    raw: Vec<(i64, bool, orch_types::Json)>,
) -> ApiResult<Vec<SessionSkillEntry>> {
    let entries = raw
        .into_iter()
        .map(|(_, selected, value)| {
            serde_json::from_value::<Entry>(value)
                .map(|entry| (selected, entry))
                .map_err(ApiError::internal)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let principal = ctx.operation_principal(conn).await?;
    let ids = entries
        .iter()
        .map(|(_, e)| e.skill_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let holdings =
        orch_authz::resources::effective_among(conn, &principal, &ids, Policy::default())
            .await
            .map_err(restoration_error)?;
    let readable = holdings
        .iter()
        .filter(|held| {
            held.held
                .iter()
                .any(|held| held.permission == Permission::SkillView)
        })
        .map(|held| held.object.id)
        .collect::<BTreeSet<_>>();
    let names = skills::any_skills(conn, &readable.iter().copied().collect::<Vec<_>>())
        .await?
        .into_iter()
        .filter(|row| row.deleted_at.is_none())
        .map(|row| (row.id, row.name))
        .collect::<HashMap<_, _>>();
    let people = entries
        .iter()
        .map(|(_, e)| e.contributor)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let people = refusals::user_refs(conn, &people).await?;
    let places = entries
        .iter()
        .filter_map(|(_, e)| match e.from {
            ThreadSkillFrom::Place { node_id } => Some(node_id),
            ThreadSkillFrom::Personal { .. } => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let visible = orch_authz::nodes::effective_many(conn, &principal, &places, Policy::default())
        .await
        .map_err(restoration_error)?
        .into_iter()
        .filter(|held| !held.held.is_empty())
        .map(|held| {
            refusals::node_kind(held.kind.as_str())
                .map(|kind| (held.node, kind))
                .ok_or_else(|| ApiError::internal("the source has an unknown node kind"))
        })
        .collect::<ApiResult<HashMap<_, _>>>()?;
    let place_names = sqlx::query_as::<_, (Id, String)>(
        "SELECT id,name FROM nodes WHERE id=ANY($1) AND deleted_at IS NULL",
    )
    .bind(visible.keys().copied().collect::<Vec<_>>())
    .fetch_all(conn)
    .await
    .map_err(orch_db::DbError::from)?
    .into_iter()
    .collect::<HashMap<_, _>>();
    entries
        .into_iter()
        .map(|(selected, e)| {
            let skill = names.get(&e.skill_id).map(|name| SessionSkillRef {
                id: e.skill_id,
                name: name.clone(),
            });
            let version = skill.as_ref().and_then(|_| {
                e.version_id
                    .zip(e.number)
                    .map(|(id, number)| SessionSkillVersion { id, number })
            });
            let source = match e.from {
                ThreadSkillFrom::Personal { .. } => SessionSkillSource::Personal,
                ThreadSkillFrom::Place { node_id } => SessionSkillSource::Place {
                    setting: visible.get(&node_id).zip(place_names.get(&node_id)).map(
                        |(kind, name)| SettingSource {
                            scope: match kind {
                                NodeKind::Org => SettingScope::Org,
                                NodeKind::Project => SettingScope::Project,
                                NodeKind::Topic => SettingScope::Topic,
                                NodeKind::Thread => SettingScope::Thread,
                            },
                            node: Some(NodeRef {
                                thread_display: None,
                                id: node_id,
                                kind: *kind,
                                name: Some(name.clone()),
                                redacted: false,
                            }),
                        },
                    ),
                },
            };
            let decision = if selected {
                SessionSkillDecision::Selected { version }
            } else {
                SessionSkillDecision::Skipped {
                    reason: e.reason.ok_or_else(|| {
                        ApiError::internal("the captured skill omission has no reason")
                    })?,
                }
            };
            Ok(SessionSkillEntry {
                skill,
                contributor: people.get(&e.contributor).cloned(),
                source,
                decision,
            })
        })
        .collect()
}

async fn get(ctx: Ctx, query: ThreadSessionSkillsQuery, (): ()) -> ApiResult<ThreadSessionSkills> {
    let thread = ctx.id("id")?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be positive"));
    }
    let after = match &query.cursor {
        None => 0,
        Some(cursor) => {
            let (revision, position) = cursor
                .split_once(':')
                .ok_or_else(|| ApiError::invalid("cursor", "is not a session skill position"))?;
            if revision != query.session_revision.to_string() {
                return Err(ApiError::invalid("cursor", "belongs to another session"));
            }
            position
                .parse::<i64>()
                .ok()
                .filter(|position| *position >= 0)
                .ok_or_else(|| ApiError::invalid("cursor", "is not a session skill position"))?
        }
    };
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    read_operation::require_current(&ctx, &mut conn).await?;
    let projection = thread_continuations::projection(&mut conn, thread, query.continuation_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(query.cursor.is_some(), query.projection_revision) {
        return Err(super::read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    if !thread_history::complete_segment(
        &mut conn,
        thread,
        projection.viewed_continuation_id,
        query.segment_id,
    )
    .await?
    {
        return Err(ApiError::not_found());
    }
    let outcome = session_skills::by_revision(&mut conn, query.session_revision)
        .await?
        .filter(|row| row.thread_id == thread && row.segment_id == query.segment_id)
        .ok_or_else(ApiError::not_found)?;
    let applied_at = outcome.applied_at.ok_or_else(ApiError::not_found)?;
    let mut raw = session_skills::page(
        &mut conn,
        query.session_revision,
        after,
        i64::from(limit) + 1,
    )
    .await?;
    let more = raw.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    if more {
        raw.pop();
    }
    let next_cursor = if more {
        raw.last()
            .map(|(position, _, _)| format!("{}:{position}", query.session_revision))
    } else {
        None
    };
    let current = threads::segment(&mut conn, query.segment_id).await?;
    let is_current = current
        .as_ref()
        .and_then(|row| row.session_skills.as_ref())
        .and_then(|value| value.get("session_revision"))
        .and_then(|value| value.as_str())
        .is_some_and(|revision| revision == query.session_revision.to_string());
    let active_segment: Option<Id> =
        sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    let next_session_changed = if after == 0
        && is_current
        && active_segment == Some(query.segment_id)
        && projection.viewed_continuation_id == projection.active_continuation_id
    {
        let viewer = ctx.operation_principal(&mut conn).await?.user();
        let parts =
            orch_engine::views::thread_parts(&mut conn, thread, Some(viewer), Time::new(ctx.now))
                .await
                .map_err(ApiError::internal)?;
        if let Some(descriptor) = parts
            .and_then(|parts| parts.binding)
            .and_then(|binding| binding.descriptor)
        {
            let mut requested = orch_engine::skills::resolve::metadata_for_thread(
                &mut conn,
                thread,
                Time::new(ctx.now),
            )
            .await
            .map_err(ApiError::internal)?;
            if descriptor.capabilities.skills
                == orch_agent_contract::descriptor::SkillsSupport::None
            {
                requested.skills.clear();
            }
            Some(orch_engine::skills::fingerprint(&requested).to_string() != outcome.fingerprint)
        } else {
            None
        }
    } else {
        None
    };
    let captured = orch_engine::continuations::projection::scope(&projection);
    conn.commit().await.map_err(orch_db::DbError::from)?;
    let entries = project(&ctx, &mut *ctx.app.db.acquire().await?, raw).await?;
    read_operation::before_return(&ctx).await?;
    Ok(ThreadSessionSkills {
        projection: captured,
        summary: SessionSkillsSummary {
            session_revision: query.session_revision,
            segment_id: query.segment_id,
            applied_at: Time::new(applied_at),
            supported: outcome.supported,
            selected: outcome.selected_count,
            skipped: outcome.skipped_count,
        },
        entries: Page {
            items: entries,
            next_cursor,
        },
        next_session_changed,
        observed_at: Time::new(ctx.now),
    })
}
