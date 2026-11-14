//! Current recipient disclosure for the immutable choice of a grouped resource operation.

use crate::{Error, Loaded, SourceRead};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_authz::{Caller, Policy, Principal};
use orch_db::{
    Conn,
    queries::notifications::{operations, outbox::Snapshot, routes},
};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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

impl OperationRecipient {
    /// The accepted resource set also identifies successive Inbox renewals.
    pub fn subjects(&self, fallback: &Option<(String, Id)>) -> Vec<(String, Id)> {
        let mut resources = self
            .environments
            .iter()
            .map(|id| ("environment".into(), *id))
            .chain(self.providers.iter().map(|id| ("provider".into(), *id)))
            .chain(self.resources.iter().cloned())
            .collect::<Vec<_>>();
        if resources.is_empty() {
            resources.extend(self.defaults.iter().map(|id| ("topic".into(), *id)));
        }
        if resources.is_empty() {
            resources.extend(self.threads.iter().map(|id| ("thread".into(), *id)));
        }
        if resources.is_empty() {
            resources.extend(fallback.clone());
        }
        resources.sort();
        resources.dedup();
        resources
    }
}

/// Names and root scopes admitted by the recipient's ordinary current resource pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disclosure {
    /// The first actually readable resource, never the operation initiator's private name.
    pub name: String,
    /// Its native page.
    pub path: String,
    /// Every root represented by this indivisible grouped message, including system resources.
    pub roots: BTreeSet<Option<Id>>,
}

/// A reviewed audience is a candidate list, not retained disclosure authority.
/// Resource managers may know aggregate affected-work counts without reading each thread.
pub async fn disclosure(
    conn: &mut Conn,
    operation: &NotificationOperation,
    person: &OperationRecipient,
    at: Time,
) -> Result<Option<Disclosure>, orch_authz::Error> {
    let principal = match Principal::load(conn, Caller::session(person.user), at).await {
        Ok(principal) => principal,
        Err(orch_authz::Error::NoSuchUser(_) | orch_authz::Error::Suspended { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let subjects = person.subjects(&operation.subject);
    if subjects.is_empty() {
        return Ok(None);
    }
    let mut named = subjects.clone();
    named.extend(person.defaults.iter().map(|id| ("topic".into(), *id)));
    named.sort();
    named.dedup();
    let mut result: Option<Disclosure> = None;
    for (kind, id) in named {
        let Ok(kind) = kind.parse::<ObjectKind>() else {
            return Ok(None);
        };
        let object = ObjectRef { kind, id };
        let found = if kind.node().is_some() {
            let graph = orch_authz::engine::Graph::load(conn, &[id]).await?;
            let Some(site) = graph.site_of(id) else {
                return Ok(None);
            };
            let Some(node) = graph.get(id) else {
                return Ok(None);
            };
            let name = node.name.clone();
            let held =
                orch_authz::things::effective_at(conn, &principal, graph, &site, Policy::default())
                    .await?;
            (held, name)
        } else {
            // Removing use keeps the resource's retained page and its normal disclosure door.
            let (graph, resource, site) = match orch_authz::resources::site(conn, object).await {
                Ok(found) => found,
                Err(orch_authz::Error::NotFound(_)) => return Ok(None),
                Err(error) => return Err(error),
            };
            let held =
                orch_authz::things::effective_at(conn, &principal, graph, &site, Policy::default())
                    .await?;
            (held, resource.name)
        };
        if found.0.held.is_empty() {
            return Ok(None);
        }
        let path = if kind.node().is_some() {
            format!("/{}/{id}", kind.as_str())
        } else {
            let collection = match kind.as_str() {
                "provider" => "providers",
                "environment" => "environments",
                "host" => "hosts",
                "machine" => "machines",
                "mcp_server" => "mcp",
                "mount" => "mounts",
                _ => return Ok(None),
            };
            format!("/resources/{collection}/{id}")
        };
        let entry = result.get_or_insert_with(|| Disclosure {
            name: found.1,
            path,
            roots: BTreeSet::new(),
        });
        entry.roots.insert(found.0.root);
    }
    Ok(result)
}

/// Private grouped source retained until each notifier call.
pub struct Opened {
    operation: NotificationOperation,
    person: OperationRecipient,
    disclosure: Disclosure,
    snapshot: Snapshot,
    channel: Option<Id>,
}

pub(crate) async fn load(
    conn: &mut Conn,
    clock: &dyn Clock,
    receiver: Id,
    snapshot: &Snapshot,
    json: serde_json::Value,
) -> Result<Loaded, Error> {
    let Ok(operation) = serde_json::from_value::<NotificationOperation>(json) else {
        return Ok(Loaded::Closed);
    };
    let Some(person) = operation
        .recipients
        .iter()
        .find(|person| person.user == receiver)
        .cloned()
    else {
        return Ok(Loaded::Closed);
    };
    let Some(disclosure) = disclosure(conn, &operation, &person, clock.time()).await? else {
        return Ok(Loaded::Closed);
    };
    if snapshot.data.get("operation_id") != Some(&serde_json::to_value(operation.id)?)
        || snapshot
            .data
            .get("operation_kind")
            .and_then(serde_json::Value::as_str)
            != Some(operation.kind.as_str())
        || snapshot
            .data
            .get("operation_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
        || snapshot
            .data
            .get("name")
            .and_then(serde_json::Value::as_str)
            != Some(disclosure.name.as_str())
        || snapshot
            .data
            .get("path")
            .and_then(serde_json::Value::as_str)
            != Some(disclosure.path.as_str())
        || snapshot.data.get("operation_roots") != Some(&serde_json::to_value(&disclosure.roots)?)
    {
        return Ok(Loaded::Closed);
    }
    let opened = Opened {
        operation,
        person,
        disclosure,
        snapshot: snapshot.clone(),
        channel: None,
    };
    Ok(if opened.check(conn, clock).await?.is_some() {
        Loaded::Source(Box::new(crate::Opened::Resource(Box::new(opened))))
    } else {
        Loaded::Closed
    })
}

impl Opened {
    pub(crate) fn through(&mut self, channel: Id) {
        self.channel = Some(channel);
    }
    pub(crate) fn inbox(&self) -> Id {
        self.snapshot.id.unwrap_or_default()
    }
    pub(crate) async fn check(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<SourceRead>, Error> {
        let Some(saved) = operations::lock(conn, self.operation.id).await? else {
            return Ok(None);
        };
        if serde_json::from_value::<NotificationOperation>(saved)
            .ok()
            .as_ref()
            != Some(&self.operation)
            || !self.operation.notify
        {
            return Ok(None);
        }
        let Some(inbox) = self.snapshot.id else {
            return Ok(None);
        };
        let current = routes::unread_rows_of(conn, self.person.user, &[inbox]).await?;
        let Some(expected) = crate::normalized(&self.snapshot) else {
            return Ok(None);
        };
        if current
            .first()
            .and_then(|row| crate::normalized(&row.snapshot()))
            != Some(expected)
        {
            return Ok(None);
        }
        if disclosure(conn, &self.operation, &self.person, clock.time())
            .await?
            .as_ref()
            != Some(&self.disclosure)
        {
            return Ok(None);
        }
        if let Some(channel) = self.channel {
            let shared =
                orch_settings::get::<bool>(conn, orch_settings::Place::System, "mail.shared")
                    .await?
                    .unwrap_or(true);
            if !routes::covers_roots(
                conn,
                channel,
                &self.disclosure.roots.iter().copied().collect::<Vec<_>>(),
                shared,
            )
            .await?
            {
                return Ok(None);
            }
        }
        // This is a recipient observation of a native completed operation, not sender mutation authority.
        Ok(Some(SourceRead {
            caller: Caller::session(self.person.user),
            until: None,
        }))
    }
}
