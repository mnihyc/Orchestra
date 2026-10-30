//! Who reaches into an environment, and by what, over HTTP: its owners, the people its own page lets in by managing it or by one of its four doors, and those who may talk to an agent that runs there.
//!
//! The request is for anybody who holds something on the environment. Everyone who can reach into
//! it is listed with each way, however the thing that lets them in is placed: whatever topics and
//! orgs the threads belong to, and whether or not the reader can see those threads. Who reaches only
//! through threads the reader may not view is counted, and not named.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::view as node_view;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::environment::{
    self as api, EnvironmentReach, ReachPagePermission, ReachPerson, ReachThreadPermission,
    ReachThrough,
};
use orch_authz::environments::{self, Reaching};
use orch_authz::{Policy, resources};
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(api::EnvironmentReachGet, get);
}

/// The permission of an environment's own page that lets a person in, as the contract names it.
fn page_permission(permission: Permission) -> Option<ReachPagePermission> {
    Some(match permission {
        Permission::EnvironmentManage => ReachPagePermission::EnvironmentManage,
        Permission::FilesManage => ReachPagePermission::FilesManage,
        Permission::FilesView => ReachPagePermission::FilesView,
        Permission::TerminalManage => ReachPagePermission::TerminalManage,
        Permission::TerminalView => ReachPagePermission::TerminalView,
        Permission::BrowserManage => ReachPagePermission::BrowserManage,
        Permission::BrowserView => ReachPagePermission::BrowserView,
        Permission::VncManage => ReachPagePermission::VncManage,
        Permission::VncView => ReachPagePermission::VncView,
        _ => return None,
    })
}

/// Everybody who can reach into the environment now, and how, for whoever holds any permission on it.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<EnvironmentReach> {
    let environment = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let object = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    let holding = match resources::effective(&mut conn, &principal, object, Policy::default()).await
    {
        Ok(effective) => effective.owner || effective.administers || !effective.held.is_empty(),
        Err(orch_authz::Error::NotFound(_)) => false,
        Err(error) => return Err(refusals::problem(&mut conn, error).await),
    };
    if !holding {
        return Err(ApiError::not_found());
    }
    let reach = environments::reach(&mut conn, &principal, environment, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let users: Vec<Id> = reach.people.iter().map(|person| person.user).collect();
    let users = refusals::user_refs(&mut conn, &users).await?;
    let mut threads: Vec<Id> = reach
        .people
        .iter()
        .flat_map(|person| person.through.iter())
        .filter_map(|way| match way {
            Reaching::Thread(thread) => Some(*thread),
            _ => None,
        })
        .collect();
    threads.sort_unstable();
    threads.dedup();
    let refs = node_view::refs(&mut conn, &principal, &threads, Policy::default()).await?;
    let mut people: Vec<ReachPerson> = Vec::with_capacity(reach.people.len());
    for person in reach.people {
        let Some(user) = users.get(&person.user).cloned() else {
            continue;
        };
        let through = person
            .through
            .into_iter()
            .filter_map(|way| match way {
                Reaching::Owner => Some(ReachThrough::Owner),
                Reaching::SourceOwner => Some(ReachThrough::SourceOwner),
                Reaching::Page(permission) => {
                    page_permission(permission).map(|permission| ReachThrough::Page { permission })
                }
                Reaching::Thread(thread) => {
                    refs.get(&thread)
                        .cloned()
                        .map(|thread| ReachThrough::Thread {
                            thread,
                            permission: ReachThreadPermission::ThreadInteract,
                        })
                }
            })
            .collect();
        people.push(ReachPerson { user, through });
    }
    // The people are in the order of their names: who owns it among them, and who may do what, is
    // read from each one's ways.
    people.sort_by(|one, other| {
        (&one.user.display_name, &one.user.username)
            .cmp(&(&other.user.display_name, &other.user.username))
    });
    Ok(EnvironmentReach {
        people,
        hidden: reach.hidden,
    })
}
