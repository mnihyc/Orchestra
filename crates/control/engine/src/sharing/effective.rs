//! Exact baseline reads, through the same native Reach used by the agent's tools.

use super::{Engine, PreparedAgentShare, ShareError, Subject};
use orch_api_contract::share::{AgentReadBefore, AgentReadBeforeReason as Reason};
use orch_authz::{
    documents::Document,
    reach::{Agent, Reach, Things},
};
use orch_db::{
    Conn,
    queries::{chats, documents, threads},
};
use orch_types::{Id, Time};
use std::collections::BTreeMap;

impl Engine {
    pub(super) async fn effective_reads_before(
        &self,
        conn: &mut Conn,
        prepared: &PreparedAgentShare,
        now: Time,
    ) -> Result<BTreeMap<Id, AgentReadBefore>, ShareError> {
        let document = match prepared.found.subject {
            Subject::Document(id) => documents::get_many(conn, &[id])
                .await?
                .first()
                .map(Document::of_row),
            _ => None,
        };
        let chat = match prepared.found.subject {
            Subject::Chat(id) => chats::chat(conn, id).await?,
            _ => None,
        };
        let thread = match prepared.found.subject {
            Subject::Node(id) if prepared.found.place_kind.as_deref() == Some("thread") => {
                threads::thread(conn, id).await?
            }
            _ => None,
        };
        let things = Things {
            nodes: thread
                .as_ref()
                .map(|row| row.node_id)
                .into_iter()
                .chain(chat.as_ref().and_then(|row| row.node_id))
                .collect(),
            documents: document.clone().into_iter().collect(),
            people_chats: chat
                .as_ref()
                .filter(|row| row.node_id.is_none())
                .map(|row| row.id)
                .into_iter()
                .collect(),
        };
        let mut out = BTreeMap::new();
        for &(id, kind) in &prepared.recipients {
            let fact = if kind != orch_access_contract::permissions::ObjectKind::Thread {
                AgentReadBefore {
                    readable: None,
                    reason: Some(Reason::RecipientContextRequired),
                }
            } else if let Some(agent) = Agent::of(conn, id).await? {
                if let Some(reason) = self.tools_unavailable(conn, id).await? {
                    AgentReadBefore {
                        readable: Some(false),
                        reason: Some(match reason {
                            "tools_off" => Reason::ToolsOff,
                            _ => Reason::NoTools,
                        }),
                    }
                } else if matches!(prepared.found.subject, Subject::Node(_)) && thread.is_none() {
                    AgentReadBefore {
                        readable: None,
                        reason: Some(Reason::PerTarget),
                    }
                } else {
                    let reach = Reach::load_current(conn, agent, now, &things).await?;
                    let readable = match prepared.found.subject {
                        Subject::Document(_) => {
                            document.as_ref().map(|row| reach.document(row).reads())
                        }
                        Subject::Chat(_) => chat.as_ref().map(|row| {
                            row.node_id.map_or_else(
                                || reach.people_chat(row.id).reads(),
                                |node| reach.node_chat(node).reads(),
                            )
                        }),
                        Subject::Node(_) => thread.as_ref().map(|row| {
                            reach
                                .conversation(row.node_id, row.side_of.is_some())
                                .reads()
                        }),
                    };
                    AgentReadBefore {
                        readable,
                        reason: match readable {
                            Some(true) => None,
                            Some(false) => Some(Reason::CurrentLimits),
                            None => Some(Reason::SourceUnavailable),
                        },
                    }
                }
            } else {
                AgentReadBefore {
                    readable: None,
                    reason: Some(Reason::SourceUnavailable),
                }
            };
            out.insert(id, fact);
        }
        Ok(out)
    }
}
