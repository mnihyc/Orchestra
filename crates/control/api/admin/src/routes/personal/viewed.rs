//! A successful authorized page read carries its visit; drawing a page sends no extra mutation.

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_api_contract::personal::PersonalTarget;

use orch_db::queries::hierarchy;

use orch_db::queries::identity;

use orch_types::Id;

use orch_types::Json;
fn id(value: &Json) -> Option<Id> {
    value.as_str()?.parse().ok()
}

/// The requested page must actually be present in this authorized read. A URL alone grants nothing.
fn selected<'a>(route: &str, value: &'a Json) -> Option<(&'a Json, PersonalTarget)> {
    let path = route.split('?').next()?;
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    let (kind, target): (&str, Option<Id>) = match parts.as_slice() {
        ["org", _, ..] => ("org", None),
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
        .or_else(|| match kind {
            "org" => value
                .get("orgs")
                .and_then(Json::as_array)
                .and_then(|rows| {
                    rows.iter().find(|row| {
                        row.get("address").and_then(Json::as_str) == parts.get(1).copied()
                    })
                })
                .or_else(|| value.get("org"))
                .or_else(|| {
                    if value.get("mode").and_then(Json::as_str) != Some("overview") {
                        return None;
                    }
                    value.get("data")?.get("org")
                }),
            "project" | "topic" => value.get("node"),
            _ => None,
        })
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

fn opened_place(row: &Json, target: &PersonalTarget) -> Option<(Id, Option<Id>)> {
    let node = match target.kind.as_str() {
        "org" | "project" | "topic" | "thread" => Some(target.id),
        "document" => row.get("node").and_then(|node| node.get("id")).and_then(id),
        _ => None,
    }?;
    let org = if target.kind.as_str() == "org" {
        Some(target.id)
    } else {
        row.get("org_id").and_then(id)
    };
    Some((node, org))
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
    if let Some((node, org)) = opened_place(row, &target) {
        identity::note_opened_place(&mut conn, user.id(), node, route, org, ctx.now).await?;
    }
    Ok(())
}
orch_api_base::__api_bind_d8907363a0!();

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_only_the_served_complete_page_can_be_recorded() {
        self::only_the_served_complete_page_can_be_recorded()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn overview_visits_require_the_served_complete_place() {
        let expected = Id::from_u128(1);
        let other = Id::from_u128(2);
        let org = Id::from_u128(3);
        for (kind, wrapper, tagged) in [
            ("project", "node", false),
            ("topic", "node", false),
            ("org", "org", false),
            ("org", "org", true),
            ("org", "orgs", false),
        ] {
            let route = if kind == "org" {
                "/org/acme".to_owned()
            } else {
                format!("/{kind}/{expected}")
            };
            let mut row = serde_json::json!({
                "id": expected, "kind": kind, "org_id": org, "access": {}
            });
            if kind == "org" {
                row["address"] = serde_json::json!("acme");
            }
            let wrap = |row: Json| {
                let overview = if wrapper == "orgs" {
                    serde_json::json!({"orgs":[row]})
                } else {
                    serde_json::json!({(wrapper): row})
                };
                if tagged {
                    serde_json::json!({"mode":"overview","data":overview})
                } else {
                    overview
                }
            };
            let value = wrap(row.clone());
            let Some((served, target)) = selected(&route, &value) else {
                panic!("the served {kind} overview must be selected; tagged={tagged}");
            };
            assert_eq!(target.id, expected);
            assert_eq!(target.kind.as_str(), kind);
            assert_eq!(served["org_id"], serde_json::json!(org));
            let expected_org = if kind == "org" { expected } else { org };
            assert_eq!(
                opened_place(served, &target),
                Some((expected, Some(expected_org)))
            );

            for (field, invalid) in [
                ("id", serde_json::json!("invalid-id")),
                ("kind", serde_json::json!("thread")),
                ("access", Json::Null),
                ("content_hidden", serde_json::json!(true)),
            ] {
                let mut refused = row.clone();
                refused[field] = invalid;
                assert!(
                    selected(&route, &wrap(refused)).is_none(),
                    "a {kind} overview with invalid {field} is refused"
                );
            }
            assert!(selected(&format!("https://elsewhere.test{route}"), &value).is_none());
            let mut refused = row.clone();
            if kind == "org" {
                refused["address"] = serde_json::json!("another-org");
                assert!(selected(&route, &wrap(refused)).is_none());
                assert!(selected(&route, &serde_json::json!({"node":row})).is_none());
            } else {
                refused["id"] = serde_json::json!(other);
                assert!(selected(&route, &wrap(refused)).is_none());
                assert!(selected(&route, &serde_json::json!({"org":row})).is_none());
            }
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_overview_visits_require_the_served_complete_place() {
        self::overview_visits_require_the_served_complete_place()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_documents_own_node_is_kept_inside_the_selected_document() {
        let expected = Id::from_u128(1);
        let node = Id::from_u128(2);
        let org = Id::from_u128(3);
        let row = serde_json::json!({
            "id":expected,"org_id":org,"access":{},
            "node":{"id":node,"kind":"topic","access":{}}
        });
        for value in [row.clone(), serde_json::json!({"document":row})] {
            let Some((served, target)) = selected(&format!("/doc/{expected}"), &value) else {
                panic!("the document's containing node cannot replace its selected row");
            };
            assert_eq!(target.id, expected);
            assert_eq!(target.kind.as_str(), "document");
            assert_eq!(served["node"]["id"], serde_json::json!(node));
            assert_eq!(opened_place(served, &target), Some((node, Some(org))));
        }
        for (route, kind) in [
            (format!("/thread/{expected}"), "thread"),
            (format!("/resources/providers/{expected}"), "api"),
        ] {
            let row = serde_json::json!({
                "id":expected,"kind":kind,"access":{},
                "node":{"id":node,"kind":"topic","access":{}}
            });
            assert!(selected(&route, &row).is_some());
            assert!(selected(&route, &serde_json::json!({"node":row})).is_none());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_documents_own_node_is_kept_inside_the_selected_document() {
        self::a_documents_own_node_is_kept_inside_the_selected_document()
    }
}
