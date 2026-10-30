//! Current topic-document policy and first-reading paths, resolved by the ordinary shared tree with no bodies in context.

use crate::tools::tree::{Scope, Tree};
use crate::{engine::Engine, error::EngineError};
use orch_api_contract::share::ReadFirstPath;
use orch_authz::reach::DocumentPolicy;
use orch_db::{Conn, queries::shares};
use orch_types::Id;
use std::collections::HashSet;

/// Material facts the current turn and Knowledge may describe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    /// Independent topic-document policy, before offered tools narrow actions.
    pub policy: DocumentPolicy,
    /// Current live paths or verified delivered copy indexes.
    pub paths: Vec<ReadFirstPath>,
    /// The agent's own conversation in the same grammar its shared tools accept.
    pub conversation_path: String,
}

impl Engine {
    /// Resolves stable hint identities against current pages and names. No document text is placed
    /// in this projection; expired/revoked and cut-loose entries contribute no path.
    pub async fn topic_material(
        &self,
        conn: &mut Conn,
        thread: Id,
    ) -> Result<Material, EngineError> {
        let Some(asking) = self.asking(conn, thread).await? else {
            return Ok(Material {
                policy: DocumentPolicy::None,
                paths: Vec::new(),
                conversation_path: String::new(),
            });
        };
        let grants = shares::grants_to(
            conn,
            &asking.agent.path,
            &["doc.view".to_owned()],
            asking.now.get(),
        )
        .await?;
        let hinted: HashSet<Id> = grants
            .iter()
            .filter(|grant| grant.read_first)
            .map(|grant| grant.entry_id)
            .collect();
        let mut paths = Vec::new();
        let tree = Tree::load(
            conn,
            asking.agent.clone(),
            asking.policy,
            asking.documents_policy,
            asking.now,
            if hinted.is_empty() {
                Scope::Under(thread)
            } else {
                Scope::Everything
            },
        )
        .await?;
        let conversation_path = tree
            .conversations()
            .into_iter()
            .find(|(id, _, _)| *id == thread)
            .map(|(_, _, entry)| entry.path)
            .ok_or_else(|| {
                EngineError::unreadable("the current thread has no conversation path")
            })?;
        if !hinted.is_empty() {
            for (document, path) in tree.documents() {
                if tree.reach().first_reading(
                    &orch_authz::documents::Document::of_row(&document.row),
                    &hinted,
                ) {
                    paths.push(ReadFirstPath {
                        id: document.row.id,
                        path,
                    });
                }
            }
        }
        for copy in shares::copies_to(conn, &[thread]).await? {
            if copy.read_first
                && copy.state == "delivered"
                && copy.include.iter().any(|area| area == "documents")
            {
                // The ordinary producer always writes and verifies this index with the manifest.
                paths.push(ReadFirstPath {
                    id: copy.id,
                    path: format!("{}{}", copy.path, crate::copies::INDEX),
                });
            }
        }
        paths.sort_by(|one, two| one.path.cmp(&two.path).then(one.id.cmp(&two.id)));
        paths.dedup_by(|one, two| one.id == two.id);
        Ok(Material {
            policy: asking.documents_policy,
            paths,
            conversation_path,
        })
    }
}
