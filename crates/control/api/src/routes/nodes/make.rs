//! Making an org inside an org, a project and a topic, and a team's root org, each with the Owners row of its maker and the audit row of its making.
//!
//! A node is made in one transaction: the node with its side row and its
//! chat, the Owners row that names its maker as its first owner, the page it
//! starts with (the defaults, or cut loose when its maker asks for a private
//! one), the audit row, and the stamp that tells whoever has the place open.
//! Who may make what is asked again inside the action transaction after
//! the body arrives; the handler also checks what only the request knows: the
//! tree's two limits, how deep orgs may lie in one another, and that the
//! names and short names are fit.

use super::{
    GENERAL_PROJECT, NEW_TOPIC, Placed, actor_name, clean_description, clean_name, invalid, record,
    shown,
};
use crate::middleware::authentication::Authed;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in::{origin, system_policy};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages::{PageChange, StoredKind, Whole};
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::auth::OrgCreation;
use orch_api_contract::common::{ErrorCode, FieldFault};
use orch_api_contract::nodes::{
    NewNodeAccess, Node, NodeKind, Org, OrgChildCreate, OrgChildCreateBody, OrgCreate,
    OrgCreateBody, Project, ProjectChildCreate, ProjectCreate, ProjectCreateBody, TopicCreate,
    TopicCreateBody, TopicCreateReply, TopicWithThread,
};
use orch_authz::pages::{self, Target};
use orch_authz::{Policy, Principal, epoch, orgs};
use orch_db::queries::{access, hierarchy, identity};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// The fewest characters of a root org's short name: the same as a username,
/// whose namespace it shares.
pub const ROOT_SLUG_MIN_CHARS: usize = 3;
/// The fewest characters of the short name of an org inside another.
pub const SLUG_MIN_CHARS: usize = 2;
/// The most characters of a short name: what the schema allows.
pub const SLUG_MAX_CHARS: usize = 32;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(OrgCreate, org_create)
        .on(OrgChildCreate, org_child_create)
        .on(ProjectCreate, project_create)
        .on(ProjectChildCreate, project_child_create)
        .on(TopicCreate, topic_create);
}

/// The most numbered short names tried when one is offered in place of a taken one ("dep2-2" to "dep2-99"):
/// a parent that holds more of one stem than that is offered none, and the person chooses one themself.
const SUGGESTIONS_TRIED: usize = 99;
/// The most characters the number added to an offered short name takes, with its hyphen: "-99".
const SUGGESTION_TAIL_CHARS: usize = 3;

/// A short name as it is kept: lower-case letters, digits, hyphens and
/// underscores, beginning with a letter or a digit. Kept in small letters
/// alone, two short names that differ only in case cannot both be made, and
/// an address is found whatever case it is written in.
fn clean_slug(path: &str, slug: &str, fewest: usize) -> ApiResult<String> {
    let slug = slug.trim();
    let length = slug.chars().count();
    let fit = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && slug
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !fit || length < fewest || length > SLUG_MAX_CHARS {
        return Err(invalid(
            path,
            &format!(
                "must be {fewest} to {SLUG_MAX_CHARS} lower-case letters, digits, hyphens or underscores, beginning with a letter or a digit"
            ),
        ));
    }
    Ok(slug.to_owned())
}

/// A node to make.
struct New<'a> {
    kind: &'static str,
    parent: Option<Id>,
    name: &'a str,
    name_by_person: bool,
    description: &'a str,
    org: Option<hierarchy::NewOrg<'a>>,
    access: Option<NewNodeAccess>,
    /// The invitation to make a team org that making this one used up, named in its audit row.
    invitation: Option<identity::OrgInvitation>,
}

/// The first short name of the stem of `slug` that nothing in its namespace holds ("dep2-2", "dep2-3"),
/// cut to fit the longest short name; `None` when every one tried is taken.
fn suggestion(slug: &str, taken: &[String]) -> Option<String> {
    (2..=SUGGESTIONS_TRIED).find_map(|number| {
        let tail = format!("-{number}");
        let stem: String = slug
            .chars()
            .take(SLUG_MAX_CHARS.saturating_sub(tail.len()))
            .collect();
        let offered = format!("{}{tail}", stem.trim_end_matches(['-', '_']));
        (!taken.contains(&offered)).then_some(offered)
    })
}

/// Refuses, before anything is made or moved, a short name that another org
/// of the same parent holds (the root orgs and the usernames for `None`): an
/// address names exactly one org, and only a clash of addresses is refused. The
/// refusal names the org that holds it where the caller administers the
/// parent, so sees what lies in it, and offers the first free short name of the
/// same stem; at the root it names nobody, since another tree is not the
/// caller's to see.
pub(super) async fn slug_free(conn: &mut Conn, parent: Option<Id>, slug: &str) -> ApiResult<()> {
    let sibling = hierarchy::org_with_slug(conn, parent, slug).await?;
    let person = parent.is_none()
        && sibling.is_none()
        && crate::sign_in::accounts::public_name_taken(conn, slug).await?;
    if sibling.is_none() && !person {
        return Ok(());
    }
    let prefix: String = slug
        .chars()
        .take(SLUG_MAX_CHARS - SUGGESTION_TAIL_CHARS)
        .collect();
    let taken =
        hierarchy::slugs_beginning(conn, parent, prefix.trim_end_matches(['-', '_'])).await?;
    let offered = suggestion(slug, &taken);
    let named = sibling.as_ref().filter(|_| parent.is_some());
    let detail = match named {
        Some(org) if org.deleted => format!(
            "{} holds the short name {slug} here already; it was deleted and can still be brought back",
            org.name
        ),
        Some(org) => format!("{} holds the short name {slug} here already", org.name),
        None => format!("the address {slug} is taken"),
    };
    let message = match &offered {
        Some(offered) => format!("{detail}; {offered} is free"),
        None => detail.clone(),
    };
    let mut error = ApiError::new(ErrorCode::ConflictExists, detail).with(
        "errors",
        vec![FieldFault {
            path: "slug".to_owned(),
            message,
        }],
    );
    if let Some(org) = named {
        error = error.with("existing_id", org.id).with(
            "existing",
            json!({ "id": org.id, "name": org.name, "deleted": org.deleted }),
        );
    }
    if let Some(offered) = offered {
        error = error.with("suggestion", offered);
    }
    Err(error)
}

/// A short name that is taken, or any other unique value.
fn taken(error: DbError) -> ApiError {
    match error {
        DbError::Unique { constraint } if constraint == "orgs_slug_key" => ApiError::new(
            ErrorCode::ConflictExists,
            "an org with that short name exists already here",
        ),
        other => other.into(),
    }
}

/// Makes the node with everything that goes with it, in the caller's
/// transaction, and returns it.
async fn make(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    new: &New<'_>,
) -> ApiResult<Placed> {
    // Select before writing so an outside creator cannot leave an ownerless thing behind.
    let first = match orch_authz::ownership::owner_of_new(conn, principal.user(), new.parent).await
    {
        Ok(first) => first,
        Err(error) => return Err(refusals::problem(conn, error).await),
    };
    let id = new_id();
    hierarchy::create(
        conn,
        &hierarchy::NewNode {
            id,
            kind: new.kind,
            parent: new.parent,
            name: new.name,
            name_by_person: new.name_by_person,
            description: new.description,
            description_by_person: !new.description.is_empty(),
            owner: first,
            org: new.org,
            at: ctx.now,
        },
    )
    .await
    .map_err(taken)?;
    Box::pin(super::color::start(conn, id, new.parent)).await?;
    Box::pin(pages::start(
        conn,
        StoredKind::Node,
        id,
        principal.user(),
        Time::new(ctx.now),
    ))
    .await
    .map_err(ApiError::internal)?;
    let (place, _) = super::placed(conn, id).await?;
    if new.access == Some(NewNodeAccess::Private) {
        for target in [Target::Live(id), Target::Defaults(PlaceRef::Node(id))] {
            // This private-page branch belongs to every make future, including defaults.
            // Keep its complete authorization state out of nested org/project creation.
            Box::pin(pages::change(
                conn,
                principal,
                target,
                &PageChange {
                    whole: Some(Whole::Private),
                    ..PageChange::default()
                },
                Policy::default(),
                &origin(ctx),
            ))
            .await
            .map_err(|error| error_of(&error))?;
        }
    }
    let who = actor_name(ctx)?;
    let mut details = json!({
        "kind": new.kind,
        "name": new.name,
        "parent": new.parent,
        "private": new.access == Some(NewNodeAccess::Private),
    });
    if let Some(used) = new.invitation {
        details["invitation"] =
            json!({ "given_by": used.given_by, "given_at": Time::new(used.given_at) });
    }
    record(
        ctx,
        conn,
        orch_db::audit::action!(AuditAction::NodeCreated),
        place,
        &format!("{who} made the {} {}", new.kind, new.name),
        details,
    )
    .await?;
    Ok(place)
}

/// What a page change that this part makes for a new node can fail with: it
/// is its maker's own page, so a failure is a defect.
fn error_of(error: &pages::ChangeError) -> ApiError {
    ApiError::internal(format!("a new node's page could not be set: {error}"))
}

/// Makes an org inside another, which people call a department.
async fn org_child_create(ctx: Ctx, (): (), body: OrgChildCreateBody) -> ApiResult<Org> {
    let parent = ctx.id("id")?;
    let name = clean_name("name", &body.name)?;
    let slug = clean_slug("slug", &body.slug, SLUG_MIN_CHARS)?;
    let description = clean_description("description", body.description.as_deref().unwrap_or(""))?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    let (place, kind) = super::placed(&mut tx, parent).await?;
    if kind != NodeKind::Org {
        return Err(ApiError::not_found());
    }
    slug_free(&mut tx, Some(parent), &slug).await?;
    let rows = hierarchy::with_ancestors(&mut tx, &[parent]).await?;
    let above = rows
        .iter()
        .find(|row| row.id == parent)
        .map(|row| {
            row.path
                .iter()
                .filter(|place| {
                    rows.iter()
                        .any(|row| row.id == **place && row.kind == "org")
                })
                .count()
        })
        .unwrap_or_default();
    if orgs::limits(&mut tx, place.org)
        .await
        .map_err(ApiError::internal)?
        .is_some_and(|limits| limits.flat)
    {
        return Err(
            ApiError::new(ErrorCode::OrgFlat, "this tree holds no org inside it")
                .with("org", refusals::named_node(&mut tx, place.org).await?),
        );
    }
    let limit = ctx.app.config.limits.org_depth;
    if !orgs::fits_depth(above + 1, limit) {
        return Err(ApiError::new(
            ErrorCode::OrgTooDeep,
            format!("orgs may lie in one another {limit} deep at most"),
        )
        .with("limit", limit));
    }
    let made = make(
        &ctx,
        &mut tx,
        &principal,
        &New {
            kind: "org",
            parent: Some(parent),
            name: &name,
            name_by_person: true,
            description: &description,
            org: Some(hierarchy::NewOrg {
                slug: &slug,
                org_kind: "team",
            }),
            access: body.access,
            invitation: None,
        },
    )
    .await?;
    if body.general == Some(true) {
        make_general(&ctx, &mut tx, &principal, made.id).await?;
    }
    let node = shown(&ctx, &mut tx, &principal, made.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    expect_org(node)
}

/// The project that a team's root org starts with, and that an org inside
/// another is given when its maker asks.
async fn make_general(ctx: &Ctx, conn: &mut Conn, principal: &Principal, org: Id) -> ApiResult<()> {
    // A root org creates this project in the same transaction, without nesting its make state.
    Box::pin(make(
        ctx,
        conn,
        principal,
        &New {
            kind: "project",
            parent: Some(org),
            name: GENERAL_PROJECT,
            name_by_person: true,
            description: "",
            org: None,
            access: None,
            invitation: None,
        },
    ))
    .await?;
    Ok(())
}

fn expect_org(node: Node) -> ApiResult<Org> {
    match node {
        Node::Org(org) => Ok(org),
        _ => Err(ApiError::internal("an org was made and is not one")),
    }
}

/// Whether `caller` may make a team org now, by `orgs.created_by` as `creation` holds it: anybody
/// signed in, a system admin in a browser session, or, while it is `invited`, the holder of an
/// unused invitation to make one. A service identity makes none.
pub(crate) fn may_make_org(creation: OrgCreation, caller: &Authed) -> bool {
    caller.user.kind == "human"
        && match creation {
            OrgCreation::Anyone => true,
            OrgCreation::Admins => caller.is_system_admin(),
            OrgCreation::Invited => {
                caller.is_system_admin() || caller.user.org_invited_at.is_some()
            }
        }
}

/// Asks `orgs.created_by` in the action's transaction whether the caller may make a team org, and
/// refuses with `org.creation_disabled` when it says no. Where an invitation is what lets them, it
/// is used up here, in the same transaction as the org it makes, and returned for its audit row:
/// of two requests at once on one invitation, one makes an org and the other is refused.
async fn org_maker(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Option<identity::OrgInvitation>> {
    let caller = ctx.user()?;
    let creation = system_policy(conn).await?.org_creation;
    let refused = |why: &str| {
        ApiError::new(ErrorCode::OrgCreationDisabled, why.to_owned()).with("org_creation", creation)
    };
    if caller.user.kind != "human" {
        return Err(refused("a service identity makes no team org"));
    }
    match creation {
        OrgCreation::Anyone => Ok(None),
        _ if caller.is_system_admin() => Ok(None),
        OrgCreation::Admins => Err(refused("only a system admin makes a team org here")),
        OrgCreation::Invited => identity::take_org_invitation(conn, caller.id())
            .await?
            .map(Some)
            .ok_or_else(|| {
                refused("a team org is made here by a system admin, or by somebody a system admin invited to make one")
            }),
    }
}

/// Makes a root org of kind team, with the caller as its first owner and
/// its first member, and a `General` project in it.
async fn org_create(ctx: Ctx, (): (), body: OrgCreateBody) -> ApiResult<Org> {
    let name = clean_name("name", &body.name)?;
    let slug = clean_slug("slug", &body.slug, ROOT_SLUG_MIN_CHARS)?;
    let (ctx, mut tx, principal) = Box::pin(super::action(&ctx)).await?;
    let invitation = org_maker(&ctx, &mut tx).await?;
    slug_free(&mut tx, None, &slug).await?;
    let made = Box::pin(make(
        &ctx,
        &mut tx,
        &principal,
        &New {
            kind: "org",
            parent: None,
            name: &name,
            name_by_person: true,
            description: "",
            org: Some(hierarchy::NewOrg {
                slug: &slug,
                org_kind: "team",
            }),
            access: None,
            invitation,
        },
    ))
    .await?;
    // Their domain is the org itself; the role they hold there is the one
    // that owning gives.
    access::add_member(&mut tx, made.id, principal.user(), made.id, None, ctx.now).await?;
    epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
    Box::pin(make_general(&ctx, &mut tx, &principal, made.id)).await?;
    // The caller was a member of nothing here when they were read: read them
    // again to show the org as they now hold it.
    let principal = Box::pin(authorization::principal(&ctx, &mut tx)).await?;
    let node = shown(&ctx, &mut tx, &principal, made.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    expect_org(node)
}

/// Makes a project in an org, at whatever level it lies.
async fn project_create(ctx: Ctx, (): (), body: ProjectCreateBody) -> ApiResult<Project> {
    make_project(&ctx, NodeKind::Org, &body).await
}

/// Makes a project inside a project, beside its topics: what the screens call a folder, and a project
/// like any other, made in the same form by whoever opens topics there.
async fn project_child_create(ctx: Ctx, (): (), body: ProjectCreateBody) -> ApiResult<Project> {
    make_project(&ctx, NodeKind::Project, &body).await
}

/// Makes a project in the node the request names, which must be of the kind `parent_kind`: an org or a
/// project. A project inside a project is refused when the path would hold more projects one inside
/// another than the system allows.
async fn make_project(
    ctx: &Ctx,
    parent_kind: NodeKind,
    body: &ProjectCreateBody,
) -> ApiResult<Project> {
    let parent = ctx.id("id")?;
    let name = clean_name("name", &body.name)?;
    let description = clean_description("description", body.description.as_deref().unwrap_or(""))?;
    let (current, mut tx, principal) = super::action(ctx).await?;
    let ctx = &current;
    let (_, kind) = super::placed(&mut tx, parent).await?;
    if kind != parent_kind {
        return Err(ApiError::not_found());
    }
    if kind == NodeKind::Project {
        let limit = ctx.app.config.limits.project_depth;
        let held = hierarchy::projects_on_the_way(&mut tx, parent).await?;
        if !orgs::fits_project_depth(held + 1, limit) {
            return Err(ApiError::new(
                ErrorCode::ProjectTooDeep,
                format!("projects may lie in one another {limit} deep at most"),
            )
            .with("limit", limit));
        }
    }
    let made = make(
        ctx,
        &mut tx,
        &principal,
        &New {
            kind: "project",
            parent: Some(parent),
            name: &name,
            name_by_person: true,
            description: &description,
            org: None,
            access: body.access,
            invitation: None,
        },
    )
    .await?;
    let node = shown(ctx, &mut tx, &principal, made.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    match node {
        Node::Project(project) => Ok(project),
        _ => Err(ApiError::internal("a project was made and is not one")),
    }
}

/// Makes a topic in a project. A thread made with it is the threads routes'.
async fn topic_create(ctx: Ctx, (): (), body: TopicCreateBody) -> ApiResult<TopicCreateReply> {
    let parent = ctx.id("id")?;
    let name = body
        .name
        .as_deref()
        .map(|name| clean_name("name", name))
        .transpose()?;
    let description = clean_description("description", body.description.as_deref().unwrap_or(""))?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    let (_, kind) = super::placed(&mut tx, parent).await?;
    if kind != NodeKind::Project {
        return Err(ApiError::not_found());
    }
    let made = make(
        &ctx,
        &mut tx,
        &principal,
        &New {
            kind: "topic",
            parent: Some(parent),
            name: name.as_deref().unwrap_or(NEW_TOPIC),
            name_by_person: name.is_some(),
            description: &description,
            org: None,
            access: body.access,
            invitation: None,
        },
    )
    .await?;
    let thread = match body.first_thread.as_ref() {
        Some(first) => Some(
            crate::routes::threads::make::create_in(&ctx, &mut tx, &principal, made.id, first)
                .await?,
        ),
        None => None,
    };
    let node = shown(&ctx, &mut tx, &principal, made.id).await?;
    tx.commit().await.map_err(DbError::from)?;
    match node {
        Node::Topic(topic) => {
            if let Some(thread) = thread {
                if body
                    .first_thread
                    .as_ref()
                    .is_some_and(|first| first.first_input.is_some())
                {
                    ctx.app.engine.wake(thread.base.id);
                }
                ctx.app.engine.kick();
                Ok(TopicCreateReply::WithThread(TopicWithThread {
                    topic,
                    thread,
                }))
            } else {
                Ok(TopicCreateReply::Topic(topic))
            }
        }
        _ => Err(ApiError::internal("a topic was made and is not one")),
    }
}
