//! Reach-shaped candidates for an agent's tree, with ancestors kept as paths rather than content scopes.

use super::{Agent, AgentReach, EngineError, Id, NODES_MAX, PlaceRow, Scope, rows};
use orch_access_contract::pages::StoredKind;
use orch_authz::reach::Grant;
use orch_db::{Conn, queries::documents};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Loaded {
    pub places: Vec<PlaceRow>,
    pub content: Vec<Id>,
    pub documents: Vec<Id>,
    pub containers: Vec<Id>,
    pub capped: bool,
}

/// Candidate scopes come only from the implicit topic/project and current agent grants. Reach still decides each row.
pub(super) async fn load(
    conn: &mut Conn,
    agent: &Agent,
    policy: AgentReach,
    scope: Scope,
    grants: &[Grant],
) -> Result<Loaded, EngineError> {
    let mut seeds = BTreeSet::from([if policy == AgentReach::Project {
        agent.project.unwrap_or(agent.topic)
    } else {
        agent.topic
    }]);
    let mut document_ids = BTreeSet::new();
    let mut container_ids = BTreeSet::new();
    let mut chat_ids = BTreeSet::new();
    for grant in grants {
        match grant.page.kind {
            StoredKind::Node => {
                seeds.insert(grant.page.id);
            }
            StoredKind::Document => {
                document_ids.insert(grant.page.id);
            }
            StoredKind::DocumentContainer => {
                container_ids.insert(grant.page.id);
            }
            StoredKind::Chat => {
                chat_ids.insert(grant.page.id);
            }
            _ => {}
        }
    }
    let under = match scope {
        Scope::Everything => None,
        Scope::Under(id) => rows::places(conn, &[id]).await?.into_iter().next(),
    };
    let mut roots = Vec::new();
    for seed in rows::places(conn, &seeds.into_iter().collect::<Vec<_>>()).await? {
        match scope {
            Scope::Everything => roots.push(seed),
            Scope::Under(id) if seed.path.contains(&id) => roots.push(seed),
            Scope::Under(_) => {
                if let Some(under) = under.as_ref().filter(|under| under.path.contains(&seed.id)) {
                    roots.push(under.clone());
                }
            }
        }
    }
    // A wider granted subtree already contains any narrower seed; never read it twice.
    roots.sort_by_key(|root| (root.path.len(), root.id));
    let mut selected: Vec<PlaceRow> = Vec::new();
    for root in roots {
        if !selected.iter().any(|above| root.path.contains(&above.id)) {
            selected.push(root);
        }
    }
    let mut content = BTreeMap::new();
    for root in selected {
        let id = root.id;
        content.insert(id, root);
        let mut before = None;
        loop {
            let page = rows::below_page(conn, id, before, NODES_MAX).await?;
            for row in page.items {
                content.insert(row.id, row);
            }
            match page.next_cursor {
                Some(next) => before = Some(next),
                None => break,
            }
        }
    }
    // The agent itself remains available even when it is an aside omitted by the ordinary subtree reader.
    if matches!(scope, Scope::Everything)
        || matches!(scope,Scope::Under(id) if agent.path.contains(&id))
    {
        for row in rows::places(conn, &[agent.thread]).await? {
            content.insert(row.id, row);
        }
    }
    let granted = documents::get_many(conn, &document_ids.into_iter().collect::<Vec<_>>()).await?;
    let folders =
        documents::containers::get_many(conn, &container_ids.into_iter().collect::<Vec<_>>())
            .await?;
    let homes: Vec<_> = granted
        .iter()
        .map(|doc| doc.node_id)
        .chain(folders.iter().map(|folder| folder.node_id))
        .collect();
    let homes = rows::places(conn, &homes).await?;
    let relevant = |node: Id| match scope {
        Scope::Everything => true,
        Scope::Under(id) => homes
            .iter()
            .any(|home| home.id == node && home.path.contains(&id)),
    };
    let documents: Vec<_> = granted
        .iter()
        .filter(|doc| relevant(doc.node_id))
        .map(|doc| doc.id)
        .collect();
    let containers: Vec<_> = folders
        .iter()
        .filter(|folder| relevant(folder.node_id))
        .map(|folder| folder.id)
        .collect();
    let mut paths: BTreeSet<_> = agent.path.iter().copied().collect();
    for place in content.values() {
        paths.extend(place.path.iter().copied());
    }
    for home in homes.iter().filter(|home| relevant(home.id)) {
        paths.extend(home.path.iter().copied());
    }
    for chat in rows::people_chats(conn, &chat_ids.into_iter().collect::<Vec<_>>()).await? {
        paths.insert(chat.org_id);
    }
    let content_ids = content.keys().copied().collect();
    let mut places: BTreeMap<_, _> = rows::places(conn, &paths.into_iter().collect::<Vec<_>>())
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    places.extend(content);
    let mut places: Vec<_> = places.into_values().collect();
    places.sort_by_key(|place| (place.created_at, place.id));
    Ok(Loaded {
        places,
        content: content_ids,
        documents,
        containers,
        capped: false,
    })
}

/// Only the selected native metadata and its ancestry; permission decisions remain in Reach.
pub(super) async fn headers(
    conn: &mut Conn,
    agent: &Agent,
    selected: &orch_db::queries::search::CandidateSet,
    scope: Scope,
) -> Result<Loaded, EngineError> {
    let documents = documents::get_many(conn, &selected.documents).await?;
    let chats = orch_db::queries::chats::chats(conn, &selected.chats).await?;
    let mut homes: BTreeSet<_> = selected
        .nodes
        .iter()
        .copied()
        .chain(agent.path.iter().copied())
        .chain(documents.iter().map(|row| row.node_id))
        .chain(chats.iter().map(|row| row.node_id.unwrap_or(row.org_id)))
        .collect();
    if let Scope::Under(id) = scope {
        homes.insert(id);
    }
    let places = rows::header_places(conn, &homes.iter().copied().collect::<Vec<_>>()).await?;
    let mut paths = homes;
    for place in places {
        paths.extend(place.path);
    }
    Ok(Loaded {
        places: rows::header_places(conn, &paths.into_iter().collect::<Vec<_>>()).await?,
        content: selected
            .nodes
            .iter()
            .copied()
            .chain(documents.iter().map(|row| row.node_id))
            .chain(chats.iter().filter_map(|row| row.node_id))
            .collect(),
        documents: selected.documents.clone(),
        containers: Vec::new(),
        capped: false,
    })
}
