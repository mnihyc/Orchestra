//! Batch document authority through the native person pages or the calling agent's own reach.

use super::{Actor, Directory, Entry, Error, Item, Standing};
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_authz::{Act, Policy, documents, engine::Graph, reach};
use orch_db::{Conn, queries::documents::containers};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use std::collections::HashMap;

pub(super) fn item(row: &containers::EntryRow) -> Item {
    if row.kind == "container" {
        Item::Container(row.id)
    } else {
        Item::Document(row.id)
    }
}

pub(super) fn project(row: &containers::EntryRow) -> Entry {
    Entry {
        item: item(row),
        parent: row
            .parent_container_id
            .map_or(Directory::NodeDocs(row.node_id), Directory::Container),
        node: row.node_id,
        root: row.org_id,
        basename: row.basename.clone(),
        title: row.title.clone(),
        revision: row.revision,
        owner: row.owner_id,
        deleted_at: row.deleted_at,
        deletion: row.deleted_in,
        files: row.files,
        versions: row.versions,
    }
}

pub(super) async fn directory(
    conn: &mut Conn,
    parent: Directory,
) -> Result<(Id, Id, Option<Id>), Error> {
    match parent {
        Directory::NodeDocs(node) => {
            let graph = Graph::load(conn, &[node]).await?;
            let site = graph.site_of(node).ok_or(Error::NotFound)?;
            Ok((node, site.root.ok_or(Error::NotFound)?, None))
        }
        Directory::Container(id) => {
            let row = containers::get(conn, id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(Error::NotFound)?;
            let mut graph = Graph::default();
            graph.extend_documents(conn, &[id]).await?;
            if graph.site_of(row.node_id).is_none() || graph.document_parent_deleted(id) {
                return Err(Error::NotFound);
            }
            Ok((row.node_id, row.org_id, Some(id)))
        }
    }
}

pub(super) async fn agent_reach(
    conn: &mut Conn,
    thread: Id,
    documents: Vec<documents::Document>,
    nodes: Vec<Id>,
    now: Time,
) -> Result<reach::Reach, Error> {
    let agent = reach::Agent::of(conn, thread)
        .await?
        .ok_or(Error::NotFound)?;
    let policy =
        orch_settings::get::<String>(conn, Place::Node(agent.topic), keys::SHARING_AGENT_REACH)
            .await
            .map_err(crate::EngineError::from)?
            .and_then(|word| reach::AgentReach::parse(&word))
            .unwrap_or_default();
    let document_policy = orch_settings::get::<String>(
        conn,
        Place::Node(agent.topic),
        keys::SHARING_AGENT_DOCUMENTS,
    )
    .await
    .map_err(crate::EngineError::from)?
    .and_then(|word| reach::DocumentPolicy::parse(&word))
    .unwrap_or_default();
    Ok(reach::Reach::load(
        conn,
        agent,
        policy,
        now,
        &reach::Things {
            nodes,
            documents,
            people_chats: Vec::new(),
        },
    )
    .await?
    .with_document_policy(document_policy))
}

pub(super) async fn standings(
    conn: &mut Conn,
    actor: Actor<'_>,
    entries: &[containers::EntryRow],
    now: Time,
    retained: bool,
) -> Result<HashMap<Item, Standing>, Error> {
    let ids: Vec<_> = entries.iter().map(|row| row.id).collect();
    let retained_ids: Vec<_> = entries
        .iter()
        .filter(|row| row.deleted_at.is_some())
        .map(|row| row.id)
        .collect();
    let work = async {
        match actor {
            Actor::Person(principal) => {
                let listed = documents::listed(conn, principal, &ids, Policy::default()).await?;
                Ok(listed
                    .into_iter()
                    .map(|row| {
                        let item = if row.document.kind
                            == orch_access_contract::permissions::ObjectKind::DocumentContainer
                        {
                            Item::Container(row.document.id)
                        } else {
                            Item::Document(row.document.id)
                        };
                        let write = if row.document.kind
                            == orch_access_contract::permissions::ObjectKind::DocumentContainer
                        {
                            row.holds(Permission::DocCreate)
                        } else {
                            row.holds(Permission::DocEdit)
                        };
                        (
                            item,
                            Standing {
                                read: row.holds(Permission::DocView),
                                write,
                                manage: row.holds(Permission::DocManage),
                            },
                        )
                    })
                    .collect())
            }
            Actor::Agent { thread } => {
                let mut graph = Graph::default();
                let documents = documents::read_into(conn, &mut graph, &ids).await?;
                let reach = agent_reach(conn, thread, documents.clone(), Vec::new(), now).await?;
                Ok(documents
                    .into_iter()
                    .map(|document| {
                        let item = if document.kind
                            == orch_access_contract::permissions::ObjectKind::DocumentContainer
                        {
                            Item::Container(document.id)
                        } else {
                            Item::Document(document.id)
                        };
                        let standing = reach.document(&document);
                        // Shared agent entries still name their holder under archive. Only an actual
                        // retained deletion may use those former mutation rights to be restored.
                        let mutable =
                            !reach.archived(document.node) || (retained && document.deleted);
                        (
                            item,
                            Standing {
                                read: standing.reads(),
                                write: mutable && standing.writes(),
                                manage: mutable && standing.manages(),
                            },
                        )
                    })
                    .collect())
            }
        }
    };
    if retained {
        Ok(orch_db::queries::trace::read_scope(&retained_ids, work).await?)
    } else {
        work.await
    }
}

pub(super) async fn require_create(
    conn: &mut Conn,
    actor: Actor<'_>,
    parent: Directory,
    now: Time,
) -> Result<(Id, Id, Option<Id>), Error> {
    let placement = directory(conn, parent).await?;
    let enabled = orch_settings::get::<bool>(conn, Place::Node(placement.0), keys::DOCS_ENABLED)
        .await
        .map_err(crate::EngineError::from)?
        .unwrap_or(true);
    if !enabled {
        return Err(Error::Denied);
    }
    let allowed = match actor {
        Actor::Person(principal) => {
            let decision = match parent {
                Directory::NodeDocs(node) => {
                    orch_authz::nodes::check_all(
                        conn,
                        principal,
                        node,
                        &[Permission::DocCreate],
                        Act::Use,
                        Policy::default(),
                    )
                    .await?
                }
                Directory::Container(id) => {
                    documents::check(
                        conn,
                        principal,
                        id,
                        Permission::DocCreate,
                        Policy::default(),
                    )
                    .await?
                }
            };
            matches!(decision, Decision::Allowed { .. })
        }
        Actor::Agent { thread } => {
            let mut graph = Graph::default();
            let descriptions = match parent {
                Directory::NodeDocs(_) => Vec::new(),
                Directory::Container(id) => documents::read_into(conn, &mut graph, &[id]).await?,
            };
            let reach =
                agent_reach(conn, thread, descriptions.clone(), vec![placement.0], now).await?;
            !reach.archived(placement.0)
                && match parent {
                    Directory::NodeDocs(node) => reach.documents_of(node).writes(),
                    Directory::Container(id) => descriptions
                        .iter()
                        .find(|row| row.id == id)
                        .is_some_and(|row| reach.document(row).writes()),
                }
        }
    };
    if !allowed {
        return Err(Error::Denied);
    }
    Ok(placement)
}

pub(super) async fn require_entries(
    conn: &mut Conn,
    actor: Actor<'_>,
    entries: &[containers::EntryRow],
    permission: Permission,
    now: Time,
    retained: bool,
    act: Act,
) -> Result<(), Error> {
    let held = standings(conn, actor, entries, now, retained).await?;
    let retained_ids: Vec<_> = entries
        .iter()
        .filter(|row| row.deleted_at.is_some())
        .map(|row| row.id)
        .collect();
    for row in entries {
        let standing = held.get(&item(row)).copied().unwrap_or_default();
        let allowed = match permission {
            Permission::DocView => standing.read,
            Permission::DocEdit => standing.write,
            _ => standing.manage,
        };
        if allowed {
            continue;
        }
        if retained && let Actor::Person(principal) = actor {
            // AsItWas is an exception for the selected tombstones, never for a live item
            // whose current permission was withheld by archive. Keep its native refusal.
            let was_deleted = row.deleted_at.is_some();
            let state = if was_deleted {
                documents::State::AsItWas
            } else {
                documents::State::Live
            };
            let permissions = [permission];
            let work = documents::check_all(
                conn,
                principal,
                row.id,
                &permissions,
                act,
                state,
                Policy::default(),
            );
            let decision = if was_deleted {
                orch_db::queries::trace::read_scope(&retained_ids, work).await?
            } else {
                work.await?
            };
            if matches!(decision, Decision::Allowed { .. }) {
                continue;
            }
        }
        return Err(Error::Denied);
    }
    Ok(())
}

pub(super) struct Identity {
    pub kind: &'static str,
    pub id: Id,
    pub owner: Id,
    pub thread: Option<Id>,
    pub agent: Option<String>,
}

pub(super) async fn identity(conn: &mut Conn, actor: Actor<'_>) -> Result<Identity, Error> {
    match actor {
        Actor::Person(principal) => Ok(Identity {
            kind: "person",
            id: principal.user(),
            owner: principal.user(),
            thread: None,
            agent: None,
        }),
        Actor::Agent { thread } => {
            let agent = reach::Agent::of(conn, thread)
                .await?
                .ok_or(Error::NotFound)?;
            let row = orch_db::queries::threads::thread(conn, thread)
                .await?
                .ok_or(Error::NotFound)?;
            let name = crate::tools::edit::agent_name(conn, &row).await?;
            Ok(Identity {
                kind: "agent",
                id: thread,
                owner: agent.owner,
                thread: Some(thread),
                agent: Some(name),
            })
        }
    }
}
