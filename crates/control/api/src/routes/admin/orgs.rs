//! Every org of the system for the people who run it, and the two limits of a root org: whether its tree takes further members and whether it holds orgs inside it.
//!
//! A system admin is the Admin of every org, so the orgs are listed and shown as the engine shows
//! them to somebody who holds that. Setting a limit removes nobody and no org: it decides only what
//! may be done from then on. Only a root org has them (an org inside another is held by the limits
//! of its root), and it is the system, never the org's own owners, that sets them: a limit that its
//! subject could lift would be none.

use super::{SECONDS_PER_DAY, admin_of, invalid, page_size, read_ahead};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{Placed, record, view};
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::admin::{AdminOrgList, AdminOrgListQuery, AdminOrgUpdate, OrgLimitsChange};
use orch_api_contract::common::{Page, SubscriptionLevel};
use orch_api_contract::documents::DeletedFilter;
use orch_api_contract::nodes::{
    Access, FieldSource, Node, NodeBase, NodeKind, NodeRef, Org, OrgKind, OrgType, PageState,
};
use orch_authz::Policy;
use orch_db::queries::hierarchy::{self, DeletedRow};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use serde_json::json;
use std::collections::HashMap;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(AdminOrgList, list).on(AdminOrgUpdate, update);
}

/// What a cursor of the list of live orgs holds: the org the page before ended with, by its
/// identifier and its name, which is where the order of the list is.
fn cursor_of_org(personal: bool, id: Id, name: &str) -> String {
    format!("{}|{id}|{name}", if personal { "p" } else { "t" })
}

fn parse_org_cursor(text: &str) -> ApiResult<(bool, Id, String)> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (kind, rest) = text.split_once('|').ok_or_else(bad)?;
    let personal = match kind {
        "p" => true,
        "t" => false,
        _ => return Err(bad()),
    };
    let (id, name) = rest.split_once('|').ok_or_else(bad)?;
    Ok((personal, id.parse().map_err(|_| bad())?, name.to_owned()))
}

/// What a cursor of the list of deleted orgs holds: the moment and the identifier of the last.
fn cursor_of_deleted(row: &DeletedRow) -> String {
    format!("{}|{}", row.deleted_at.timestamp_micros(), row.id)
}

fn parse_deleted_cursor(text: &str) -> ApiResult<(DateTime<Utc>, Id)> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    Ok((
        micros
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?,
        id.parse().map_err(|_| bad())?,
    ))
}

/// Every org of the system, by name; with `deleted=only`, the root orgs that were deleted and
/// are not purged yet, as a person who deleted one sees theirs, the latest first.
async fn list(ctx: Ctx, query: AdminOrgListQuery, (): ()) -> ApiResult<Page<Org>> {
    admin_of(&ctx)?;
    let size = page_size(query.limit);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut conn = ctx.app.db.acquire().await?;
    if query.deleted == Some(DeletedFilter::Only) {
        return deleted(&ctx, &mut conn, q, query.cursor.as_deref(), size).await;
    }
    let after = query.cursor.as_deref().map(parse_org_cursor).transpose()?;
    let org_kind = query.org_kind.map(|kind| match kind {
        OrgType::Personal => "personal",
        OrgType::Team => "team",
    });
    let mut rows = hierarchy::orgs_by_name(
        &mut conn,
        q,
        org_kind,
        after
            .as_ref()
            .map(|(personal, id, name)| (*personal, name.as_str(), *id)),
        read_ahead(size),
    )
    .await?;
    let more = rows.len() > size;
    rows.truncate(size);
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let ids: Vec<Id> = rows.iter().map(|(id, _, _)| *id).collect();
    let nodes = view::visible(&mut conn, &principal, &ids, ctx.now, Policy::default()).await?;
    Ok(Page {
        next_cursor: more
            .then(|| {
                rows.last()
                    .map(|(id, name, personal)| cursor_of_org(*personal, *id, name))
            })
            .flatten(),
        items: nodes
            .into_iter()
            .filter_map(|node| match node {
                Node::Org(org) => Some(org),
                _ => None,
            })
            .collect(),
    })
}

/// How long a deleted org is kept before it is purged.
fn kept_for(days: u64) -> Duration {
    Duration::days(i64::try_from(days).unwrap_or(i64::MAX / SECONDS_PER_DAY))
}

/// The root orgs that were deleted and not purged yet. The org of the engine's views is a live
/// one; these are built from what is stored, with nothing held on them but the way back, which
/// is the restore.
async fn deleted(
    ctx: &Ctx,
    conn: &mut Conn,
    q: Option<&str>,
    cursor: Option<&str>,
    size: usize,
) -> ApiResult<Page<Org>> {
    let after = cursor.map(parse_deleted_cursor).transpose()?;
    let purged_before = ctx.now - kept_for(ctx.app.config.retention.soft_delete_days);
    let mut rows =
        hierarchy::deleted_root_orgs(conn, purged_before, q, after, read_ahead(size)).await?;
    let more = rows.len() > size;
    rows.truncate(size);
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let details = hierarchy::details(conn, &ids, ctx.now).await?;
    let facts: HashMap<Id, hierarchy::OrgFacts> = hierarchy::org_facts(conn, &ids, ctx.now)
        .await?
        .into_iter()
        .map(|facts| (facts.node_id, facts))
        .collect();
    let mut people: Vec<Id> = details.iter().map(|detail| detail.owner_id).collect();
    people.extend(details.iter().filter_map(|detail| detail.archived_by));
    let people = refusals::user_refs(conn, &people).await?;
    let mut items = Vec::new();
    for row in &rows {
        let Some(detail) = details.iter().find(|detail| detail.id == row.id) else {
            continue;
        };
        let Some(owner) = people.get(&detail.owner_id).cloned() else {
            return Err(ApiError::internal(format!(
                "the first owner of {} is not a user",
                row.id
            )));
        };
        let found = facts.get(&row.id);
        items.push(Org {
            base: NodeBase {
                inspection: None,
                id: detail.id,
                kind: OrgKind::Org,
                parent_id: detail.parent_id,
                org_id: detail.org_id,
                path: vec![NodeRef {
                    thread_display: None,
                    id: detail.id,
                    kind: NodeKind::Org,
                    name: Some(detail.name.clone()),
                    redacted: false,
                }],
                name: detail.name.clone(),
                description: detail.description.clone(),
                color: detail.color.and_then(|color| u32::try_from(color).ok()),
                custom_color: detail.custom_color.clone(),
                // The list of every org is no view of one: an org's README is read on its own page.
                readme: None,
                owner,
                owners: detail.owners,
                title_source: field_source(&detail.title_source),
                description_source: field_source(&detail.description_source),
                // Management lists identities; assigned tags are read on the org's own page.
                tags: Vec::new(),
                tags_source: field_source(detail.tags_source.as_deref().unwrap_or("auto")),
                auto_updated_at: detail.auto_updated_at.map(Time::new),
                page: PageState::Follows,
                chat_id: detail.chat_id.unwrap_or_else(Id::nil),
                archived: detail.archived_at.is_some(),
                archived_at: detail.archived_at.map(Time::new),
                archived_by: detail.archived_by.and_then(|by| people.get(&by).cloned()),
                revision: detail.revision,
                created_at: Time::new(detail.created_at),
                updated_at: Time::new(detail.updated_at),
                access: Access {
                    admin_place: None,
                    admin_only: Vec::new(),
                    admin_access: None,
                    participating: false,
                    permissions: Vec::new(),
                    role: None,
                    role_from: None,
                    administers: true,
                    owns: false,
                    subscription: None,
                    effective_subscription: SubscriptionLevel::Off,
                    inherited_subscription: None,
                },
            },
            slug: row.slug.clone().unwrap_or_default(),
            address: found.map(|found| found.address.clone()).unwrap_or_default(),
            depth: 0,
            org_kind: OrgType::Team,
            closed: detail.closed.unwrap_or_default(),
            flat: detail.flat.unwrap_or_default(),
            member_count: found.map(|found| found.member_count).unwrap_or_default(),
            people_count: found.map(|facts| facts.people_count),
            org_count: found.map(|found| found.org_count).unwrap_or_default(),
            project_count: found.map(|found| found.project_count).unwrap_or_default(),
            changed_at: Time::new(found.map_or(detail.updated_at, |found| found.changed_at)),
        });
    }
    Ok(Page {
        next_cursor: more.then(|| rows.last().map(cursor_of_deleted)).flatten(),
        items,
    })
}

fn field_source(word: &str) -> FieldSource {
    if word == "auto" {
        FieldSource::Auto
    } else {
        FieldSource::User
    }
}

/// The sentence a history line says of a change of limits.
fn limits_summary(who: &str, name: &str, closed: Option<bool>, flat: Option<bool>) -> String {
    let mut parts = Vec::new();
    match closed {
        Some(true) => parts.push(format!("closed {name} to further members")),
        Some(false) => parts.push(format!("opened {name} to further members")),
        None => {}
    }
    match flat {
        Some(true) => parts.push(format!("stopped orgs from being made inside {name}")),
        Some(false) => parts.push(format!("let orgs be made inside {name}")),
        None => {}
    }
    format!("{who} {}", parts.join(" and "))
}

/// Sets the limits of a root org. What is asked for and already so changes nothing and leaves no
/// line.
async fn update(mut ctx: Ctx, (): (), body: OrgLimitsChange) -> ApiResult<Org> {
    admin_of(&ctx)?;
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let org = hierarchy::lock_live_org(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if org.parent_id.is_some() {
        return Err(invalid(
            "id",
            "an org that lies in another has no limits of its own: they are its root org's",
        ));
    }
    let closed = body.closed.filter(|asked| *asked != org.closed);
    let flat = body.flat.filter(|asked| *asked != org.flat);
    if closed.is_some() || flat.is_some() {
        hierarchy::set_org_limits(&mut tx, id, closed, flat, ctx.now).await?;
        let name = hierarchy::with_ancestors(&mut tx, &[id])
            .await?
            .into_iter()
            .find(|row| row.id == id)
            .map(|row| row.name)
            .ok_or_else(ApiError::not_found)?;
        let who = guard.user().user.display_name.clone();
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::OrgLimitsChanged),
            Placed { id, org: id },
            &limits_summary(&who, &name, closed, flat),
            json!({
                "closed": { "was": org.closed, "now": closed.unwrap_or(org.closed) },
                "flat": { "was": org.flat, "now": flat.unwrap_or(org.flat) },
            }),
        )
        .await?;
        hierarchy::stamp(&mut tx, id).await?;
    }
    let shown = view::one(&mut tx, guard.principal(), id, ctx.now, Policy::default()).await?;
    tx.commit().await.map_err(DbError::from)?;
    match shown {
        Some(Node::Org(org)) => Ok(org),
        _ => Err(ApiError::not_found()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_of_limits_is_told_in_the_words_of_what_it_does() {
        assert_eq!(
            limits_summary("Ada", "Acme", Some(true), None),
            "Ada closed Acme to further members"
        );
        assert_eq!(
            limits_summary("Ada", "Acme", Some(false), Some(false)),
            "Ada opened Acme to further members and let orgs be made inside Acme"
        );
    }

    #[test]
    fn a_cursor_of_orgs_keeps_a_name_with_any_characters() {
        let id = orch_types::new_id();
        for personal in [false, true] {
            let (kind, back, name) =
                parse_org_cursor(&cursor_of_org(personal, id, "A|B, 3")).unwrap();
            assert_eq!((kind, back, name.as_str()), (personal, id, "A|B, 3"));
        }
        assert!(
            parse_org_cursor(&format!("{id}|Acme")).is_err(),
            "a cursor without its kind"
        );
    }
}
