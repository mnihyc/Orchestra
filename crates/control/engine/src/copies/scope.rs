//! Factual copy sources restricted by the shared operation predicate, never another evaluator.

use super::*;
use crate::operation_scope::{Prepared, targets};
use orch_access_contract::key_scope::{KeyIdentityKind, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;

/// Current operation restriction for copy preparation; never supplied by a client.
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    /// The exact existing operation and its declared source collections.
    pub plan: &'a Prepared,
    /// The unchanged authenticated caller whose operation bounds apply.
    pub caller: &'a Caller,
}
/// A source actually used by the copy or a recipient's projected reference.
#[derive(Debug, Clone)]
pub struct SourceBinding {
    /// The declared snapshot source collection.
    pub role: &'static str,
    /// The actual business resource family.
    pub kind: KeyTargetKind,
    /// Fresh factual identity and ancestry from the shared target producer.
    pub target: KeyResolvedTarget,
}
/// One prepared copy and its materialized sources. Body bytes are never Debug/Serialize payloads.
#[derive(Clone)]
pub struct PreparedCopy {
    /// The unchanged copy format, with scoped omissions and safe path components.
    pub files: Files,
    /// Actual source reads used to make these bytes or names.
    pub sources: Vec<SourceBinding>,
}
/// One recipient's resolved reference forms, retained without rebuilding the copy.
#[derive(Clone)]
pub struct PreparedWritings {
    /// The normal recipient resolver's permitted outputs; absent entries remain ordinary hidden.
    pub writings: HashMap<orch_documents::references::Typed, orch_documents::references::Writing>,
    /// Actual source reads used to make these bytes or names.
    pub sources: Vec<SourceBinding>,
}

pub(super) fn record(sources: &mut Vec<SourceBinding>, binding: &SourceBinding) {
    if !sources
        .iter()
        .any(|known| known.role == binding.role && known.target.object == binding.target.object)
    {
        sources.push(binding.clone());
    }
}
impl Scope<'_> {
    pub(super) fn binding(
        self,
        target: KeyResolvedTarget,
    ) -> Result<Option<SourceBinding>, ShareError> {
        let kind = target.object.kind;
        let role = match kind {
            KeyTargetKind::Object(ObjectKind::Org) => "snapshot_source_orgs",
            KeyTargetKind::Object(ObjectKind::Project) => "snapshot_source_projects",
            KeyTargetKind::Object(ObjectKind::Topic) => "snapshot_source_topics",
            KeyTargetKind::Object(ObjectKind::Thread) => "snapshot_source_threads",
            KeyTargetKind::Object(ObjectKind::Document) => "snapshot_source_documents",
            KeyTargetKind::Object(ObjectKind::DocumentContainer) => {
                "snapshot_source_document_containers"
            }
            KeyTargetKind::Object(ObjectKind::Environment) => "snapshot_source_environments",
            KeyTargetKind::Identity(KeyIdentityKind::Chat) => "snapshot_source_chats",
            _ => return Ok(None),
        };
        let index = self
            .plan
            .variant()
            .facets
            .iter()
            .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
            .ok_or_else(|| {
                EngineError::unreadable("the snapshot source collection is not declared")
            })?;
        if !self
            .plan
            .permits_binding(self.caller, index, kind, std::slice::from_ref(&target))
            .map_err(EngineError::unreadable)?
        {
            return Ok(None);
        }
        Ok(Some(SourceBinding { role, kind, target }))
    }

    pub(super) async fn subject(
        self,
        conn: &mut Conn,
        subject: Subject,
    ) -> Result<Option<SourceBinding>, ShareError> {
        let target = match subject {
            Subject::Node(id) => targets::node(conn, id).await,
            Subject::Document(id) => {
                targets::page_object(
                    conn,
                    orch_access_contract::decisions::ObjectRef {
                        kind: ObjectKind::Document,
                        id,
                    },
                )
                .await
            }
            Subject::Chat(id) => targets::chat(conn, id).await,
        };
        let target = match target {
            Ok(target) => target,
            Err(targets::TargetError::Missing) => return Err(ShareError::NotFound),
            Err(targets::TargetError::Authz(error)) => return Err(error.into()),
            Err(targets::TargetError::Db(error)) => return Err(EngineError::from(error).into()),
        };
        self.binding(target)
    }
}

impl Scope<'_> {
    /// Modern operation rules are an additional boundary; ordinary and legacy recipients retain
    /// their established optional-reference behavior where no such rule policy exists.
    pub(super) fn restricted(self) -> bool {
        self.caller
            .grant()
            .and_then(|grant| grant.operation_scope())
            .is_some()
    }

    pub(super) async fn reference(
        self,
        conn: &mut Conn,
        typed: &orch_documents::references::Typed,
        recipient_topic: Option<Id>,
    ) -> Result<Option<Vec<SourceBinding>>, ShareError> {
        use orch_access_contract::decisions::ObjectRef;
        use orch_api_contract::documents::InlineRefKind;
        let mut chat_node = None;
        let target = match typed.kind {
            InlineRefKind::Node => targets::node(conn, typed.id).await,
            InlineRefKind::Document => {
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Document,
                        id: typed.id,
                    },
                )
                .await
            }
            InlineRefKind::Chat => {
                let Some(chat) = orch_db::queries::chats::chat(conn, typed.id)
                    .await
                    .map_err(EngineError::from)?
                else {
                    return Ok(None);
                };
                chat_node = chat.node_id;
                targets::from_chat(conn, &chat).await
            }
            InlineRefKind::File => {
                // Use the exact retained owner witness of the ordinary recipient producer.
                // A synthetic UUID is never inverted or treated as its own authority target.
                let owner: Option<Id> =
                    sqlx::query_scalar("SELECT document_id FROM document_files WHERE id=$1")
                        .bind(typed.id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(orch_db::DbError::from)
                        .map_err(EngineError::from)?;
                if let Some(owner) = owner {
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Document,
                            id: owner,
                        },
                    )
                    .await
                } else if let Some((chat, _, _)) =
                    crate::tools::tree::attachments::chat_file_reference(conn, typed.id).await?
                {
                    // find_attachment will independently recheck this recipient's current Chat
                    // reach and the actual message/slot before constructing its filename/path.
                    targets::chat(conn, chat).await
                } else {
                    return Ok(None);
                }
            }
            InlineRefKind::EnvironmentFile => {
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: typed.id,
                    },
                )
                .await
            }
        };
        let target = match target {
            Ok(target) => target,
            Err(targets::TargetError::Missing)
            | Err(targets::TargetError::Authz(
                orch_authz::Error::NotFound(_) | orch_authz::Error::Archived { .. },
            )) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error).into()),
        };
        let Some(binding) = self.binding(target.clone())? else {
            return Ok(None);
        };
        let mut sources = vec![binding];
        // Workspace paths belong to the actual environment named by Typed.id. The original
        // recipient resolver still verifies its own workspace and path form, without new FS work.
        if typed.kind == InlineRefKind::EnvironmentFile {
            return Ok(Some(sources));
        }
        let relative = recipient_topic.and_then(|topic| {
            if typed.kind == InlineRefKind::Node && typed.id == topic {
                return None;
            }
            target
                .node_ancestors
                .iter()
                .position(|ancestor| ancestor.id == Some(topic))
        });
        let mut nodes = target
            .node_ancestors
            .iter()
            .skip(relative.map_or(0, |at| at + 1))
            .filter_map(|ancestor| ancestor.id)
            .collect::<Vec<_>>();
        // Even a relative node-chat path has a dynamic "Chat of <place>" label.
        nodes.extend(chat_node);
        nodes.sort_unstable();
        nodes.dedup();
        if !nodes.is_empty() {
            let graph = Graph::load(conn, &nodes).await?;
            for id in nodes {
                let Some(site) = graph.site_of(id) else {
                    return Ok(None);
                };
                let Some(target) = targets::from_site(&graph, &site) else {
                    return Ok(None);
                };
                let Some(binding) = self.binding(target)? else {
                    return Ok(None);
                };
                record(&mut sources, &binding);
            }
        }
        // Folder labels remain inside the live document path even when its topic prefix is relative.
        for ancestor in target.document_ancestors {
            let Some(id) = ancestor.id else {
                return Ok(None);
            };
            let target = match targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::DocumentContainer,
                    id,
                },
            )
            .await
            {
                Ok(target) => target,
                Err(targets::TargetError::Missing)
                | Err(targets::TargetError::Authz(
                    orch_authz::Error::NotFound(_) | orch_authz::Error::Archived { .. },
                )) => return Ok(None),
                Err(error) => return Err(EngineError::unreadable(error).into()),
            };
            let Some(binding) = self.binding(target)? else {
                return Ok(None);
            };
            record(&mut sources, &binding);
        }
        Ok(Some(sources))
    }
}
