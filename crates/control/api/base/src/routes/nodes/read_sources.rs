//! Actual typed references of the native node projection, without an authorization decision.

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::routes::access::subject;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::documents::Author;

use orch_api_contract::documents::DocumentRef;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeBase;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::nodes::NodeRef;

use orch_authz::engine::Graph;

use orch_db::Conn;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashMap;
#[derive(Default)]
pub struct Sources(pub BTreeSet<(ObjectKind, Id)>);

impl Sources {
    /// Typed identities already selected by an existing native factual producer.
    pub fn from_objects(objects: impl IntoIterator<Item = ObjectRef>) -> Self {
        Self(
            objects
                .into_iter()
                .map(|object| (object.kind, object.id))
                .collect(),
        )
    }
    pub fn object(&mut self, kind: ObjectKind, id: Id) {
        self.0.insert((kind, id));
    }

    /// Record only a typed reference whose protected name or display is actually disclosed.
    pub fn node(&mut self, node: &NodeRef) {
        if node.name.is_some() || node.thread_display.is_some() {
            let kind = match node.kind {
                orch_api_contract::nodes::NodeKind::Org => ObjectKind::Org,
                orch_api_contract::nodes::NodeKind::Project => ObjectKind::Project,
                orch_api_contract::nodes::NodeKind::Topic => ObjectKind::Topic,
                orch_api_contract::nodes::NodeKind::Thread => ObjectKind::Thread,
            };
            self.object(kind, node.id);
        }
    }

    pub fn place(&mut self, place: &NodeOrSystem) {
        if let NodeOrSystem::Node(node) = place {
            self.node(node);
        }
    }

    pub fn author(&mut self, author: &Author) {
        if let Author::Agent(agent) = author {
            self.node(&agent.thread);
        }
    }

    pub fn document(&mut self, document: &DocumentRef) {
        self.object(ObjectKind::Document, document.id);
        self.node(&document.node);
        self.author(&document.created_by);
        if let Some(author) = &document.updated_by {
            self.author(author);
        }
        if let Some(admin) = &document.admin_access {
            self.place(&admin.place);
        }
    }

    pub fn base<K>(&mut self, base: &NodeBase<K>) {
        for node in &base.path {
            self.node(node);
        }
        for tag in &base.tags {
            self.node(&tag.node);
        }
        if let Some(place) = &base.access.role_from {
            self.place(place);
        }
        if let Some(place) = &base.access.admin_place {
            self.place(place);
        }
        if let Some(admin) = &base.access.admin_access {
            self.place(&admin.place);
        }
        if let Some(document) = &base.readme {
            self.document(document);
        }
    }

    /// This visits actual returned payloads, never guessed bindings or an unrelated endpoint's proof.
    pub fn of(node: &Node) -> ApiResult<Self> {
        let mut sources = Self::default();
        match node {
            Node::Org(node) => sources.base(&node.base),
            Node::Project(node) => sources.base(&node.base),
            Node::Topic(node) => {
                sources.base(&node.base);
                for environment in &node.environments {
                    sources.object(ObjectKind::Environment, environment.id);
                }
            }
            Node::Thread(node) => {
                sources.base(&node.base);
                if let Some(parent) = &node.side_of {
                    sources.node(parent);
                }
                if let Some(environment) = &node.environment {
                    sources.object(ObjectKind::Environment, environment.id);
                }
                if let Some(binding) = &node.binding {
                    sources.object(ObjectKind::Agent, binding.agent.id);
                    sources.object(ObjectKind::Provider, binding.provider.id);
                    if let Some(machine) = &binding.agent.machine {
                        sources.object(ObjectKind::Machine, machine.id);
                    }
                }
                if let Some(sandbox) = node
                    .blocked
                    .as_ref()
                    .and_then(|blocked| blocked.sandbox.as_ref())
                {
                    sources.object(ObjectKind::Environment, sandbox.environment.id);
                    if let Some(machine) = &sandbox.machine {
                        sources.object(ObjectKind::Machine, machine.id);
                    }
                }
                for missing in &node.input_access.missing {
                    if missing.object.name.is_some() {
                        let kind =
                            subject::kind_of_word(&missing.object.kind).ok_or_else(|| {
                                ApiError::internal(
                                    "a named input-access source has no typed identity",
                                )
                            })?;
                        sources.object(kind, missing.object.id);
                    }
                }
            }
        }
        Ok(sources)
    }

    /// Actual typed references of the existing Shared Node/document/folder union.
    pub fn of_shared(item: &orch_api_contract::nodes::SharedListItem) -> ApiResult<Self> {
        use orch_api_contract::nodes::SharedListItem;
        let mut sources = Self::default();
        match item {
            SharedListItem::Node(node) => return Self::of(node),
            SharedListItem::Document { document, path, .. } => {
                sources.document(document);
                for node in path {
                    sources.node(node);
                }
            }
            SharedListItem::DocumentContainer {
                path, admin_access, ..
            } => {
                for node in path {
                    sources.node(node);
                }
                if let Some(admin) = admin_access {
                    sources.place(&admin.place);
                }
            }
        }
        Ok(sources)
    }

    pub fn objects(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        self.0.iter().map(|&(kind, id)| ObjectRef { kind, id })
    }

    /// Current placement is read in batches, using the same neutral Graph/Site
    /// producers as ordinary node/document/resource readers. Machines retain
    /// their canonical Host facade or personal Machine subject.
    pub async fn targets(
        &self,
        conn: &mut Conn,
    ) -> ApiResult<HashMap<ObjectRef, KeyResolvedTarget>> {
        self.resolve_targets(conn, false).await
    }

    /// Identity facts for references already selected by their native context producer.
    /// Retained resources remain references; this does not authorize live Get/inspection.
    pub async fn identity_targets(
        &self,
        conn: &mut Conn,
    ) -> ApiResult<HashMap<ObjectRef, KeyResolvedTarget>> {
        self.resolve_targets(conn, true).await
    }

    pub async fn resolve_targets(
        &self,
        conn: &mut Conn,
        identities: bool,
    ) -> ApiResult<HashMap<ObjectRef, KeyResolvedTarget>> {
        let mut found = HashMap::new();
        let nodes: Vec<_> = self
            .objects()
            .filter(|object| object.kind.node().is_some())
            .collect();
        let graph = Graph::load(
            conn,
            &nodes.iter().map(|object| object.id).collect::<Vec<_>>(),
        )
        .await
        .map_err(ApiError::internal)?;
        for object in nodes {
            let target = if identities {
                let actual = graph
                    .get(object.id)
                    .filter(|node| ObjectKind::from(node.kind) == object.kind)
                    .ok_or_else(ApiError::not_found)?;
                orch_engine::operation_scope::targets::from_node_identity(&graph, actual.id)
                    .ok_or_else(ApiError::not_found)?
            } else {
                let site = graph
                    .site_of(object.id)
                    .filter(|site| site.kind == object.kind)
                    .ok_or_else(ApiError::not_found)?;
                targets::from_site(&graph, &site)?
            };
            found.insert(object, target);
        }
        let documents: Vec<_> = self
            .objects()
            .filter(|object| object.kind.is_document())
            .collect();
        if !documents.is_empty() {
            let mut graph = Graph::default();
            let rows: HashMap<_, _> = orch_authz::documents::read_into(
                conn,
                &mut graph,
                &documents.iter().map(|object| object.id).collect::<Vec<_>>(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|row| ((row.kind, row.id), row))
            .collect();
            for object in documents {
                let row = rows
                    .get(&(object.kind, object.id))
                    .filter(|row| identities || !row.deleted)
                    .ok_or_else(ApiError::not_found)?;
                let site =
                    orch_authz::documents::site_of(&graph, row).ok_or_else(ApiError::not_found)?;
                found.insert(object, targets::from_site(&graph, &site)?);
            }
        }
        let resources: Vec<_> = self
            .objects()
            .filter(|object| {
                object.kind.node().is_none()
                    && !object.kind.is_document()
                    && object.kind != ObjectKind::Machine
            })
            .collect();
        if !resources.is_empty() {
            let mut graph = Graph::default();
            let rows: HashMap<_, _> = orch_authz::resources::read_into(
                conn,
                &mut graph,
                &resources.iter().map(|object| object.id).collect::<Vec<_>>(),
            )
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|row| ((ObjectKind::from(row.kind), row.id), row))
            .collect();
            for object in resources {
                let row = rows
                    .get(&(object.kind, object.id))
                    .filter(|row| identities || !row.deleted)
                    .ok_or_else(ApiError::not_found)?;
                let target = if identities {
                    orch_engine::operation_scope::targets::from_resource_identity(&graph, row)
                        .ok_or_else(ApiError::not_found)?
                } else {
                    let site = orch_authz::resources::site_of(&graph, row)
                        .ok_or_else(ApiError::not_found)?;
                    targets::from_site(&graph, &site)?
                };
                found.insert(object, target);
            }
        }
        let machines: Vec<_> = self
            .objects()
            .filter(|object| object.kind == ObjectKind::Machine)
            .collect();
        let ids: Vec<_> = machines.iter().map(|object| object.id).collect();
        let mut physical = if identities {
            targets::machine_identities(conn, &ids).await?
        } else {
            targets::machine_subjects(conn, &ids).await?
        };
        for object in machines {
            found.insert(
                object,
                physical
                    .remove(&object.id)
                    .ok_or_else(ApiError::not_found)?,
            );
        }
        Ok(found)
    }
}
