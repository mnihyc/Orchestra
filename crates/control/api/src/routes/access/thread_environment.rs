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
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::access::{
    AccessChoice, Dependency, DependencyObject, DependencyOpen, PageReaches, PermissionPage,
};
use orch_authz::depends::Open;
use orch_authz::doors::{self, Wants};
use orch_authz::{Policy, Principal, environments};
use orch_db::Conn;
use orch_db::queries::environments::workspace as environment_rows;
use orch_db::queries::resources;
use orch_types::Id;

/// The permission of the environment that a dependency says it needs: the choice of looking in, or
/// the row of one door.
fn wanted(wants: Wants) -> String {
    match wants {
        Wants::Looking => Permission::EnvironmentView.as_str().to_owned(),
        Wants::Door(row) => row.as_str().to_owned(),
    }
}

/// The ordinary environment reach and door decisions retained until their source is admitted.
/// No public environment reference is constructed during preparation.
#[derive(Clone)]
pub(crate) struct PreparedThreadEnvironment {
    environment: Id,
    reaches: Option<environments::Reaches>,
    doors: Vec<doors::Dependency>,
}

impl PreparedThreadEnvironment {
    /// The actual environment whose reference, reach counts and door information will be shown.
    pub(crate) fn objects(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        (self.reaches.is_some() || !self.doors.is_empty())
            .then_some(ObjectRef {
                kind: ObjectKind::Environment,
                id: self.environment,
            })
            .into_iter()
    }

    /// Environment doors have no separately named node provenance.
    pub(crate) fn nodes(&self) -> impl Iterator<Item = Id> {
        std::iter::empty()
    }
}

/// Fills in what the page of the thread shows of its environment, using the ordinary producer.
pub async fn show(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    policy: Policy,
    page: &mut PermissionPage,
) -> ApiResult<()> {
    if let Some(prepared) = prepare_thread(conn, principal, thread, policy).await? {
        project_thread(conn, principal, &prepared, page).await?;
    }
    Ok(())
}

/// Resolves the actual environment and the existing reach/door decisions without publishing its
/// reference. Page authority owns these decisions; no new EnvironmentView requirement is added.
pub(crate) async fn prepare_thread(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    policy: Policy,
) -> ApiResult<Option<PreparedThreadEnvironment>> {
    let Some(environment) = environment_rows::thread_place(conn, thread)
        .await?
        .and_then(|place| place.environment_id)
    else {
        return Ok(None);
    };
    let policies = AuthzAccess::policies(conn, environment)
        .await
        .map_err(ApiError::internal)?;
    // The original environment reference omits missing/deleted environments. Keep that absence
    // check before computing reach/doors, without constructing its public reference or counts.
    let present = resources::resource_facts(conn, &[environment])
        .await?
        .iter()
        .any(|row| row.id == environment && row.kind == "environment" && !row.deleted);
    if !present {
        return Ok(None);
    }
    let reaches = environments::reaches(conn, principal, thread, policy)
        .await
        .map_err(ApiError::internal)?;
    let doors = doors::of_thread(conn, principal, thread, environment, &policies)
        .await
        .map_err(ApiError::internal)?;
    Ok(Some(PreparedThreadEnvironment {
        environment,
        reaches,
        doors,
    }))
}

/// Publishes the existing full environment reference and prepared door information after the
/// caller has admitted that Environment source under the same operation.
pub(crate) async fn project_thread(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedThreadEnvironment,
    page: &mut PermissionPage,
) -> ApiResult<()> {
    if prepared.reaches.is_none() && prepared.doors.is_empty() {
        return Ok(());
    }
    let Some(reference) = environment_refs(conn, &[prepared.environment], &[], principal.now())
        .await?
        .into_iter()
        .next()
        .map(|(_, reference)| reference)
    else {
        return Ok(());
    };
    if let Some(reaches) = &prepared.reaches {
        page.reaches = Some(PageReaches {
            environment: reference,
            people: reaches.people,
            threads: reaches.threads,
        });
    }
    let found = &prepared.doors;

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
