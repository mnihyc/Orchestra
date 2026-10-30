//! The permission page of a resource, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice.
//!
//! The page of a node is read by the routes of the hierarchy; the requests are the same for a
//! resource, and the page machinery is the access engine's for both. What a resource adds is its home:
//! its page follows the defaults of the org it belongs to, and the two policies that govern a change
//! (whether its owners may name somebody from outside its root org, and whether the subscription it
//! may be is kept to its first owner) are the settings of that org.

use crate::middleware::authorization::{self, refusals, resource_kind_of};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page::permission_page;
use crate::routes::resources::providers::home_policies;
use crate::sign_in::origin;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ResourceKind;
use orch_api_contract::access as api;
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Policy, Principal, resources};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};

/// The resource a `{object}/{id}` path names.
fn named(ctx: &Ctx) -> ApiResult<(ResourceKind, Id)> {
    let kind = resource_kind_of(ctx.param("object")?).ok_or_else(ApiError::not_found)?;
    Ok((kind, ctx.id("id")?))
}

/// The policies that govern a change of the page of a resource at its home: those of sharing, which
/// the access engine asks of any page (whether its owners may name somebody from outside its root
/// org, and whether somebody who holds it without managing its page may share it), and the
/// subscription it may be, if it is one, which its home may keep to its first owner.
async fn policy_of(
    conn: &mut Conn,
    home: Option<Id>,
    kind: ResourceKind,
) -> ApiResult<(Policy, ResourcePolicy)> {
    let sharing = crate::routes::access::subject::policy_at_place(conn, home).await?;
    let shared_subscriptions = if let Some(home) = home
        && kind == ResourceKind::Provider
    {
        home_policies(conn, home).await?.subscription_sharing
    } else {
        None
    };
    Ok((
        sharing,
        ResourcePolicy {
            owners_name_outsiders: sharing.owners_name_outsiders,
            shared_subscriptions,
        },
    ))
}

async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    object: ObjectRef,
    root: Option<Id>,
    policy: Policy,
) -> ApiResult<api::PermissionPage> {
    let page = pages::read(conn, principal, Target::Resource(object), policy)
        .await
        .map_err(ApiError::internal)?;
    permission_page(conn, principal, &page, root, Time::new(ctx.now)).await
}

/// Reads the page of a resource: whoever holds any permission on it.
pub async fn get(ctx: &Ctx) -> ApiResult<api::PermissionPage> {
    let (kind, id) = named(ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let object = ObjectRef {
        kind: kind.into(),
        id,
    };
    let (_, resource, _) =
        resources::site(&mut conn, object)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
    if resource.deleted {
        return Err(ApiError::not_found());
    }
    let (sharing, _) = policy_of(&mut conn, resource.home, kind).await?;
    crate::routes::access::subject::require_resource_reader(&mut conn, &principal, object, sharing)
        .await?;
    read(ctx, &mut conn, &principal, object, resource.root, sharing).await
}

/// Changes the page of a resource, as a whole or not at all: the one change that names owners and
/// holders at a choice. Who may is the engine's, and it was asked before this ran.
pub async fn update(ctx: &Ctx, body: &api::PageChange) -> ApiResult<api::PermissionPage> {
    let (kind, id) = named(ctx)?;
    let (change, people) = crate::routes::notifications::person::page_change(body)?;
    let (ctx, mut tx, principal) = super::begin_mutation(ctx).await?;
    let object = ObjectRef {
        kind: kind.into(),
        id,
    };
    let (_, resource, _) = resources::site(&mut tx, object)
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    if resource.deleted {
        return Err(ApiError::not_found());
    }
    let (sharing, policy) = policy_of(&mut tx, resource.home, kind).await?;
    if crate::routes::notifications::person::replay_page(&ctx, &mut tx, &principal, body).await? {
        crate::routes::access::subject::require_page(
            &mut tx,
            &principal,
            orch_authz::things::Thing::Other(object),
            sharing,
        )
        .await?;
        let page = read(&ctx, &mut tx, &principal, object, resource.root, sharing).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(page);
    }
    match pages::change_resource_under(
        &mut tx,
        &principal,
        object,
        &change,
        sharing,
        &policy,
        &origin(&ctx),
    )
    .await
    {
        Ok(_) => {}
        Err(ChangeError::Stale { current }) => {
            let page = read(&ctx, &mut tx, &principal, object, resource.root, sharing).await?;
            return Err(
                refusals::change_failed(&mut tx, ChangeError::Stale { current })
                    .await
                    .with("current", page),
            );
        }
        Err(ChangeError::SubscriptionNotShared) => {
            return Err(subscription_not_shared(&resource));
        }
        Err(error) => return Err(refusals::change_failed(&mut tx, error).await),
    }
    if !people.is_empty() {
        crate::routes::notifications::person::tell(
            &ctx,
            &mut tx,
            &principal,
            ctx.param("object")?,
            id,
            &people,
            body.notify_anchor,
            None,
        )
        .await?;
    }
    let page = read(&ctx, &mut tx, &principal, object, resource.root, sharing).await?;
    crate::routes::notifications::person::keep_page(&ctx, &mut tx, &principal, body).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}

/// The refusal to name anybody in the Use row of a subscription that its home keeps to its first
/// owner, with the harness and the place that says so.
fn subscription_not_shared(resource: &resources::Resource) -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ProviderSubscriptionNotShared,
        "a subscription of this harness stays with its first owner, so nobody else is named in its Use row",
    )
    .with("harness_kind", resource.subscription.clone())
    .with("scope", resource.home)
}
