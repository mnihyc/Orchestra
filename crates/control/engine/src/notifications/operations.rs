//! Reviewed operation audiences and a single renewable notification for each affected person.

use crate::{EngineError, loss::managers};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_authz::Principal;
use orch_db::{
    Conn,
    queries::{
        environments,
        notifications::{operations as rows, people::NewNotification},
    },
};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// Resolves the operation's editable notification default through the normal scope and locks.
pub async fn operation_default(
    conn: &mut Conn,
    place: orch_settings::Place,
    blocking: bool,
) -> Result<bool, EngineError> {
    let choice = orch_settings::get::<String>(
        conn,
        place,
        orch_settings_contract::registry::keys::NOTIFY_OPERATIONS,
    )
    .await
    .map_err(EngineError::from)?;
    match choice.as_deref() {
        Some("always") => Ok(true),
        Some("never") => Ok(false),
        Some("when_blocking") | None => Ok(blocking),
        _ => Err(EngineError::unreadable(
            "the operation notification default is not recognized",
        )),
    }
}

/// What one person needs to know of an operation, with no inaccessible thread names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OperationRecipient {
    /// The affected person.
    pub user: Id,
    /// Environments they own or manage.
    pub environments: BTreeSet<Id>,
    /// Threads they own or may drive in these environments.
    pub threads: BTreeSet<Id>,
    /// Topic defaults they may replace.
    pub defaults: BTreeSet<Id>,
    /// Other resources they manage when the operation changes their availability.
    #[serde(default)]
    pub resources: BTreeSet<(String, Id)>,
    /// Providers they own or manage.
    pub providers: BTreeSet<Id>,
}

/// A confirmed notification choice, retained on every child job of the operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationOperation {
    /// The operation that groups all independent members.
    pub id: Id,
    /// The visible operation kind, such as lost, recover or host_delete.
    pub kind: String,
    /// The safe source name already visible through the affected resource.
    pub name: String,
    /// Whether the person explicitly selected notification.
    pub notify: bool,
    /// The destination of the grouped item.
    pub path: String,
    /// The resource named by the operation, for a recipient with no environment or provider.
    pub subject: Option<(String, Id)>,
    /// Each person appears once, however many resources named them.
    pub recipients: Vec<OperationRecipient>,
}

/// Current owners, managers and thread owners, expanded under the operation's authority snapshot.
pub async fn operation_audience(
    conn: &mut Conn,
    principal: &Principal,
    environment_ids: &[Id],
    providers: &[Id],
) -> Result<Vec<OperationRecipient>, EngineError> {
    let mut recipients: BTreeMap<Id, OperationRecipient> = BTreeMap::new();
    let defaults = orch_db::queries::loss::default_topics(conn, environment_ids).await?;
    for id in environment_ids {
        let Some(environment) = environments::hosting::environment(conn, *id).await? else {
            continue;
        };
        let mut people = managers(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: *id,
            },
            Permission::EnvironmentManage,
        )
        .await?;
        people.push(environment.owner_id);
        let threads = environments::threads_in(conn, &[*id]).await?;
        for user in people {
            let entry = recipients
                .entry(user)
                .or_insert_with(|| OperationRecipient {
                    user,
                    ..Default::default()
                });
            entry.environments.insert(*id);
            entry
                .threads
                .extend(threads.iter().map(|thread| thread.thread_id));
        }
        for thread in threads {
            let entry = recipients
                .entry(thread.owner_id)
                .or_insert_with(|| OperationRecipient {
                    user: thread.owner_id,
                    ..Default::default()
                });
            entry.threads.insert(thread.thread_id);
        }
        let topics = defaults
            .iter()
            .filter(|(environment, _)| environment == id)
            .map(|(_, topic)| *topic)
            .collect::<Vec<_>>();
        for topic in topics {
            let graph = orch_authz::engine::Graph::load(conn, &[topic]).await?;
            let Some(site) = graph.site_of(topic) else {
                continue;
            };
            let permission = Permission::TopicManage;
            let mut asked = vec![permission];
            asked.extend(permission.held_through());
            let frame = orch_authz::engine::Frame::load(
                conn,
                principal,
                orch_authz::Policy::default(),
                graph,
                std::slice::from_ref(&site),
                &asked,
            )
            .await?;
            for user in
                orch_authz::holders::who_holds_for_notifications(conn, &frame, &site, permission)
                    .await?
            {
                let entry = recipients
                    .entry(user)
                    .or_insert_with(|| OperationRecipient {
                        user,
                        ..Default::default()
                    });
                entry.defaults.insert(topic);
            }
        }
    }
    for provider in providers {
        let bound:Vec<(Id,Id)>=sqlx::query_as("SELECT t.node_id,n.owner_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.provider_id=$1 AND n.deleted_at IS NULL ORDER BY t.node_id").bind(provider).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        for (thread, user) in &bound {
            recipients
                .entry(*user)
                .or_insert_with(|| OperationRecipient {
                    user: *user,
                    ..Default::default()
                })
                .threads
                .insert(*thread);
        }
        for user in managers(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: *provider,
            },
            Permission::ProviderManage,
        )
        .await?
        {
            let entry = recipients
                .entry(user)
                .or_insert_with(|| OperationRecipient {
                    user,
                    ..Default::default()
                });
            entry.providers.insert(*provider);
            entry
                .threads
                .extend(bound.iter().map(|(thread, _)| *thread));
        }
    }
    let users = recipients.keys().copied().collect::<Vec<_>>();
    let humans = rows::receivers(conn, &users, "environment.attention")
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    Ok(recipients
        .into_values()
        .filter(|entry| humans.contains(&entry.user))
        .collect())
}

/// A selected-thread batch tells only its thread owners and current managers, rather than every neighbor.
pub async fn operation_thread_audience(
    conn: &mut Conn,
    principal: &Principal,
    threads: &[Id],
) -> Result<Vec<OperationRecipient>, EngineError> {
    let graph = orch_authz::engine::Graph::load(conn, threads).await?;
    let sites = threads
        .iter()
        .filter_map(|id| graph.site_of(*id))
        .collect::<Vec<_>>();
    let permission = Permission::ThreadManage;
    let mut asked = vec![permission];
    asked.extend(permission.held_through());
    let frame = orch_authz::engine::Frame::load(
        conn,
        principal,
        orch_authz::Policy::default(),
        graph,
        &sites,
        &asked,
    )
    .await?;
    let mut people = BTreeMap::new();
    for site in sites {
        let mut users =
            orch_authz::holders::who_holds_for_notifications(conn, &frame, &site, permission)
                .await?;
        if let Some(row) = orch_db::queries::threads::thread(conn, site.id).await? {
            users.push(row.owner_id);
        }
        for user in users {
            people
                .entry(user)
                .or_insert_with(|| OperationRecipient {
                    user,
                    ..Default::default()
                })
                .threads
                .insert(site.id);
        }
    }
    let ids = people.keys().copied().collect::<Vec<_>>();
    let receivers = rows::receivers(conn, &ids, "environment.attention")
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    Ok(people
        .into_values()
        .filter(|person| receivers.contains(&person.user))
        .collect())
}

/// Adds managers of affected infrastructure, mounts and retained native imports, without duplicate people.
pub async fn extend_operation_audience(
    conn: &mut Conn,
    principal: &Principal,
    people: &mut Vec<OperationRecipient>,
    resources: &[(ObjectRef, Permission)],
) -> Result<(), EngineError> {
    let mut all = people
        .drain(..)
        .map(|person| (person.user, person))
        .collect::<BTreeMap<_, _>>();
    for (object, permission) in resources {
        for user in managers(conn, principal, *object, *permission).await? {
            all.entry(user)
                .or_insert_with(|| OperationRecipient {
                    user,
                    ..Default::default()
                })
                .resources
                .insert((object.kind.as_str().into(), object.id));
        }
    }
    let ids = all.keys().copied().collect::<Vec<_>>();
    let actual = rows::receivers(conn, &ids, "environment.attention")
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    people.extend(
        all.into_values()
            .filter(|person| actual.contains(&person.user)),
    );
    Ok(())
}

/// Publishes the explicit choice once for each person, renewing the prior item for that resource set.
pub async fn notify_operation(
    conn: &mut Conn,
    operation: &NotificationOperation,
    state: &str,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    if !operation.notify {
        return Ok(());
    }
    let receivers = rows::receivers(
        conn,
        &operation
            .recipients
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
        "environment.attention",
    )
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    let unavailable_defaults = if operation.kind == "host_delete" {
        let environments:Vec<Id>=sqlx::query_scalar("SELECT DISTINCT e.id FROM settings s JOIN environments e ON e.id=(s.value #>> '{}')::uuid WHERE s.key='env.default_id' AND (e.lost_at IS NOT NULL OR e.status IN ('lost','removed') OR e.deleted_at IS NOT NULL OR e.remove_at IS NOT NULL)").fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        orch_db::queries::loss::default_topics(conn, &environments)
            .await?
            .into_iter()
            .map(|(_, topic)| topic)
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    for person in operation
        .recipients
        .iter()
        .filter(|person| receivers.contains(&person.user))
    {
        let mut resources = person
            .environments
            .iter()
            .map(|id| ("environment".into(), *id))
            .chain(person.providers.iter().map(|id| ("provider".into(), *id)))
            .collect::<Vec<_>>();
        resources.extend(person.resources.iter().cloned());
        if resources.is_empty() {
            resources.extend(person.defaults.iter().map(|id| ("topic".into(), *id)));
        }
        if resources.is_empty() {
            resources.extend(person.threads.iter().map(|id| ("thread".into(), *id)));
        }
        if resources.is_empty() {
            resources.extend(operation.subject.clone());
        }
        let key = rows::group_key("resources", &resources);
        let path = person
            .environments
            .iter()
            .next()
            .map(|id| format!("/resources/environments/{id}"))
            .or_else(|| {
                person
                    .defaults
                    .iter()
                    .next()
                    .map(|id| format!("/topic/{id}"))
            })
            .or_else(|| {
                person
                    .providers
                    .iter()
                    .next()
                    .map(|id| format!("/resources/providers/{id}"))
            })
            .or_else(|| {
                person
                    .threads
                    .iter()
                    .next()
                    .map(|id| format!("/thread/{id}"))
            })
            .or_else(|| {
                person.resources.iter().next().map(|(kind, id)| {
                    format!(
                        "/resources/{}/{}",
                        match kind.as_str() {
                            "mcp_server" => "mcp",
                            "mount" => "mounts",
                            "host" => "hosts",
                            "machine" => "machines",
                            _ => "environments",
                        },
                        id
                    )
                })
            })
            .unwrap_or_else(|| operation.path.clone());
        let default_lost = !person.defaults.is_empty()
            && (operation.kind == "lost"
                || (operation.kind == "host_delete"
                    && person
                        .defaults
                        .iter()
                        .any(|topic| unavailable_defaults.contains(topic))));
        // A resource manager may know the work count without being able to read its threads.
        let data = json!({"operation_kind":operation.kind,"name":operation.name,"state":state,"environment_ids":person.environments,"thread_count":person.threads.len(),"default_topics":person.defaults,"provider_ids":person.providers,"resource_ids":person.resources,"path":path,"loss":operation.kind=="lost","default_lost":default_lost,"at":Time::new(at)});
        rows::publish(
            conn,
            operation.id,
            &NewNotification {
                user_id: person.user,
                kind: "environment.attention",
                node_id: None,
                thread_id: None,
                approval_id: None,
                chat_id: None,
                title: &format!("{}: {}", operation.name, operation.kind),
                body: state,
                data: &data,
                dedupe_key: &key,
                at,
            },
        )
        .await?;
    }
    Ok(())
}

/// Retains the operation choice on independent children so their ordinary hooks cannot flood the Inbox.
pub async fn attach_operation(
    conn: &mut Conn,
    ids: &[Id],
    operation: &NotificationOperation,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    rows::save(conn, operation.id, &serde_json::to_value(operation)?, now).await?;
    for id in ids {
        orch_db::queries::jobs::merge_plan(
            conn,
            *id,
            &json!({"notification_operation":operation.id}),
            now,
        )
        .await?;
    }
    Ok(())
}

/// One operation outcome replaces per-job finish notices and is written when its final child ends.
pub(crate) async fn operation_job_finished(
    conn: &mut Conn,
    job: &orch_db::queries::jobs::JobRow,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, EngineError> {
    let Some(id) = job
        .plan
        .get("notification_operation")
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<Id>().ok())
    else {
        return Ok(false);
    };
    let value = rows::lock(conn, id).await?.ok_or_else(|| {
        EngineError::unreadable("the operation's accepted notification choice is missing")
    })?;
    let operation: NotificationOperation = serde_json::from_value(value)?;
    let open:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE plan->>'notification_operation'=$1 AND state IN ('queued','running'))").bind(operation.id.to_string()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !open {
        let failed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE plan->>'notification_operation'=$1 AND state IN ('failed','lost','cancelled'))").bind(operation.id.to_string()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        notify_operation(conn, &operation, if failed { "failed" } else { "done" }, at).await?;
    }
    Ok(true)
}
