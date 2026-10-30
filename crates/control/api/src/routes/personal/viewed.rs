//! A successful authorized page read carries its visit; drawing a page sends no extra mutation.
use crate::middleware::requests::ApiResult;
use crate::registry::Ctx;
use orch_api_contract::personal::PersonalTarget;
use orch_db::queries::{hierarchy, identity};
use orch_types::{Id, Json};

fn id(value: &Json) -> Option<Id> {
    value.as_str()?.parse().ok()
}

/// The requested page must actually be present in this authorized read. A URL alone grants nothing.
fn selected<'a>(route: &str, value: &'a Json) -> Option<(&'a Json, PersonalTarget)> {
    let path = route.split('?').next()?;
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    let (kind, target): (&str, Option<Id>) = match parts.as_slice() {
        ["org", slug, ..] => {
            if let Some(row) = value.get("orgs").and_then(Json::as_array).and_then(|rows| {
                rows.iter()
                    .find(|row| row.get("address").and_then(Json::as_str) == Some(*slug))
            }) && row.get("access").is_some_and(Json::is_object)
            {
                return Some((
                    row,
                    PersonalTarget {
                        kind: serde_json::from_value(serde_json::json!("org")).ok()?,
                        id: id(row.get("id")?)?,
                    },
                ));
            }
            ("org", None)
        }
        [kind @ ("project" | "topic" | "thread"), target, ..] => {
            (*kind, Some(target.parse().ok()?))
        }
        ["doc", target, ..] => ("document", Some(target.parse().ok()?)),
        ["resources", kind, target, ..] => (
            match *kind {
                "providers" => "provider",
                "agents" => "agent",
                "environments" => "environment",
                "machines" => "machine",
                "hosts" => "host",
                "templates" => "template",
                "mcp" => "mcp_server",
                "mounts" => "mount",
                "skills" => "skill",
                _ => return None,
            },
            Some(target.parse().ok()?),
        ),
        _ => return None,
    };
    let row = value
        .get("thread")
        .or_else(|| value.get("document"))
        .or_else(|| value.get("sync").and_then(|sync| sync.get("thread")))
        .unwrap_or(value);
    if row.get("content_hidden").and_then(Json::as_bool) == Some(true) {
        return None;
    }
    let row_id = id(row.get("id")?)?;
    if target.is_some_and(|target| target != row_id) {
        return None;
    }
    if matches!(kind, "org" | "project" | "topic" | "thread")
        && row.get("kind").and_then(Json::as_str) != Some(kind)
    {
        return None;
    }
    if kind == "org" && row.get("address").and_then(Json::as_str) != parts.get(1).copied() {
        return None;
    }
    if !row.get("access").is_some_and(Json::is_object) {
        return None;
    }
    Some((
        row,
        PersonalTarget {
            kind: serde_json::from_value(serde_json::json!(kind)).ok()?,
            id: row_id,
        },
    ))
}

pub(crate) async fn record(ctx: &Ctx, value: &Json) -> ApiResult<()> {
    if ctx.route.method.as_str() != "GET" || ctx.inspect_deleted {
        return Ok(());
    }
    // Location bookkeeping needs only an ordinary route and tab/filter query, bounded to 2 KiB
    // so an arbitrary request header cannot expand the saved sidebar without limit.
    let Some(route) = ctx
        .headers
        .get("x-orch-view")
        .and_then(|value| value.to_str().ok())
        .filter(|route| route.len() <= 2048)
    else {
        return Ok(());
    };
    let Some((row, target)) = selected(route, value) else {
        return Ok(());
    };
    let Ok(user) = ctx.user() else {
        return Ok(());
    };
    let mut conn = ctx.app.db.acquire().await?;
    hierarchy::touch_item_recent(
        &mut conn,
        user.id(),
        target.kind.as_str(),
        target.id,
        ctx.now,
        i64::from(ctx.app.config.limits.recents_per_user),
    )
    .await?;
    let node = match target.kind.as_str() {
        "org" | "project" | "topic" | "thread" => Some(target.id),
        "document" => row.get("node").and_then(|node| node.get("id")).and_then(id),
        _ => None,
    };
    if let Some(node) = node {
        let org = if target.kind.as_str() == "org" {
            Some(target.id)
        } else {
            row.get("org_id").and_then(id)
        };
        identity::note_opened_place(&mut conn, user.id(), node, route, org, ctx.now).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_served_complete_page_can_be_recorded() {
        let target = "00000000-0000-4000-8000-000000000001";
        let row = serde_json::json!({"id":target,"kind":"thread","access":{"permissions":["thread.view"]}});
        assert!(
            selected(
                &format!("/thread/{target}"),
                &serde_json::json!({"thread":row})
            )
            .is_some()
        );
        assert!(
            selected(
                &format!("/thread/{target}"),
                &serde_json::json!({"sync":{"thread":row}})
            )
            .is_some()
        );
        assert!(selected("/thread/00000000-0000-4000-8000-000000000002", &row).is_none());
        assert!(
            selected(
                &format!("/thread/{target}"),
                &serde_json::json!({"id":target,"kind":"thread"})
            )
            .is_none()
        );
        assert!(selected("https://elsewhere.test/thread/1", &row).is_none());
    }
}
