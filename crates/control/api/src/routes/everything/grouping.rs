//! Home and origin references on the native reader's already selected resource page.

use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
    routes::resources::view,
};
use orch_access_contract::permissions::Permission;
use orch_api_contract::{nodes::NodeRef, resource_groups::ResourceGrouping, resources::MachineRef};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, queries::resources::grouping as stored};
use orch_types::Id;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Default)]
pub(super) struct Prepared {
    rows: HashMap<(String, Id), stored::Row>,
    origins: HashMap<Id, MachineRef>,
}

impl Prepared {
    /// Only protected references actually returned join the final inventory source footprint.
    pub(super) fn returned_origins(&self) -> impl Iterator<Item = Id> + '_ {
        self.origins.keys().copied()
    }

    pub(super) fn resource(
        &self,
        kind: &str,
        id: &str,
        path: &[NodeRef],
    ) -> ApiResult<Option<ResourceGrouping>> {
        let Ok(id) = id.parse::<Id>() else {
            return Ok(None);
        };
        let Some(row) = self.rows.get(&(kind.to_owned(), id)) else {
            return Ok(None);
        };
        let home = row
            .home_id
            .and_then(|home| path.iter().find(|node| node.id == home).cloned());
        if row.home_id.is_some() && home.is_none() {
            return Err(ApiError::internal(
                "the resource home is absent from its native inventory path",
            ));
        }
        Ok(Some(ResourceGrouping {
            home,
            home_path: path.to_vec(),
            origin_machine: row
                .origin_machine_id
                .and_then(|machine| self.origins.get(&machine).cloned()),
            // A flat inventory has no execution context. Contextual pickers supply
            // the native judgement; origin alone cannot decide remote arrangements.
            availability: Vec::new(),
        }))
    }
}

pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    rows: &[orch_db::queries::everything::Row],
    grants: &[orch_db::queries::everything::Grant],
    predicate: Option<&super::operation::Predicate>,
) -> ApiResult<Prepared> {
    let subjects = rows
        .iter()
        .zip(grants)
        .filter(|(row, grant)| grant.view && row.parent_id.as_deref() == Some("resources"))
        .map(|(row, _)| {
            row.id
                .parse::<Id>()
                .map(|id| (row.kind.clone(), id))
                .map_err(ApiError::internal)
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let selected = stored::among(conn, &subjects).await?;
    let candidates = selected
        .iter()
        .filter_map(|row| row.origin_machine_id)
        .collect::<BTreeSet<_>>();
    let mut allowed = Vec::new();
    for machine in candidates {
        let scoped;
        let reader = match predicate {
            Some(predicate) => {
                let Some(reader) = predicate.origin_principal(ctx, conn, machine).await? else {
                    continue;
                };
                scoped = reader;
                &scoped
            }
            None => principal,
        };
        if orch_authz::resources::check_machine(
            conn,
            reader,
            machine,
            Permission::MachineView,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed()
        {
            allowed.push(machine);
        }
    }
    Ok(Prepared {
        rows: selected
            .into_iter()
            .map(|row| ((row.kind.clone(), row.id), row))
            .collect(),
        origins: view::machine_refs(&ctx.app, conn, &allowed).await?,
    })
}
