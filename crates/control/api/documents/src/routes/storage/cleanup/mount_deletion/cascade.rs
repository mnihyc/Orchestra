//! The reviewed settings to switch off, independent of running attachments and data custody.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::PermissionName};
use orch_authz::Act;
use orch_settings::Place;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Places {
    pub nodes: BTreeMap<Id, ObjectKind>,
    pub environments: BTreeSet<Id>,
    pub system: bool,
}

impl Places {
    pub(super) fn node_managers(&self) -> impl Iterator<Item = (ObjectRef, Permission)> + '_ {
        use orch_settings_contract::registry::{self, Scope, Writer};
        self.nodes.iter().filter_map(|(&id, &kind)| {
            let scope = match kind {
                ObjectKind::Org => Scope::Org,
                ObjectKind::Project => Scope::Project,
                ObjectKind::Topic => Scope::Topic,
                _ => return None,
            };
            match registry::get(registry::keys::ENV_MOUNTS)?.written_by(scope)? {
                Writer::OnTheNode(permission) => Some((ObjectRef { kind, id }, permission)),
                _ => None,
            }
        })
    }
}

pub(super) async fn review(
    conn: &mut Conn,
    principal: &Principal,
    facts: &[resource_facts::RawFacts],
    mut book: Option<&mut Book>,
) -> ApiResult<(Places, HashSet<Option<ObjectRef>>)> {
    let uses: HashSet<_> = facts
        .iter()
        .flat_map(|row| &row.dependents)
        .filter(|edge| edge.relation == DependentRelation::ExplicitGiving)
        .map(|edge| edge.object)
        .collect();
    let mut places = Places::default();
    if uses.is_empty() {
        return Ok((places, HashSet::new()));
    }
    for place in &uses {
        match place {
            Some(object) if object.kind == ObjectKind::Environment => {
                places.environments.insert(object.id);
            }
            Some(object) if object.kind.node().is_some() => {
                places.nodes.insert(object.id, object.kind);
            }
            None => places.system = true,
            _ => return Err(ApiError::internal("unrecognized place enabling a mount")),
        }
    }
    let mut allowed = HashSet::new();
    let mut by_permission = HashMap::<Permission, Vec<Id>>::new();
    for (node, permission) in places.node_managers() {
        by_permission.entry(permission).or_default().push(node.id);
    }
    for (permission, ids) in by_permission {
        for node in orch_authz::nodes::holding(
            conn,
            principal,
            &ids,
            PermissionName::Exact(permission),
            Act::Use,
            Default::default(),
        )
        .await
        .map_err(ApiError::internal)?
        {
            allowed.insert(Some(ObjectRef {
                kind: node.kind.into(),
                id: node.id,
            }));
        }
    }
    if !places.environments.is_empty() {
        let ids: Vec<_> = places.environments.iter().copied().collect();
        for env in orch_authz::resources::holding(
            conn,
            principal,
            &ids,
            Permission::EnvironmentManage,
            Default::default(),
        )
        .await
        .map_err(ApiError::internal)?
        {
            allowed.insert(Some(env.object()));
        }
    }
    if places.system && principal.is_system_admin() {
        allowed.insert(None);
    }
    let key = orch_settings_contract::registry::keys::ENV_MOUNTS;
    let locked: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM settings WHERE key=$1 AND locked)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let mut refused = HashSet::new();
    for object in uses {
        if !allowed.contains(&object) {
            refused.insert(object);
            continue;
        }
        if locked {
            let place = match object {
                None => Place::System,
                Some(o) if o.kind == ObjectKind::Environment => Place::Environment(o.id),
                Some(o) => Place::Node(o.id),
            };
            if !orch_settings::resolve(conn, place, key)
                .await
                .map_err(ApiError::internal)?
                .open_to_write()
            {
                refused.insert(object);
                continue;
            }
        }
        if let Some(book) = book.as_deref_mut()
            && !book.mount_consumer(conn, object).await?
        {
            refused.insert(object);
        }
    }
    Ok((places, refused))
}
