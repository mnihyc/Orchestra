//! Actual explicit resource permission subjects shared by library figures and people.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::{engine::Graph, resources};
use orch_db::{Conn, queries::resources::mcp};
use orch_engine::operation_scope::targets;
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

pub(crate) struct Subject {
    pub object: ObjectRef,
    pub target: KeyResolvedTarget,
    pub environment: Option<KeyResolvedTarget>,
}

pub(crate) async fn select(conn: &mut Conn, objects: &[ObjectRef]) -> ApiResult<Vec<Subject>> {
    let wanted: BTreeSet<_> = objects
        .iter()
        .map(|object| (object.kind, object.id))
        .collect();
    if wanted.iter().any(|(kind, _)| kind.resource().is_none()) {
        return Err(ApiError::not_found());
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mcp_ids: Vec<_> = wanted
        .iter()
        .filter(|(kind, _)| *kind == ObjectKind::McpServer)
        .map(|(_, id)| *id)
        .collect();
    let owned: HashMap<Id, Id> = mcp::subjects(conn, &mcp_ids)
        .await?
        .into_iter()
        .filter_map(|row| row.environment_id.map(|environment| (row.id, environment)))
        .collect();
    let mut ids: BTreeSet<_> = wanted.iter().map(|(_, id)| *id).collect();
    ids.extend(owned.values().copied());
    let mut graph = Graph::default();
    let rows: HashMap<_, _> =
        resources::read_into(conn, &mut graph, &ids.into_iter().collect::<Vec<_>>())
            .await
            .map_err(authorization::restoration_error)?
            .into_iter()
            .map(|row| ((ObjectKind::from(row.kind), row.id), row))
            .collect();
    let mut result = Vec::new();
    for (kind, id) in wanted {
        let object = ObjectRef { kind, id };
        let environment = if kind == ObjectKind::McpServer {
            owned.get(&id).copied()
        } else {
            None
        };
        let native = match environment {
            Some(id) => rows.get(&(ObjectKind::Environment, id)),
            None => rows.get(&(kind, id)),
        }
        .ok_or_else(ApiError::not_found)?;
        let target =
            targets::from_resource_identity(&graph, native).ok_or_else(ApiError::not_found)?;
        result.push(Subject {
            object,
            environment: environment.map(|_| target.clone()),
            target,
        });
    }
    Ok(result)
}
