//! Explicit accepted thread links, with current authority and one history marker.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::source::{ThreadLink, ThreadLinksSet, ThreadLinksSetBody};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::{source, threads};
use orch_engine::unit::Unit;
use serde_json::json;

/// Binds the accepted-link replacement on the existing thread page.
pub fn mount(registry: &mut Registry) {
    registry.on(ThreadLinksSet, links_set);
}

fn clean_link(mut link: ThreadLink) -> ApiResult<ThreadLink> {
    let url = url::Url::parse(&link.url)
        .map_err(|_| ApiError::invalid("links.url", "a link is an HTTP or HTTPS address"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ApiError::invalid(
            "links.url",
            "a link is an HTTP or HTTPS address without user information",
        ));
    }
    link.label = link.label.trim().to_owned();
    if link.label.is_empty() {
        return Err(ApiError::invalid("links.label", "a link has a label"));
    }
    link.url = url.to_string();
    Ok(link)
}

async fn links_set(ctx: Ctx, (): (), body: ThreadLinksSetBody) -> ApiResult<Vec<ThreadLink>> {
    let links = body
        .links
        .into_iter()
        .map(clean_link)
        .collect::<ApiResult<Vec<_>>>()?;
    let thread = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let now = guard.now();
    let row = threads::lock(&mut tx, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let value = serde_json::to_value(&links).map_err(ApiError::internal)?;
    if row.links == value {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(links);
    }
    let mut unit = Unit::new(&mut tx, now.get());
    let seq = unit.stamp(thread).await.map_err(ApiError::internal)?;
    source::set_links(unit.conn, thread, &value, seq).await?;
    let by = orch_engine::views::user_ref_json(unit.conn, guard.user().id())
        .await
        .map_err(ApiError::internal)?;
    let count = i64::try_from(links.len()).map_err(ApiError::internal)?;
    if let Some(segment) = row.current_segment_id {
        orch_engine::ingest::add_marker(
            &mut unit,
            thread,
            segment,
            None,
            &json!({ "marker": "links_changed", "data": { "by": by, "count": count } }),
            &format!("links:{seq}"),
        )
        .await
        .map_err(ApiError::internal)?;
    }
    let mut entry = Entry::new(
        Actor::User(guard.user().id()),
        orch_db::audit::action!(AuditAction::ThreadLinksChanged).as_str(),
        "thread",
        Some(thread),
        "Changed the thread's links",
    );
    entry.history = true;
    entry.node_id = Some(thread);
    entry.org_id = Some(row.org_id);
    entry.details = json!({ "count": count });
    audit::append(unit.conn, &entry, &crate::sign_in::origin(&ctx), now.get()).await?;
    unit.finish().await.map_err(ApiError::internal)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(links)
}
