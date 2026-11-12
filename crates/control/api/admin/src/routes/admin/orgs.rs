//! Every org of the system for the people who run it, and the two limits of a root org: whether its tree takes further members and whether it holds orgs inside it.
//!
//! A system admin is the Admin of every org, so the orgs are listed and shown as the engine shows
//! them to somebody who holds that. Setting a limit removes nobody and no org: it decides only what
//! may be done from then on. Only a root org has them (an org inside another is held by the limits
//! of its root), and it is the system, never the org's own owners, that sets them: a limit that its
//! subject could lift would be none.

use super::SECONDS_PER_DAY;

use super::invalid;

use super::page_size;

use super::read_ahead;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::Placed;

use crate::routes::nodes::record;

use chrono::DateTime;

use chrono::Duration;

use chrono::Utc;

use futures::future::BoxFuture;

use orch_access_contract::audit::AuditAction;

use orch_api_contract::admin::AdminOrgList;

use orch_api_contract::admin::AdminOrgListQuery;

use orch_api_contract::admin::AdminOrgUpdate;

use orch_api_contract::admin::OrgLimitsChange;

use orch_api_contract::admin::OrgLimitsReceipt;

use orch_api_contract::admin::OrgLimitsResponse;

use orch_api_contract::admin_sources as sources;

use orch_api_contract::admin_sources::Source;

use orch_api_contract::common::Page;

use orch_api_contract::common::SubscriptionLevel;

use orch_api_contract::documents::DeletedFilter;

use orch_api_contract::nodes::Access;

use orch_api_contract::nodes::FieldSource;

use orch_api_contract::nodes::NodeBase;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::nodes::Org;

use orch_api_contract::nodes::OrgKind;

use orch_api_contract::nodes::OrgType;

use orch_api_contract::nodes::PageState;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::queries::hierarchy;

use orch_db::queries::hierarchy::DeletedRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use serde_json::json;

use std::collections::HashMap;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            AdminOrgList,
            native::selected::<Organizations>,
            list,
        )
        .on_with_operation(AdminOrgUpdate, effects::prepare::<Limits>, update);
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
    native::selected_read::<Organizations>(ctx, query, ()).await
}

struct Organizations;

struct PreparedOrganizations {
    page: Page<Org>,
    selected: Vec<Id>,
}

impl native::Selected for Organizations {
    type Source = Source;
    type Query = AdminOrgListQuery;
    type Value = PreparedOrganizations;
    type Reply = Page<Org>;
    const DECLARATION: &'static Declaration<Source> = &sources::ORGS;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(select_organizations(ctx, conn, caller, query))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::native::require(ctx, principal) })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(value.page.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a Self::Query,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.selected.iter().all(|id| fresh.selected.contains(id))
                && opened.page.items.iter().all(|org| {
                    fresh
                        .page
                        .items
                        .iter()
                        .any(|row| row.base.id == org.base.id)
                }))
        })
    }
}

async fn select_organizations(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &AdminOrgListQuery,
) -> ApiResult<(PreparedOrganizations, Vec<native::Facts<Source>>)> {
    let size = page_size(query.limit);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut selected = Vec::new();
    let mut facts = Vec::new();
    if query.deleted == Some(DeletedFilter::Only) {
        let mut after = query
            .cursor
            .as_deref()
            .map(parse_deleted_cursor)
            .transpose()?;
        let purged_before = ctx.now - kept_for(ctx.app.config.retention.soft_delete_days);
        let mut rows = Vec::new();
        'pages: loop {
            let batch =
                hierarchy::deleted_root_orgs(conn, purged_before, q, after, read_ahead(size))
                    .await?;
            let exhausted = batch.len() < size.saturating_add(1);
            for row in batch {
                after = Some((row.deleted_at, row.id));
                let target = targets::node_identity(conn, row.id).await?;
                if super::native::permits(
                    ctx,
                    &sources::ORGS,
                    caller,
                    Source::Results,
                    target.object.kind,
                    &target,
                )? {
                    selected.push(row.id);
                    facts.push((target.object.kind, target));
                    rows.push(row);
                    if rows.len() > size {
                        break 'pages;
                    }
                }
            }
            if exhausted {
                break;
            }
        }
        let more = rows.len() > size;
        rows.truncate(size);
        let page = deleted_views(ctx, conn, &rows, more).await?;
        return Ok((
            PreparedOrganizations { page, selected },
            vec![
                super::native::collection(Source::Results, facts),
                super::native::collection(Source::Names, Vec::new()),
            ],
        ));
    }
    let mut after = query.cursor.as_deref().map(parse_org_cursor).transpose()?;
    let org_kind = query.org_kind.map(|kind| match kind {
        OrgType::Personal => "personal",
        OrgType::Team => "team",
    });
    let mut rows = Vec::new();
    'pages: loop {
        let batch = hierarchy::orgs_by_name(
            conn,
            q,
            org_kind,
            after
                .as_ref()
                .map(|(personal, id, name)| (*personal, name.as_str(), *id)),
            read_ahead(size),
        )
        .await?;
        let exhausted = batch.len() < size.saturating_add(1);
        for (id, name, personal) in batch {
            after = Some((personal, id, name.clone()));
            let target = targets::node(conn, id).await?;
            if super::native::permits(
                ctx,
                &sources::ORGS,
                caller,
                Source::Results,
                target.object.kind,
                &target,
            )? {
                selected.push(id);
                facts.push((target.object.kind, target));
                rows.push((id, name, personal));
                if rows.len() > size {
                    break 'pages;
                }
            }
        }
        if exhausted {
            break;
        }
    }
    let more = rows.len() > size;
    rows.truncate(size);
    let results = super::native::collection(Source::Results, facts);
    let plan = native::bind_known(ctx, &sources::ORGS, std::slice::from_ref(&results))?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let ids = rows.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
    let (items, names) =
        super::native::org_content(ctx, conn, &principal, caller, &sources::ORGS, &ids).await?;
    Ok((
        PreparedOrganizations {
            selected,
            page: Page {
                next_cursor: more
                    .then(|| {
                        rows.last()
                            .map(|(id, name, personal)| cursor_of_org(*personal, *id, name))
                    })
                    .flatten(),
                items,
            },
        },
        vec![results, names],
    ))
}

/// How long a deleted org is kept before it is purged.
fn kept_for(days: u64) -> Duration {
    Duration::days(i64::try_from(days).unwrap_or(i64::MAX / SECONDS_PER_DAY))
}

/// The root orgs that were deleted and not purged yet. The org of the engine's views is a live
/// one; these are built from what is stored, with nothing held on them but the way back, which
/// is the restore.
async fn deleted_views(
    ctx: &Ctx,
    conn: &mut Conn,
    rows: &[DeletedRow],
    more: bool,
) -> ApiResult<Page<Org>> {
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
    for row in rows {
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
async fn update(ctx: Ctx, (): (), _body: OrgLimitsChange) -> ApiResult<OrgLimitsResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Limits>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = OrgLimitsResponse::Receipt(OrgLimitsReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        id: done.output.id,
        revision: done.output.revision,
    });
    publication::receipt(&done.ctx, &receipt)?;
    Ok(
        reply::render::<LimitsContent>(&done.ctx, &done.accepted, done.output)
            .await?
            .map_or(receipt, OrgLimitsResponse::full),
    )
}

struct Limits;

struct PreparedLimits {
    org: hierarchy::OrgRow,
    name: String,
    closed: Option<bool>,
    flat: Option<bool>,
}

struct AcceptedLimits {
    id: Id,
    revision: orch_types::Revision,
}

impl effects::Effect for Limits {
    type Source = Source;
    type Query = ();
    type Body = OrgLimitsChange;
    type Value = PreparedLimits;
    type Output = AcceptedLimits;
    fn declaration(_value: &Self::Value) -> &'static Declaration<Source> {
        &sources::ORG_UPDATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        _query: (),
        body: OrgLimitsChange,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let org = hierarchy::lock_live_org(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if org.parent_id.is_some() {
                return Err(invalid(
                    "id",
                    "an org that lies in another has no limits of its own: they are its root org's",
                ));
            }
            // Both rows touched by the native writer belong to this preparation footprint.
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let name = hierarchy::with_ancestors(conn, &[id])
                .await?
                .into_iter()
                .find(|row| row.id == id)
                .map(|row| row.name)
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, id).await?;
            let closed = body.closed.filter(|asked| *asked != org.closed);
            let flat = body.flat.filter(|asked| *asked != org.flat);
            Ok((
                PreparedLimits {
                    org,
                    name,
                    closed,
                    flat,
                },
                vec![native::Facts::one(Source::Org, target.object.kind, target)],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::native::require(ctx, principal) })
    }
}

impl effects::Apply for Limits {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let id = value.org.node_id;
            let changed = value.closed.is_some() || value.flat.is_some();
            if changed {
                hierarchy::set_org_limits_before(
                    conn,
                    id,
                    value.closed,
                    value.flat,
                    ctx.now,
                    move || before.check(),
                )
                .await?;
                let who = ctx.user()?.user.display_name.clone();
                record(ctx,conn,orch_db::audit::action!(AuditAction::OrgLimitsChanged),Placed {id,org:id},
                    &limits_summary(&who,&value.name,value.closed,value.flat),json!({
                        "closed":{"was":value.org.closed,"now":value.closed.unwrap_or(value.org.closed)},
                        "flat":{"was":value.org.flat,"now":value.flat.unwrap_or(value.org.flat)},
                    })).await?;
                hierarchy::stamp(conn, id).await?;
            }
            let revision = sqlx::query_scalar("SELECT revision FROM nodes WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let accepted = AcceptedLimits { id, revision };
            Ok(if changed {
                effects::Outcome::Applied(accepted)
            } else {
                effects::Outcome::Unchanged(accepted)
            })
        })
    }
}

struct LimitsContent;

impl reply::Projection for LimitsContent {
    type Source = Source;
    type Input = AcceptedLimits;
    type Subject = Id;
    type Value = Org;
    type Reply = Org;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_input: &Self::Input) -> &'static Declaration<Source> {
        &sources::ORG_UPDATE
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let target = targets::node(conn, input.id).await?;
            Ok((
                input.id,
                vec![
                    native::Facts::one(Source::Org, target.object.kind, target.clone()),
                    native::Facts::one(Source::Content, target.object.kind, target),
                ],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        _subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { super::native::standing(ctx, principal) })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Org, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (mut orgs, names) = super::native::org_content(
                ctx,
                conn,
                principal,
                principal.caller(),
                &sources::ORG_UPDATE,
                &[*subject],
            )
            .await?;
            Ok((orgs.pop().ok_or_else(ApiError::not_found)?, vec![names]))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Org>> {
        Box::pin(async move { Ok(prepared.value.clone()) })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject == fresh.subject && old.value.base.id == fresh.value.base.id)
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_change_of_limits_is_told_in_the_words_of_what_it_does() {
        self::a_change_of_limits_is_told_in_the_words_of_what_it_does()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_cursor_of_orgs_keeps_a_name_with_any_characters() {
        self::a_cursor_of_orgs_keeps_a_name_with_any_characters()
    }
}
