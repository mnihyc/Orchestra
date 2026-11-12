//! Personal pages, visible pins, work by day and direct shares between people, with recorded inspection of otherwise closed pages.

pub(crate) use profile_read::node_authority_preserved;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use crate::routes::nodes::view;

use chrono::Offset;

use orch_access_contract::permissions::NodePermission;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::PermissionName;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::users::MeProfilePinsSet;

use orch_api_contract::users::MeProfilePinsSetBody;

use orch_api_contract::users::MeProfileUpdate;

use orch_api_contract::users::MeProfileUpdateBody;

use orch_api_contract::users::ProfileActivityGet;

use orch_api_contract::users::ProfileAudience;

use orch_api_contract::users::ProfileGet;

use orch_api_contract::users::ProfileLocalTime;

use orch_api_contract::users::ProfilePin;

use orch_api_contract::users::ProfilePinInput;

use orch_api_contract::users::ProfileSettings;

use orch_api_contract::users::UserProfile;

use orch_api_contract::users::UserSharesGet;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::nodes;

use orch_db::queries::access;

use orch_db::queries::chats;

use orch_db::queries::documents;

use orch_db::queries::identity;

use orch_db::queries::profiles;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
// The page's published bounds keep the small identity page small.
const BIO_CHARS: usize = 2000;

const SHORT_CHARS: usize = 40;

const LINK_URL_CHARS: usize = 500;

const ITEMS_MAX: usize = 6;

// UTC offsets are returned as whole minutes.
const SECONDS_PER_MINUTE: i32 = 60;

/// Binds personal-page requests and the direct shares shown on a person's card.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            ProfileGet,
            crate::key_operations::native::selected::<profile_read::Profile>,
            crate::key_operations::native::selected_read::<profile_read::Profile>,
        )
        .on_snapshot_read_with_optional_operation(
            ProfileActivityGet,
            crate::key_operations::native::selected::<activity_read::Activity>,
            crate::key_operations::native::selected_read::<activity_read::Activity>,
        )
        .on(MeProfileUpdate, update)
        .on(MeProfilePinsSet, set_pins)
        .on_snapshot_read_with_optional_operation(
            UserSharesGet,
            crate::key_operations::native::selected::<shares_read::Shares>,
            crate::key_operations::native::selected_read::<shares_read::Shares>,
        );
}
orch_api_base::__api_bind_82605ffc6d!();

struct Opened {
    person: identity::UserRow,
    page: profiles::ProfileRow,
    memberships: Vec<access::MembershipRow>,
    inspected: Option<NodeRef>,
}

async fn open_person(
    conn: &mut Conn,
    principal: &Principal,
    person: identity::UserRow,
) -> ApiResult<Opened> {
    let page = profiles::get(conn, person.id).await?;
    let memberships = access::memberships(conn, person.id).await?;
    let ordinary = principal.user() == person.id
        || page.audience == "system"
        || (page.audience == "orgs" && memberships.iter().any(|m| principal.is_in_tree(m.org_id)));
    let inspected = if ordinary {
        None
    } else {
        // Start with the person's domain, so the band names the nearest qualifying org.
        let ids: Vec<Id> = memberships
            .iter()
            .flat_map(|m| m.domain_path.iter().rev().copied())
            .collect();
        if !principal.caller().as_admin() {
            return Err(ApiError::not_found());
        }
        let graph = orch_authz::engine::Graph::load(conn, &ids)
            .await
            .map_err(ApiError::internal)?;
        let sites: Vec<_> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
        let frame = orch_authz::engine::Frame::load(
            conn,
            principal,
            Policy::default(),
            graph,
            &sites,
            &[Permission::MembersManage],
        )
        .await
        .map_err(ApiError::internal)?;
        let org = sites
            .iter()
            .find(|site| frame.administers(site))
            .ok_or_else(ApiError::not_found)?
            .id;
        Some(
            view::qualified_refs(conn, principal, &[org], Policy::default())
                .await?
                .remove(&org)
                .ok_or_else(ApiError::not_found)?,
        )
    };
    Ok(Opened {
        person,
        page,
        memberships,
        inspected,
    })
}

/// The native scope for a privately opened person's page; explicit Members permissions
/// alone cannot inspect it, and an ordinary audience match never carries the marker.
pub(crate) async fn admin_place(
    conn: &mut Conn,
    principal: &Principal,
    user: Id,
) -> ApiResult<Option<orch_access_contract::roles::PlaceRef>> {
    if !principal.caller().as_admin() {
        return Ok(None);
    }
    let person = identity::user_by_id(conn, user)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let page = profiles::get(conn, user).await?;
    let memberships = access::memberships(conn, user).await?;
    if principal.user() == person.id
        || page.audience == "system"
        || (page.audience == "orgs" && memberships.iter().any(|m| principal.is_in_tree(m.org_id)))
    {
        return Ok(None);
    }
    let ids: Vec<_> = memberships
        .iter()
        .flat_map(|m| m.domain_path.iter().rev().copied())
        .collect();
    let graph = orch_authz::engine::Graph::load(conn, &ids)
        .await
        .map_err(ApiError::internal)?;
    let sites: Vec<_> = ids.iter().filter_map(|id| graph.site_of(*id)).collect();
    let frame = orch_authz::engine::Frame::load(
        conn,
        principal,
        Policy::default(),
        graph,
        &sites,
        &[Permission::MembersManage],
    )
    .await
    .map_err(ApiError::internal)?;
    Ok(sites.iter().find_map(|site| frame.admin_place(site)))
}
orch_api_base::__api_bind_6e5f32e3ad!();

async fn pins(
    conn: &mut Conn,
    principal: &Principal,
    user: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<ProfilePin>> {
    let stored = profiles::pins(conn, user).await?;
    let node_ids: Vec<Id> = stored
        .iter()
        .filter(|(kind, _)| kind == "node")
        .map(|(_, id)| *id)
        .collect();
    let doc_ids: Vec<Id> = stored
        .iter()
        .filter(|(kind, _)| kind == "document")
        .map(|(_, id)| *id)
        .collect();
    let mut visible_nodes: HashMap<Id, _> =
        view::visible(conn, principal, &node_ids, now, Policy::default())
            .await?
            .into_iter()
            .map(|node| (crate::routes::nodes::node_id(&node), node))
            .collect();
    let listed = orch_authz::documents::listed(conn, principal, &doc_ids, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let mut held: HashMap<Id, _> = listed
        .into_iter()
        .filter(|each| each.holds(Permission::DocView))
        .map(|each| (each.document.id, each))
        .collect();
    let entries: Vec<_> = documents::get_many(conn, &doc_ids)
        .await?
        .into_iter()
        .filter(|row| row.deleted_at.is_none())
        .filter_map(|row| held.remove(&row.id).map(|each| (row, each)))
        .collect();
    let mut visible_docs: HashMap<Id, _> =
        crate::routes::documents::build::refs(conn, principal, &entries, now, Policy::default())
            .await?
            .into_iter()
            .map(|doc| (doc.id, doc))
            .collect();
    Ok(stored
        .into_iter()
        .filter_map(|(kind, id)| {
            if kind == "node" {
                visible_nodes.remove(&id).map(|node| ProfilePin {
                    node: Some(node),
                    document: None,
                })
            } else {
                visible_docs.remove(&id).map(|document| ProfilePin {
                    node: None,
                    document: Some(document),
                })
            }
        })
        .collect())
}

async fn may_message(conn: &mut Conn, principal: &Principal, opened: &Opened) -> ApiResult<bool> {
    if principal.user() == opened.person.id || opened.person.suspended() {
        return Ok(false);
    }
    for membership in &opened.memberships {
        let root = membership.org_id;
        if !principal.is_in_tree(root)
            || principal
                .suspension()
                .closing(&membership.domain_path)
                .is_some()
        {
            continue;
        }
        let standing = chats::standing(conn, root, &[principal.user(), opened.person.id]).await?;
        if standing.len() != 2 || !standing.iter().all(chats::Standing::is_present) {
            continue;
        }
        let graph = orch_authz::engine::Graph::load(conn, &[root])
            .await
            .map_err(ApiError::internal)?;
        if graph
            .site_of(root)
            .is_none_or(|site| site.archived_node.is_some())
        {
            continue;
        }
        if orch_chat::chats::direct_allowed(conn, root)
            .await
            .map_err(ApiError::internal)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn shown(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    opened: Opened,
) -> ApiResult<UserProfile> {
    let pins = pins(conn, principal, opened.person.id, ctx.now).await?;
    shown_parts(ctx, conn, principal, &opened, pins, None).await
}

async fn shown_parts(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    opened: &Opened,
    pins: Vec<ProfilePin>,
    places: Option<&BTreeSet<Id>>,
) -> ApiResult<UserProfile> {
    let is_me = principal.user() == opened.person.id;
    let mut domain_ids: Vec<Id> = opened
        .memberships
        .iter()
        .filter(|m| principal.is_in_tree(m.org_id))
        .map(|m| m.domain_id)
        .collect();
    domain_ids.extend(opened.person.personal_org_id);
    domain_ids.retain(|id| places.is_none_or(|places| places.contains(id)));
    let visible: HashSet<Id> = nodes::holding(
        conn,
        principal,
        &domain_ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|each| each.id)
    .collect();
    let ids: Vec<_> = domain_ids
        .into_iter()
        .filter(|id| visible.contains(id))
        .collect();
    let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
    let member_of = opened
        .memberships
        .iter()
        .filter(|m| principal.is_in_tree(m.org_id))
        .filter_map(|m| refs.get(&m.domain_id).cloned())
        .collect();
    let personal_org = opened
        .person
        .personal_org_id
        .and_then(|id| refs.get(&id).cloned());
    let local_time = if opened.page.show_time {
        opened
            .person
            .timezone
            .parse::<chrono_tz::Tz>()
            .ok()
            .map(|zone| ProfileLocalTime {
                timezone: opened.person.timezone.clone(),
                utc_offset_minutes: i64::from(
                    ctx.now
                        .with_timezone(&zone)
                        .offset()
                        .fix()
                        .local_minus_utc()
                        / SECONDS_PER_MINUTE,
                ),
            })
    } else {
        None
    };
    let user = refusals::user_refs(conn, &[opened.person.id])
        .await?
        .remove(&opened.person.id)
        .ok_or_else(ApiError::not_found)?;
    let can_message = may_message(conn, principal, opened).await?;
    let audience = match opened.page.audience.as_str() {
        "orgs" => ProfileAudience::Orgs,
        "private" => ProfileAudience::Private,
        _ => ProfileAudience::System,
    };
    let settings = is_me.then_some(ProfileSettings {
        show_time: opened.page.show_time,
        show_activity: opened.page.show_activity,
        audience,
    });
    Ok(UserProfile {
        user,
        joined_at: Time::new(opened.person.created_at),
        is_me,
        bio: opened.page.bio.clone(),
        pronouns: opened.page.pronouns.clone(),
        links: serde_json::from_value(opened.page.links.clone()).map_err(ApiError::internal)?,
        local_time,
        member_of,
        personal_org,
        pins,
        activity: is_me || opened.page.show_activity,
        can_message,
        inspected: opened.inspected.clone(),
        settings,
    })
}

fn validate(body: &MeProfileUpdateBody) -> ApiResult<()> {
    for (field, text, most) in [
        ("bio", body.bio.as_deref(), BIO_CHARS),
        ("pronouns", body.pronouns.as_deref(), SHORT_CHARS),
    ] {
        if text.is_some_and(|text| text.chars().count() > most) {
            return Err(invalid(
                field,
                &format!("must be at most {most} characters"),
            ));
        }
    }
    if let Some(links) = &body.links {
        if links.len() > ITEMS_MAX {
            return Err(invalid("links", "must contain at most six links"));
        }
        for (index, link) in links.iter().enumerate() {
            if link.label.trim().is_empty() || link.label.chars().count() > SHORT_CHARS {
                return Err(invalid(
                    &format!("links[{index}].label"),
                    "must be one to forty characters",
                ));
            }
            if link.url.chars().count() > LINK_URL_CHARS
                || !url::Url::parse(&link.url)
                    .is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some())
            {
                return Err(invalid(
                    &format!("links[{index}].url"),
                    "must be an HTTPS address of at most five hundred characters",
                ));
            }
        }
    }
    Ok(())
}

async fn own(conn: &mut Conn, principal: &Principal) -> ApiResult<Opened> {
    let person = identity::user_by_id(conn, principal.user())
        .await?
        .filter(|user| user.kind == "human")
        .ok_or_else(ApiError::not_found)?;
    let page = profiles::get(conn, person.id).await?;
    let memberships = access::memberships(conn, person.id).await?;
    Ok(Opened {
        person,
        page,
        memberships,
        inspected: None,
    })
}

async fn update(ctx: Ctx, (): (), body: MeProfileUpdateBody) -> ApiResult<UserProfile> {
    validate(&body)?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    profiles::lock(&mut tx, principal.user()).await?;
    let mut opened = own(&mut tx, &principal).await?;
    if let Some(bio) = body.bio {
        opened.page.bio = bio;
    }
    if let Some(pronouns) = body.pronouns {
        opened.page.pronouns = pronouns;
    }
    if let Some(links) = body.links {
        opened.page.links = serde_json::to_value(links).map_err(ApiError::internal)?;
    }
    if let Some(show) = body.show_time {
        opened.page.show_time = show;
    }
    if let Some(show) = body.show_activity {
        opened.page.show_activity = show;
    }
    if let Some(audience) = body.audience {
        opened.page.audience = match audience {
            ProfileAudience::System => "system",
            ProfileAudience::Orgs => "orgs",
            ProfileAudience::Private => "private",
        }
        .to_owned();
    }
    profiles::put(&mut tx, principal.user(), &opened.page, ctx.now).await?;
    let profile = shown(&ctx, &mut tx, &principal, opened).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(profile)
}

async fn set_pins(ctx: Ctx, (): (), body: MeProfilePinsSetBody) -> ApiResult<UserProfile> {
    if body.pins.len() > ITEMS_MAX {
        return Err(invalid("pins", "must contain at most six pins"));
    }
    let ordered: Vec<(String, Id)> = body
        .pins
        .iter()
        .map(|pin| match pin {
            ProfilePinInput::Node(pin) => ("node".to_owned(), pin.node_id),
            ProfilePinInput::Document(pin) => ("document".to_owned(), pin.document_id),
        })
        .collect();
    if ordered.iter().collect::<HashSet<_>>().len() != ordered.len() {
        return Err(invalid("pins", "must not repeat a pin"));
    }
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    profiles::lock(&mut tx, principal.user()).await?;
    let opened = own(&mut tx, &principal).await?;
    let node_ids: Vec<Id> = ordered
        .iter()
        .filter(|(kind, _)| kind == "node")
        .map(|(_, id)| *id)
        .collect();
    let doc_ids: Vec<Id> = ordered
        .iter()
        .filter(|(kind, _)| kind == "document")
        .map(|(_, id)| *id)
        .collect();
    let node_count = nodes::holding(
        &mut tx,
        &principal,
        &node_ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .len();
    let doc_count = orch_authz::documents::listed(&mut tx, &principal, &doc_ids, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|each| each.holds(Permission::DocView))
        .count();
    if node_count != node_ids.len() || doc_count != doc_ids.len() {
        return Err(invalid(
            "pins",
            "each pin must be something you may view now",
        ));
    }
    profiles::put_pins(&mut tx, principal.user(), &ordered).await?;
    let profile = shown(&ctx, &mut tx, &principal, opened).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(profile)
}

#[path = "profiles/activity_read.rs"]
pub mod activity_read;

#[path = "profiles/profile_read.rs"]
pub mod profile_read;

#[path = "profiles/shares_read.rs"]
pub mod shares_read;
