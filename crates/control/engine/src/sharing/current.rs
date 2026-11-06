//! Reader-authorized contribution assessments, using the same per-entry checks as agent acts.
//!
//! Callers first apply their existing source visibility rules. A source place is never reduced
//! to one root result, and a place holder is never represented by a fabricated recipient agent.

use super::{Record, ShareError, Subject};
use orch_api_contract::share::{
    AgentShareCurrent, AgentShareCurrentReason as Reason, AgentShareCurrentState as State,
    ShareDocumentsLevel, ShareLevel, ShareParts,
};
use orch_authz::documents::Document;
use orch_authz::reach::{Agent, Reach, Things};
use orch_db::Conn;
use orch_db::queries::{access, chats, documents};
use orch_types::{Id, Time};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

fn assessment(
    recipient: Option<Id>,
    parts: Option<ShareParts>,
    state: State,
    reason: Option<Reason>,
) -> AgentShareCurrent {
    AgentShareCurrent {
        recipient,
        parts,
        state,
        reason,
    }
}

/// Projects visible live records. `recipient` is the real thread of a thread-oriented read;
/// otherwise each record must itself name a real thread. One Reach is prepared per recipient,
/// for all its exact sources and givers, and each result admits only that record's entry IDs.
pub async fn project(
    conn: &mut Conn,
    records: &[Record],
    recipient: Option<Id>,
    now: Time,
) -> Result<HashMap<Id, AgentShareCurrent>, ShareError> {
    let live: Vec<_> = records
        .iter()
        .filter(|record| record.copy.is_none())
        .collect();
    if live.is_empty() {
        return Ok(HashMap::new());
    }
    let givers: BTreeSet<_> = live.iter().filter_map(|record| record.by).collect();
    let active: HashSet<_> = access::active_givers(conn, &givers.into_iter().collect::<Vec<_>>())
        .await?
        .into_iter()
        .collect();
    let recipients: BTreeSet<_> = live
        .iter()
        .map(|record| recipient.unwrap_or(record.to))
        .collect();
    let mut agents = HashMap::new();
    for id in recipients {
        agents.insert(id, Agent::of(conn, id).await?);
    }
    let mut out = HashMap::new();
    let mut batches: BTreeMap<Id, Vec<&Record>> = BTreeMap::new();
    for record in live {
        let agent = agents
            .get(&recipient.unwrap_or(record.to))
            .and_then(Option::as_ref)
            .filter(|agent| agent.path.contains(&record.to));
        let recipient = agent.map(|agent| agent.thread);
        let broad = matches!(record.subject, Subject::Node(_));
        if record.by.is_none_or(|giver| !active.contains(&giver)) {
            out.insert(
                record.id,
                assessment(
                    recipient,
                    (!broad && recipient.is_some()).then(ShareParts::default),
                    State::Inactive,
                    Some(Reason::GiverUnavailable),
                ),
            );
        } else if broad {
            out.insert(
                record.id,
                assessment(recipient, None, State::Unassessed, Some(Reason::PerTarget)),
            );
        } else if let Some(recipient) = recipient {
            batches.entry(recipient).or_default().push(record);
        } else {
            out.insert(
                record.id,
                assessment(
                    None,
                    None,
                    State::Unassessed,
                    Some(Reason::RecipientContextRequired),
                ),
            );
        }
    }
    for (recipient, records) in batches {
        let Some(agent) = agents.remove(&recipient).flatten() else {
            continue;
        };
        out.extend(for_recipient(conn, agent, records, now).await?);
    }
    Ok(out)
}

async fn for_recipient(
    conn: &mut Conn,
    agent: Agent,
    records: Vec<&Record>,
    now: Time,
) -> Result<HashMap<Id, AgentShareCurrent>, ShareError> {
    let recipient = agent.thread;
    let document_ids: BTreeSet<_> = records
        .iter()
        .filter_map(|record| match record.subject {
            Subject::Document(id) => Some(id),
            _ => None,
        })
        .collect();
    let chat_ids: BTreeSet<_> = records
        .iter()
        .filter_map(|record| match record.subject {
            Subject::Chat(id) => Some(id),
            _ => None,
        })
        .collect();
    let documents: HashMap<_, _> =
        documents::get_many(conn, &document_ids.into_iter().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|row| (row.id, Document::of_row(&row)))
            .collect();
    let chats: HashMap<_, _> = chats::chats(conn, &chat_ids.into_iter().collect::<Vec<_>>())
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let things = Things {
        nodes: chats.values().filter_map(|chat| chat.node_id).collect(),
        documents: documents.values().cloned().collect(),
        people_chats: chats
            .values()
            .filter(|chat| chat.node_id.is_none())
            .map(|chat| chat.id)
            .collect(),
    };
    let reach = Reach::load_current(conn, agent, now, &things).await?;
    let mut out = HashMap::new();
    for record in records {
        let entries: HashSet<_> = record.grants.iter().map(|grant| grant.entry_id).collect();
        let parts = match record.subject {
            Subject::Document(id) => documents.get(&id).map(|document| {
                let standing = reach.contribution_document(document, &entries);
                ShareParts {
                    documents: if standing.manages() {
                        Some(ShareDocumentsLevel::Manage)
                    } else if standing.writes() {
                        Some(ShareDocumentsLevel::Edit)
                    } else if standing.reads() {
                        Some(ShareDocumentsLevel::Read)
                    } else {
                        None
                    },
                    ..ShareParts::default()
                }
            }),
            Subject::Chat(id) => chats.get(&id).map(|chat| {
                let standing = reach.contribution_chat(id, chat.node_id, &entries);
                ShareParts {
                    chat: if standing.writes() {
                        Some(ShareLevel::Write)
                    } else if standing.reads() {
                        Some(ShareLevel::Read)
                    } else {
                        None
                    },
                    ..ShareParts::default()
                }
            }),
            Subject::Node(_) => None,
        };
        let (state, reason) = match parts {
            Some(parts) if parts == record.parts => (State::Active, None),
            Some(parts) if parts == ShareParts::default() => {
                (State::Inactive, Some(Reason::CurrentLimits))
            }
            Some(_) => (State::Reduced, Some(Reason::CurrentLimits)),
            None => (State::Unassessed, Some(Reason::SourceUnavailable)),
        };
        out.insert(record.id, assessment(Some(recipient), parts, state, reason));
    }
    Ok(out)
}
