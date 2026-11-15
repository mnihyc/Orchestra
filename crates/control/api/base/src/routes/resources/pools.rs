//! Native pool metadata projection with the existing resource holdings and protected names.
use crate::middleware::{
    authorization::refusals,
    requests::{ApiError, ApiResult},
};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::resources::view::{self, Holding, PreparedAccess};
use orch_api_contract::pools::Pool;
use orch_authz::{Principal, resources::Seen};
use orch_db::{
    Conn,
    queries::pools::{self as rows, Row},
};
use orch_types::Id;

pub struct Prepared {
    pub seen: Vec<Seen>,
    pub rows: Vec<Row>,
    pub access: PreparedAccess,
    pub sources: NameSources,
    pub bounds: NameBounds,
}
impl Prepared {
    pub fn name_sources(&self) -> &NameSources {
        &self.sources
    }
    pub fn bound_names(&mut self, bounds: NameBounds) {
        self.bounds = bounds;
    }
    pub fn preserves_sources(&self, fresh: &Self) -> bool {
        self.access.preserves_authority(&fresh.access)
            && self.rows.iter().all(|row| {
                fresh.rows.iter().any(|next| {
                    row.id == next.id
                        && row.home_id == next.home_id
                        && row.owner_id == next.owner_id
                        && row.kind == next.kind
                })
            })
    }
}
pub async fn prepare_inventory(conn: &mut Conn, seen: &[Seen]) -> ApiResult<Prepared> {
    let holdings: Vec<_> = seen.iter().map(Holding::from_seen).collect();
    let ids: Vec<_> = seen.iter().map(|row| row.resource.id).collect();
    let rows = rows::among(conn, &ids).await?;
    let access = view::prepare_accesses(conn, &holdings).await?;
    let mut sources = access.names.clone();
    sources.nodes.extend(rows.iter().map(|row| row.home_id));
    Ok(Prepared {
        seen: seen.to_vec(),
        rows,
        access,
        sources,
        bounds: NameBounds::default(),
    })
}
pub async fn show_inventory_prepared(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &Prepared,
) -> ApiResult<Vec<Pool>> {
    let holdings: Vec<_> = prepared.seen.iter().map(Holding::from_seen).collect();
    let people = refusals::user_refs(
        conn,
        &prepared
            .rows
            .iter()
            .map(|row| row.owner_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let homes = prepared
        .bounds
        .refs(
            conn,
            principal,
            &prepared
                .rows
                .iter()
                .map(|row| row.home_id)
                .collect::<Vec<_>>(),
        )
        .await?;
    let access = view::accesses_prepared(
        conn,
        principal,
        &holdings,
        &prepared.access,
        Some(&prepared.bounds),
    )
    .await?;
    let addresses = view::addresses_with_names(
        conn,
        &prepared.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        Some(&prepared.bounds),
    )
    .await?;
    prepared
        .rows
        .iter()
        .map(|row| {
            let definition: orch_provider_contract::pools::Definition =
                serde_json::from_value(row.definition.clone()).map_err(ApiError::internal)?;
            let home = homes.get(&row.home_id).cloned();
            Ok(Pool {
                id: row.id,
                name: row.name.clone(),
                description: row.description.clone(),
                address: addresses.get(&row.id).cloned().unwrap_or_default(),
                kind: definition.kind,
                enabled: definition.enabled,
                policy: definition.policy,
                lend_members: definition.lend_members,
                revision: row.revision,
                owner: people.get(&row.owner_id).cloned(),
                home: home.clone(),
                access: access.get(&row.id).cloned().ok_or_else(|| {
                    ApiError::internal("the pool's native holdings were not read")
                })?,
                grouping: orch_api_contract::resource_groups::ResourceGrouping {
                    home: home.clone(),
                    home_path: home.into_iter().collect(),
                    ..Default::default()
                },
            })
        })
        .collect()
}
