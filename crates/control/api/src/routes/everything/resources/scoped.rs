//! Reuse qualified native resource preparations under this inventory's own operation.

use super::*;
use crate::routes::access::operations::NameRoles;
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::resources::projection::{self, Rendered};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;

pub(in crate::routes::everything) const NAMES: NameRoles = NameRoles {
    groups: Some("inventory_groups"),
    nodes: [
        (ObjectKind::Org, "inventory_names_orgs"),
        (ObjectKind::Project, "inventory_names_projects"),
        (ObjectKind::Topic, "inventory_names_topics"),
        (ObjectKind::Thread, "inventory_names_threads"),
    ],
    containers: Some("inventory_containers"),
};

pub(in crate::routes::everything) struct Prepared(projection::Prepared);

pub(in crate::routes::everything) struct Selection<'a> {
    pub kind: &'a str,
    pub rows: &'a [Row],
    pub grants: &'a [Grant],
    pub projection: &'a Projection,
}

impl Prepared {
    pub(in crate::routes::everything) fn name_sources(&self) -> &NameSources {
        self.0.name_sources()
    }

    pub(in crate::routes::everything) fn foreign_sources(&self) -> &[ObjectRef] {
        self.0.foreign_sources()
    }

    pub(in crate::routes::everything) fn bound_names(&mut self, bounds: NameBounds) {
        self.0.bound_names(bounds);
    }

    pub(in crate::routes::everything) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<HashMap<String, EverythingResource>> {
        // Only the inventory wrapper lives here; the native renderer is shared.
        macro_rules! inventory {
            ($rows:expr, $kind:ident) => {
                $rows
                    .into_iter()
                    .map(|row| EverythingResource::$kind(Box::new(row)))
                    .collect()
            };
        }
        let values = match self.0.render(ctx, conn, principal).await? {
            Rendered::Agent(rows) => inventory!(rows, Agent),
            Rendered::Provider(rows) => inventory!(rows, Provider),
            Rendered::Template(rows) => inventory!(rows, Template),
            Rendered::Machine(rows) => inventory!(rows, Machine),
            Rendered::Mount(rows) => inventory!(rows, Mount),
            Rendered::Skill(rows) => inventory!(rows, Skill),
            Rendered::Host(rows) => inventory!(rows, Host),
            Rendered::Environment(rows) => inventory!(rows, Environment),
            Rendered::Mcp(rows) => inventory!(rows, McpServer),
        };
        Ok(super::keyed(values))
    }
}

pub(in crate::routes::everything) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    selection: Selection<'_>,
) -> ApiResult<Option<Prepared>> {
    let ids: HashSet<Id> = selection
        .rows
        .iter()
        .zip(selection.grants)
        .filter(|(_, grant)| grant.view)
        .map(|(row, _)| row.id.parse().map_err(ApiError::internal))
        .collect::<ApiResult<_>>()?;
    if ids.is_empty() {
        return Ok(None);
    }
    let seen: Vec<_> = selection
        .projection
        .resources
        .iter()
        .filter(|seen| {
            seen.resource.kind.as_str() == selection.kind && ids.contains(&seen.resource.id)
        })
        .cloned()
        .collect();
    let kind = selection
        .kind
        .parse()
        .map_err(|_| ApiError::invalid("kinds", "Choose one resource kind."))?;
    projection::prepare(
        ctx,
        conn,
        principal,
        projection::Selection {
            kind,
            ids: ids.into_iter().collect(),
            seen: &seen,
        },
    )
    .await
    .map(|prepared| prepared.map(Prepared))
}
