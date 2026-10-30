//! What a thread's page shows of the environment it runs in: how many people read its files whatever the page says, and what sharing the thread offers beside it (looking in, and for whoever sets the thread up the Manage of its files and its shells).
//!
//! A thread's page holds none of the environment's doors, so what is shown beside it is read from the
//! environment's page: the environment with how many people look into it and how many other threads
//! work there, and for each thing that goes with the thread whether the people the thread names have
//! it, and whether the caller may give it there. A thread that runs in no environment shows neither.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::resources::view::environment_refs;
use crate::thread_access::AuthzAccess;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::{
    AccessChoice, Dependency, DependencyObject, DependencyOpen, PageReaches, PermissionPage,
};
use orch_authz::depends::Open;
use orch_authz::doors::{self, Wants};
use orch_authz::{Policy, Principal, environments};
use orch_db::Conn;
use orch_db::queries::environments::workspace as environment_rows;
use orch_types::Id;

/// The permission of the environment that a dependency says it needs: the choice of looking in, or
/// the row of one door.
fn wanted(wants: Wants) -> String {
    match wants {
        Wants::Looking => Permission::EnvironmentView.as_str().to_owned(),
        Wants::Door(row) => row.as_str().to_owned(),
    }
}

/// Fills in what the page of the thread `thread` shows of its environment, for the caller.
pub async fn show(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    policy: Policy,
    page: &mut PermissionPage,
) -> ApiResult<()> {
    let Some(environment) = environment_rows::thread_place(conn, thread)
        .await?
        .and_then(|place| place.environment_id)
    else {
        return Ok(());
    };
    let policies = AuthzAccess::policies(conn, environment)
        .await
        .map_err(ApiError::internal)?;
    let Some(reference) = environment_refs(conn, &[environment], &[], principal.now())
        .await?
        .into_iter()
        .next()
        .map(|(_, reference)| reference)
    else {
        return Ok(());
    };
    if let Some(reaches) = environments::reaches(conn, principal, thread, policy)
        .await
        .map_err(ApiError::internal)?
    {
        page.reaches = Some(PageReaches {
            environment: reference,
            people: reaches.people,
            threads: reaches.threads,
        });
    }
    let found = doors::of_thread(conn, principal, thread, environment, &policies)
        .await
        .map_err(ApiError::internal)?;
    let owners: Vec<Id> = found.iter().filter_map(|each| each.owner).collect();
    let owners = refusals::user_refs(conn, &owners).await?;
    for each in found {
        page.depends.push(Dependency {
            for_permission: each.for_permission.as_str().to_owned(),
            object: DependencyObject {
                kind: each.object.kind.as_str().to_owned(),
                id: each.object.id,
                name: each.name.clone(),
                owner: each.owner.and_then(|owner| owners.get(&owner).cloned()),
            },
            permission: wanted(each.wants),
            choice: match each.wants {
                Wants::Looking => Some(AccessChoice::FullView),
                Wants::Door(_) => None,
            },
            options: None,
            open: match each.open {
                Open::All => DependencyOpen::All,
                Open::Part => DependencyOpen::Part,
                Open::Nobody => DependencyOpen::Nobody,
            },
            through: None,
            can_give: each.can_give,
        });
    }
    Ok(())
}
