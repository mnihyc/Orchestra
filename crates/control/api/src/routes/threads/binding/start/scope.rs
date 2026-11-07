//! Native binding-choice bounds; canonical actions stay in the operation declaration.

use super::super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::binding_sources::{self as sources, Source};
use orch_authz::{Caller, engine::Graph};
use std::collections::BTreeMap;

pub(super) struct Bounds {
    plan: OperationPlan,
    caller: Caller,
}
impl Bounds {
    pub fn new(ctx: &Ctx, caller: &Caller) -> ApiResult<Self> {
        Ok(Self {
            plan: OperationPlan::new(ctx.route, sources::OPTIONS.operation)?,
            caller: caller.clone(),
        })
    }
    pub async fn object(
        &self,
        conn: &mut Conn,
        object: ObjectRef,
    ) -> ApiResult<(bool, KeyResolvedTarget)> {
        let target = if object.kind == ObjectKind::Machine {
            targets::machine_identities(conn, &[object.id])
                .await?
                .remove(&object.id)
                .ok_or_else(ApiError::not_found)?
        } else {
            targets::resource_identity(conn, object).await?
        };
        let allowed = !native::modern(&self.caller)
            || native::permits(
                &self.plan,
                &sources::OPTIONS,
                &self.caller,
                &Source::OptionsChoices,
                KeyTargetKind::Object(object.kind),
                &target,
            )?;
        Ok((allowed, target))
    }
}
impl choice_window::Bounds for Bounds {
    fn allows<'a>(
        &'a self,
        conn: &'a mut Conn,
        rows: Vec<&'a resources::Resource>,
    ) -> BoxFuture<'a, ApiResult<Vec<bool>>> {
        Box::pin(async move {
            let homes = rows.iter().filter_map(|row| row.home).collect::<Vec<_>>();
            let graph = Graph::load(conn, &homes)
                .await
                .map_err(ApiError::internal)?;
            let mut allowed = Vec::with_capacity(rows.len());
            for row in rows {
                let target =
                    orch_engine::operation_scope::targets::from_resource_identity(&graph, row)
                        .ok_or_else(ApiError::not_found)?;
                allowed.push(native::permits(
                    &self.plan,
                    &sources::OPTIONS,
                    &self.caller,
                    &Source::OptionsChoices,
                    KeyTargetKind::Object(ObjectKind::from(row.kind)),
                    &target,
                )?);
            }
            Ok(allowed)
        })
    }
}

pub(super) fn collection(
    source: Source,
    targets: impl IntoIterator<Item = (KeyTargetKind, KeyResolvedTarget)>,
) -> native::Facts<Source> {
    let mut groups = BTreeMap::<KeyTargetKind, BTreeMap<KeyObjectRef, KeyResolvedTarget>>::new();
    for (family, target) in targets {
        groups
            .entry(family)
            .or_default()
            .insert(target.object, target);
    }
    native::Facts::collection(
        source,
        groups
            .into_iter()
            .map(|(family, targets)| (family, targets.into_values().collect()))
            .collect(),
    )
}

/// Actual native resources, resolved in one graph without relying on formatted choices.
pub(super) async fn resource_facts<'a>(
    conn: &mut Conn,
    rows: impl IntoIterator<Item = &'a resources::Resource>,
) -> ApiResult<Vec<(KeyTargetKind, KeyResolvedTarget)>> {
    let rows = rows.into_iter().collect::<Vec<_>>();
    let homes = rows.iter().filter_map(|row| row.home).collect::<Vec<_>>();
    let graph = Graph::load(conn, &homes)
        .await
        .map_err(ApiError::internal)?;
    rows.into_iter()
        .map(|row| {
            let target = orch_engine::operation_scope::targets::from_resource_identity(&graph, row)
                .ok_or_else(ApiError::not_found)?;
            Ok((KeyTargetKind::Object(ObjectKind::from(row.kind)), target))
        })
        .collect()
}
