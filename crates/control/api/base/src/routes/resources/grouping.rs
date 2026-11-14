//! Grouping references from the native reader's selected identities.
//!
//! Producers supply their own operation's protected name and Machine bounds. This
//! module projects references only; availability remains the native picker's verdict.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::routes::access::page::NameBounds;

use crate::state::AppState;

use orch_access_contract::permissions::Permission;

use orch_api_contract::resource_groups::ResourceGrouping;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::resources::grouping as stored;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashMap;
/// Whether `reader` may open the machine that a resource names as its origin, so that the machine may label the
/// resource. An origin that cannot be opened labels nothing and fails nothing: a container's machine has no page of
/// its own (its provider's or environment's page governs it), and a reader who is suspended where it stands, or finds
/// its place archived, is shown no label. One answer for every list of resources, since a list must never fail whole
/// for the label of one row; only a fault of the store is an error.
pub async fn origin_label_allowed(
    conn: &mut Conn,
    reader: &Principal,
    machine: Id,
) -> ApiResult<bool> {
    match orch_authz::resources::check_machine(
        conn,
        reader,
        machine,
        Permission::MachineView,
        Policy::default(),
    )
    .await
    {
        Ok(decision) => Ok(decision.is_allowed()),
        Err(
            orch_authz::Error::NotFound(_)
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. },
        ) => {
            tracing::debug!(%machine, "an origin machine is not the reader's to open: the row is shown without its label");
            Ok(false)
        }
        Err(error) => Err(ApiError::internal(error)),
    }
}

#[derive(Default)]
pub struct Prepared {
    pub rows: Vec<stored::Row>,
}

impl Prepared {
    pub async fn select(conn: &mut Conn, subjects: &[(String, Id)]) -> ApiResult<Self> {
        Ok(Self {
            rows: stored::among(conn, subjects).await?,
        })
    }
    pub fn homes(&self) -> impl Iterator<Item = Id> + '_ {
        self.rows.iter().filter_map(|row| row.home_id)
    }
    pub fn origins(&self) -> BTreeSet<Id> {
        self.rows
            .iter()
            .filter_map(|row| row.origin_machine_id)
            .collect()
    }
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        self.rows.iter().all(|old| {
            fresh.rows.iter().any(|row| {
                old.kind == row.kind
                    && old.id == row.id
                    && old.home_id == row.home_id
                    && old.owner_id == row.owner_id
                    && old.origin_machine_id == row.origin_machine_id
                    && old.container_environment_id == row.container_environment_id
            })
        })
    }
    pub async fn choices(
        &self,
        app: &AppState,
        conn: &mut Conn,
        principal: &Principal,
        names: Option<&NameBounds>,
        machines: Option<&BTreeSet<Id>>,
    ) -> ApiResult<Vec<orch_api_contract::resource_groups::ResourceChoiceGrouping>> {
        use orch_access_contract::{
            decisions::ObjectRef,
            permissions::{ObjectKind, ResourceKind},
        };
        let machine_ids = self
            .rows
            .iter()
            .filter(|row| row.kind == "machine")
            .map(|row| row.id)
            .collect::<Vec<_>>();
        let facades = orch_db::queries::resources::hosting::machine_hosts(conn, &machine_ids)
            .await?
            .into_iter()
            .map(|(machine, host, _)| (machine, host))
            .collect::<HashMap<_, _>>();
        let mut subjects = HashMap::new();
        for row in &self.rows {
            let kind = row
                .kind
                .parse::<ResourceKind>()
                .map_err(ApiError::internal)?;
            let (object, permission) = if let Some(environment) = row.container_environment_id {
                (
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment,
                    },
                    Permission::EnvironmentView,
                )
            } else if kind == ResourceKind::Machine
                && let Some(&host) = facades.get(&row.id)
            {
                (
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    Permission::HostView,
                )
            } else {
                let permission = match kind {
                    ResourceKind::Provider => Permission::ProviderView,
                    ResourceKind::Agent => Permission::AgentView,
                    ResourceKind::Environment => Permission::EnvironmentView,
                    ResourceKind::Host => Permission::HostView,
                    ResourceKind::Machine => Permission::MachineView,
                    ResourceKind::Template => Permission::TemplateView,
                    ResourceKind::McpServer => Permission::McpView,
                    ResourceKind::Mount => Permission::MountView,
                    ResourceKind::Skill => Permission::SkillView,
                };
                (
                    ObjectRef {
                        kind: kind.into(),
                        id: row.id,
                    },
                    permission,
                )
            };
            subjects.insert((row.kind.clone(), row.id), (object, permission));
        }
        let ids = subjects
            .values()
            .map(|(object, _)| object.id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let held = orch_authz::resources::effective_among(conn, principal, &ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|held| (held.object, held))
            .collect::<HashMap<_, _>>();
        // This is the native authorization frame on this snapshot, not a DTO permission flag.
        // Keep the basic option if the current ordinary View does not license its extra metadata.
        let readable = Self {
            rows: self
                .rows
                .iter()
                .filter(|row| {
                    subjects
                        .get(&(row.kind.clone(), row.id))
                        .is_some_and(|(object, permission)| {
                            held.get(object).is_some_and(|held| {
                                held.held
                                    .iter()
                                    .any(|grant| grant.permission == *permission)
                            })
                        })
                })
                .cloned()
                .collect(),
        };
        let grouping = readable
            .render(app, conn, principal, names, machines)
            .await?;
        let owners = readable
            .rows
            .iter()
            .filter_map(|row| row.owner_id)
            .collect::<Vec<_>>();
        let owners = crate::middleware::authorization::refusals::user_refs(conn, &owners).await?;
        Ok(readable
            .rows
            .iter()
            .filter_map(|row| {
                grouping
                    .get(&(row.kind.clone(), row.id))
                    .cloned()
                    .map(
                        |grouping| orch_api_contract::resource_groups::ResourceChoiceGrouping {
                            kind: row.kind.clone(),
                            id: row.id,
                            owner: row.owner_id.and_then(|id| owners.get(&id).cloned()),
                            grouping,
                        },
                    )
            })
            .collect())
    }

    pub async fn render(
        &self,
        app: &AppState,
        conn: &mut Conn,
        principal: &Principal,
        names: Option<&NameBounds>,
        machines: Option<&BTreeSet<Id>>,
    ) -> ApiResult<HashMap<(String, Id), ResourceGrouping>> {
        let homes = self.homes().collect::<Vec<_>>();
        let graph = orch_authz::engine::Graph::load(conn, &homes)
            .await
            .map_err(ApiError::internal)?;
        let home_paths = graph.nodes().map(|node| node.id).collect::<Vec<_>>();
        let nodes = match names {
            Some(bounds) => bounds.refs(conn, principal, &home_paths).await?,
            None => {
                crate::routes::nodes::view::refs(conn, principal, &home_paths, Policy::default())
                    .await?
            }
        };
        let mut allowed = Vec::new();
        for id in self.origins() {
            if machines.is_some_and(|bounds| !bounds.contains(&id)) {
                continue;
            }
            if origin_label_allowed(conn, principal, id).await? {
                allowed.push(id);
            }
        }
        let origins = super::view::machine_refs(app, conn, &allowed).await?;
        Ok(self
            .rows
            .iter()
            .map(|row| {
                (
                    (row.kind.clone(), row.id),
                    ResourceGrouping {
                        home: row.home_id.and_then(|id| nodes.get(&id).cloned()),
                        home_path: row
                            .home_id
                            .and_then(|id| graph.get(id))
                            .into_iter()
                            .flat_map(|node| node.path.iter())
                            .filter_map(|id| nodes.get(id).cloned())
                            .collect(),
                        origin_machine: row
                            .origin_machine_id
                            .and_then(|id| origins.get(&id).cloned()),
                        availability: Vec::new(),
                    },
                )
            })
            .collect())
    }
}
