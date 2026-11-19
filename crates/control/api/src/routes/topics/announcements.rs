//! What a person tells the threads of a topic, or of every open topic of a project, and what has been told: the four requests that list and make announcements for people.
//!
//! Every place uses one delivery and reader policy. Each row is one topic's delivery;
//! its group identifier joins rows made by one announcement without hiding their scope.

use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::topics::{
    Announcement, AnnouncementCreateBody, AnnouncementTo, NodeAnnouncementCreate,
    NodeAnnouncementList, NodeAnnouncementListQuery, PlaceAnnouncementCreateReply,
};
use orch_authz::{Act, Policy, nodes};
use orch_db::queries::threads::tools as rows;
use orch_engine::tools::announce::{AnnounceError, PersonAnnouncement};
use orch_types::Id;
use std::collections::HashSet;

/// Binds the one place-level announcement path.
pub fn mount_announcements(registry: &mut Registry) {
    registry
        .on(NodeAnnouncementList, list)
        .on(NodeAnnouncementCreate, create);
}

async fn problem(ctx: &Ctx, error: AnnounceError) -> ApiError {
    match error {
        AnnounceError::NotFound => ApiError::not_found(),
        AnnounceError::Invalid(message) => ApiError::invalid("", &message),
        AnnounceError::Denied(refusal) => match ctx.app.db.acquire().await {
            Ok(mut conn) => match refusals::denied(&mut conn, *refusal).await {
                Ok(problem) | Err(problem) => problem,
            },
            Err(error) => error.into(),
        },
        AnnounceError::Internal(orch_engine::error::EngineError::Authz(error)) => {
            match ctx.app.db.acquire().await {
                Ok(mut conn) => refusals::problem(&mut conn, error).await,
                Err(error) => error.into(),
            }
        }
        AnnounceError::Internal(error) => ApiError::engine(error),
    }
}

fn request_of(body: AnnouncementCreateBody) -> PersonAnnouncement {
    PersonAnnouncement {
        title: body.title,
        body: body.body,
        category: body.category,
        severity: body.severity,
        targets: body.targets,
        delivery: body.delivery,
        fallback: body.fallback,
        wake_idle: body.wake_idle,
    }
}

/// Time and identifier keep announcements made at the same instant on separate pages.
fn cursor(row: &rows::AnnouncementRow) -> String {
    format!("{}:{}", row.created_at.timestamp_micros(), row.id)
}

async fn list(ctx: Ctx, query: NodeAnnouncementListQuery, (): ()) -> ApiResult<Page<Announcement>> {
    let place = ctx.id("id")?;
    let limit = i64::from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    );
    let mut before = query
        .cursor
        .as_deref()
        .map(|value| {
            let (time, id) = value
                .split_once(':')
                .ok_or_else(|| ApiError::invalid("cursor", "is not a cursor of this list"))?;
            let time = time
                .parse::<i64>()
                .ok()
                .and_then(chrono::DateTime::from_timestamp_micros)
                .ok_or_else(|| ApiError::invalid("cursor", "is not a cursor of this list"))?;
            let id = id
                .parse::<Id>()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))?;
            Ok::<_, ApiError>((time, id))
        })
        .transpose()?;
    let everything = query.to == Some(AnnouncementTo::All);
    let category = query.category.map(|category| match category {
        orch_api_contract::threads::AnnouncementCategory::Notice => "notice",
        orch_api_contract::threads::AnnouncementCategory::Milestone => "milestone",
    });
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if !matches!(
        rows::node_kind(&mut conn, place).await?.as_deref(),
        Some("org" | "project" | "topic")
    ) {
        return Err(ApiError::not_found());
    }
    let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
    let mut kept = Vec::new();
    loop {
        let found =
            rows::announcements_of_place(&mut conn, place, category, everything, before, limit + 1)
                .await?;
        let read = found.len();
        let Some(last) = found.last().map(|row| (row.created_at, row.id)) else {
            break;
        };
        before = Some(last);
        let topics: Vec<Id> = found
            .iter()
            .map(|row| row.topic_id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let viewable: HashSet<Id> = nodes::holding(
            &mut conn,
            &principal,
            &topics,
            exact(Permission::TopicView),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|node| node.id)
        .collect();
        let found = found
            .into_iter()
            .filter(|row| viewable.contains(&row.topic_id))
            .collect();
        kept.extend(if everything {
            seen_by(&mut conn, &principal, found).await?
        } else {
            found
        });
        if kept.len() > wanted || read <= wanted {
            break;
        }
    }
    let more = kept.len() > wanted;
    kept.truncate(wanted);
    let next_cursor = if more { kept.last().map(cursor) } else { None };
    let items = ctx
        .app
        .engine
        .announcement_views(&kept)
        .await
        .map_err(ApiError::internal)?;
    Ok(Page { items, next_cursor })
}

/// Private messages and follower-only milestones retain the existing thread-view check.
async fn seen_by(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    found: Vec<rows::AnnouncementRow>,
) -> ApiResult<Vec<rows::AnnouncementRow>> {
    let threads: Vec<Id> = found
        .iter()
        .flat_map(|row| [row.from_thread_id, row.to_thread_id].into_iter().flatten())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let viewable: HashSet<Id> = if threads.is_empty() {
        HashSet::new()
    } else {
        nodes::holding(
            conn,
            principal,
            &threads,
            exact(Permission::ThreadView),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|node| node.id)
        .collect()
    };
    Ok(found
        .into_iter()
        .filter(|row| {
            matches!(row.targets.as_str(), "all" | "active")
                || row.from_user_id.is_some()
                || [row.from_thread_id, row.to_thread_id]
                    .into_iter()
                    .flatten()
                    .any(|thread| viewable.contains(&thread))
        })
        .collect())
}

async fn create(
    ctx: Ctx,
    (): (),
    body: AnnouncementCreateBody,
) -> ApiResult<PlaceAnnouncementCreateReply> {
    let place = ctx.id("id")?;
    let (tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let sent = match ctx
        .app
        .engine
        .announce_to_place_guarded(tx, guard.into_principal(), place, &request_of(body))
        .await
    {
        Ok(sent) => sent,
        Err(error) => return Err(problem(&ctx, error).await),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let mut announcements = Vec::with_capacity(sent.sent.len());
    for each in &sent.sent {
        announcements.push(
            orch_engine::tools::announce::announcement_view(
                &mut conn,
                &each.row,
                Some(each.delivered),
            )
            .await
            .map_err(ApiError::internal)?,
        );
    }
    Ok(PlaceAnnouncementCreateReply {
        group_id: sent.group_id,
        announcements,
        topics_skipped: sent.skipped,
    })
}
