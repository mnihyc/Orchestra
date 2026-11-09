//! Who reaches into an environment, and by what, over HTTP: its owners, the people its own page lets in by managing it or by one of its four doors, and those who may talk to an agent that runs there.
//!
//! The request is for anybody who holds something on the environment. Everyone who can reach into
//! it is listed with each way, however the thing that lets them in is placed: whatever topics and
//! orgs the threads belong to, and whether or not the reader can see those threads. Who reaches only
//! through threads the reader may not view is counted, and not named.

use super::reach_operation;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::Registry;

use crate::routes::access::page::NameBounds;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::environment as api;

use orch_api_contract::environment::EnvironmentReach;

use orch_api_contract::environment::ReachPagePermission;

use orch_api_contract::environment::ReachPerson;

use orch_api_contract::environment::ReachThreadPermission;

use orch_api_contract::environment::ReachThrough;

use orch_authz::environments;

use orch_authz::environments::Reaching;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::resources;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use std::collections::BTreeSet;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(api::EnvironmentReachGet, operation, get);
}

struct PreparedReach {
    reach: environments::Reach,
    names: NameBounds,
}

fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = reach_operation::root(ctx, conn, caller).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let environment = ctx.id("id")?;
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        };
        let holding = match resources::effective(conn, &principal, object, Policy::default()).await
        {
            Ok(effective) => effective.owner || effective.administers || !effective.held.is_empty(),
            Err(orch_authz::Error::NotFound(_)) => false,
            Err(error) => {
                return Err(reach_operation::error(
                    caller,
                    refusals::problem(conn, error).await,
                ));
            }
        };
        if !holding {
            return Err(ApiError::not_found());
        }
        let mut reach = environments::reach(conn, &principal, environment, Policy::default())
            .await
            .map_err(ApiError::internal)?;
        let ids: BTreeSet<Id> = reach
            .people
            .iter()
            .flat_map(|person| &person.through)
            .filter_map(|way| match way {
                Reaching::Thread(id) => Some(*id),
                _ => None,
            })
            .collect();
        let selected =
            reach_operation::threads(conn, caller, &mut plan, "reach_threads", &ids).await?;
        for person in &mut reach.people {
            person.through.retain(|way| match way {
                Reaching::Thread(id) => selected.contains(id),
                _ => true,
            });
        }
        // The existing hidden count belongs to this Environment. Scope-omitted ways neither
        // expose an otherwise absent person nor inflate that ordinary unreadable aggregate.
        reach.people.retain(|person| !person.through.is_empty());
        let names = NameBounds {
            limited: caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some()),
            nodes: selected,
            ..Default::default()
        };
        Ok(plan.with_data(PreparedReach { reach, names }))
    })
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
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

async fn snapshot(ctx: Ctx) -> ApiResult<EnvironmentReach> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<PreparedReach>()?;
    let reach = &prepared.reach;
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
    let refs = prepared.names.refs(&mut conn, &principal, &threads).await?;
    let mut people: Vec<ReachPerson> = Vec::with_capacity(reach.people.len());
    for person in &reach.people {
        let Some(user) = users.get(&person.user).cloned() else {
            continue;
        };
        let through = person
            .through
            .iter()
            .filter_map(|way| match way {
                Reaching::Owner => Some(ReachThrough::Owner),
                Reaching::SourceOwner => Some(ReachThrough::SourceOwner),
                Reaching::Page(permission) => {
                    page_permission(*permission).map(|permission| ReachThrough::Page { permission })
                }
                Reaching::Thread(thread) => {
                    refs.get(thread)
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
    let shown = EnvironmentReach {
        people,
        hidden: reach.hidden,
    };
    conn.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
