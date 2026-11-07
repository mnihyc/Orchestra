//! Stored native storage identities are factual operands, never retained authorization.
use crate::{
    EngineError,
    operation_scope::{sources::Facts, targets},
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind},
    permissions::{NodePermission, PermissionName},
};
use orch_api_contract::storage_sources::Source;
use orch_authz::{Act, Principal};
use orch_db::{Conn, queries};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A business family and the actual native subject it selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reference {
    /// The operation facet's business family, including canonical machine facades.
    pub family: KeyTargetKind,
    /// A real stored identity selected by the native producer.
    pub object: KeyObjectRef,
}
/// Actual typed sources kept beside a private native preview/recipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sources {
    /// The report/cleanup namespace, resolved by the original native input.
    pub namespace: Reference,
    /// The exact selected retained governors.
    pub records: Vec<Reference>,
    /// Actual native deletion destinations.
    pub physical: Vec<Reference>,
    /// Existing reference names, separate from the retained records' intrinsic metadata.
    pub names: Vec<Reference>,
    /// Physical machines retain their original canonical permission-page association.
    pub machines: Vec<(Id, KeyObjectRef)>,
}
fn target_error(error: targets::TargetError) -> Result<Option<KeyResolvedTarget>, EngineError> {
    match error {
        targets::TargetError::Missing => Ok(None),
        targets::TargetError::Db(error) => Err(error.into()),
        targets::TargetError::Authz(error) => Err(error.into()),
    }
}
async fn resolve(
    conn: &mut Conn,
    object: KeyObjectRef,
) -> Result<Option<KeyResolvedTarget>, EngineError> {
    let result = match (object.kind, object.id) {
        (KeyTargetKind::Identity(KeyIdentityKind::System), None) => {
            return Ok(Some(targets::system()));
        }
        (KeyTargetKind::Object(kind), Some(id)) if kind.node().is_some() => {
            targets::node_identity(conn, id).await
        }
        (KeyTargetKind::Object(kind), Some(id)) => {
            if kind.is_document() {
                targets::page_object(conn, ObjectRef { kind, id }).await
            } else {
                targets::resource_identity(conn, ObjectRef { kind, id }).await
            }
        }
        (KeyTargetKind::Identity(KeyIdentityKind::Account), Some(id)) => {
            targets::account(conn, id).await
        }
        (KeyTargetKind::Identity(KeyIdentityKind::Export), Some(id)) => {
            let Some(row) = queries::exports::get(conn, id).await? else {
                return Ok(None);
            };
            targets::from_export(conn, &row).await
        }
        (KeyTargetKind::Identity(KeyIdentityKind::Upload), Some(id)) => {
            let Some(row) = queries::storage::session(conn, id).await? else {
                return Ok(None);
            };
            let mut target = match targets::account(conn, row.user_id).await {
                Ok(target) => target,
                Err(error) => return target_error(error),
            };
            target.object = object;
            return Ok(Some(target));
        }
        _ => return Ok(None),
    };
    match result {
        Ok(target) => Ok((target.object == object).then_some(target)),
        Err(error) => target_error(error),
    }
}
fn grouped<'a>(
    references: impl Iterator<Item = &'a Reference>,
    current: &HashMap<KeyObjectRef, KeyResolvedTarget>,
) -> Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for reference in references {
        if let Some(target) = current.get(&reference.object) {
            groups
                .entry(reference.family)
                .or_default()
                .push(target.clone());
        }
    }
    groups.into_iter().collect()
}
impl Sources {
    /// Reload current identities and ancestry from native rows; missing original subjects close disclosure/work.
    pub async fn facts(&self, conn: &mut Conn) -> Result<Option<Vec<Facts<Source>>>, EngineError> {
        let mut current = HashMap::new();
        for reference in std::iter::once(&self.namespace)
            .chain(&self.records)
            .chain(&self.physical)
            .chain(&self.names)
        {
            if current.contains_key(&reference.object) {
                continue;
            }
            let Some(target) = resolve(conn, reference.object).await? else {
                return Ok(None);
            };
            current.insert(reference.object, target);
        }
        let machines = targets::machine_identities(
            conn,
            &self.machines.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        )
        .await;
        let machines = match machines {
            Ok(machines) => machines,
            Err(targets::TargetError::Missing) => return Ok(None),
            Err(targets::TargetError::Db(error)) => return Err(error.into()),
            Err(targets::TargetError::Authz(error)) => return Err(error.into()),
        };
        if self.machines.iter().any(|(id, expected)| {
            machines
                .get(id)
                .is_none_or(|target| target.object != *expected)
        }) {
            return Ok(None);
        }
        let Some(namespace) = current.get(&self.namespace.object) else {
            return Ok(None);
        };
        Ok(Some(vec![
            Facts::one(Source::Namespace, self.namespace.family, namespace.clone()),
            Facts::collection(Source::Records, grouped(self.records.iter(), &current)),
            Facts::collection(Source::Physical, grouped(self.physical.iter(), &current)),
            Facts::collection(
                Source::Content,
                grouped(self.records.iter().chain(&self.physical), &current),
            ),
            Facts::collection(Source::Names, grouped(self.names.iter(), &current)),
        ]))
    }

    /// Keep the native current ordinary reading gate when showing a saved private preview/job.
    pub async fn may_read(
        &self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<bool, EngineError> {
        if self.namespace.object.kind == KeyTargetKind::Identity(KeyIdentityKind::System)
            && !principal.is_system_admin()
        {
            return Ok(false);
        }
        let objects = self
            .records
            .iter()
            .chain(&self.physical)
            .chain(&self.names)
            .filter_map(
                |reference| match (reference.object.kind, reference.object.id) {
                    (KeyTargetKind::Object(kind), Some(id)) => Some(ObjectRef { kind, id }),
                    _ => None,
                },
            )
            .collect::<Vec<_>>();
        let ids = objects
            .iter()
            .filter(|object| object.kind.resource().is_some())
            .map(|object| object.id)
            .collect::<Vec<_>>();
        let resources = crate::resource_facts::authorities(conn, principal, &ids).await?;
        if objects
            .iter()
            .filter(|object| object.kind.resource().is_some())
            .any(|object| {
                !resources
                    .iter()
                    .any(|row| row.resource.object() == *object && row.readable)
            })
        {
            return Ok(false);
        }
        let ids = objects
            .iter()
            .filter(|object| object.kind.node().is_some())
            .map(|object| object.id)
            .collect::<Vec<_>>();
        let nodes = orch_authz::nodes::holding(
            conn,
            principal,
            &ids,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Default::default(),
        )
        .await?;
        if ids.iter().any(|id| !nodes.iter().any(|row| row.id == *id)) {
            return Ok(false);
        }
        let ids = objects
            .iter()
            .filter(|object| object.kind.is_document())
            .map(|object| object.id)
            .collect::<Vec<_>>();
        let documents =
            orch_authz::documents::listed(conn, principal, &ids, Default::default()).await?;
        if ids.iter().any(|id| {
            !documents.iter().any(|row| {
                row.document.id == *id
                    && !row.document.deleted
                    && row.holds(orch_access_contract::permissions::Permission::DocView)
            })
        }) {
            return Ok(false);
        }
        Ok(true)
    }
}
