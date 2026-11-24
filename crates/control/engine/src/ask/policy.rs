//! Each contributing Ask policy restricts the backend that can serve an answer.
use orch_api_contract::ask::AskTarget as Entry;
use orch_provider_contract::{
    capabilities::BackendRef,
    pools::{Kind, Leaf, Target},
};
use orch_types::Id;

/// Which pools in each contributing policy must still contain this exact backend.
fn required_pools(layers: &[Vec<Entry>], backend: &BackendRef) -> Option<Vec<Vec<Id>>> {
    if layers.is_empty() {
        return None;
    }
    let mut clauses = Vec::new();
    for layer in layers {
        let mut alternatives = Vec::new();
        let mut exact = false;
        for entry in layer {
            match entry {
                Entry::Backend(entry) => exact |= entry.backend == *backend,
                Entry::Pool {
                    target: Target::Single { resource },
                } => {
                    exact |= includes(resource, backend);
                }
                Entry::Pool {
                    target:
                        Target::Pool {
                            kind: Kind::Model,
                            id,
                        },
                } => {
                    alternatives.push(*id);
                }
                Entry::Pool { .. } => {}
            }
        }
        if !exact {
            if alternatives.is_empty() {
                return None;
            }
            clauses.push(alternatives);
        }
    }
    Some(clauses)
}

pub(super) fn includes(leaf: &Leaf, backend: &BackendRef) -> bool {
    match leaf {
        Leaf::Model { backend: member } => {
            if member == backend {
                return true;
            }
            // A provider's ordinary model listing supplies an API offering. An exact
            // program with its own placement or options can only match that program.
            member.route == "generation.model"
                && member.extensions.is_empty()
                && (member.config.is_null()
                    || member
                        .config
                        .as_object()
                        .is_some_and(|config| config.is_empty()))
                && backend.route == orch_api_contract::ask::API_ROUTE
                && backend.extensions.is_empty()
                && backend.config.is_null()
                && member.offering.is_some()
                && member.offering == backend.offering
        }
        _ => false,
    }
}

pub(super) struct Policy {
    layers: Vec<Vec<Entry>>,
}

impl Policy {
    pub(super) async fn load(
        conn: &mut orch_db::Conn,
        root: Id,
    ) -> Result<Self, crate::EngineError> {
        use orch_settings_contract::registry::keys;
        let view = orch_settings::resolve(conn, orch_settings::Place::Node(root), keys::ASK_MODELS)
            .await?;
        let layers = view
            .parts()
            .iter()
            .map(|part| serde_json::from_value(part.value.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { layers })
    }

    pub(super) async fn entries(
        &self,
        conn: &mut orch_db::Conn,
        principal: &orch_authz::Principal,
    ) -> Result<Vec<Entry>, crate::EngineError> {
        let mut entries = Vec::new();
        if let Some(first) = self.layers.first() {
            for entry in first {
                if entries.contains(entry) {
                    continue;
                }
                match entry {
                    Entry::Backend(identity)
                        if !self
                            .permits(conn, principal, &identity.backend, None)
                            .await? =>
                    {
                        continue;
                    }
                    Entry::Pool { target }
                        if !matches!(
                            target,
                            Target::Pool {
                                kind: Kind::Model,
                                ..
                            }
                        ) =>
                    {
                        continue;
                    }
                    _ => entries.push(entry.clone()),
                }
            }
        }
        Ok(entries)
    }

    pub(super) fn names_model_pool(&self, id: Id) -> bool {
        self.layers.iter().flatten().any(|entry|
            matches!(entry, Entry::Pool {target: Target::Pool {kind: Kind::Model, id: listed}} if *listed == id))
    }

    /// A live path that is already being checked by the shared resolver satisfies
    /// membership in its root pool without another traversal for every candidate.
    pub(super) async fn permits(
        &self,
        conn: &mut orch_db::Conn,
        principal: &orch_authz::Principal,
        backend: &BackendRef,
        current_pool: Option<Id>,
    ) -> Result<bool, crate::EngineError> {
        if current_pool.is_some_and(|pool| !self.names_model_pool(pool)) {
            return Ok(false);
        }
        let Some(clauses) = required_pools(&self.layers, backend) else {
            return Ok(false);
        };
        let mut seen = std::collections::HashMap::new();
        for alternatives in clauses {
            if current_pool.is_some_and(|id| alternatives.contains(&id)) {
                continue;
            }
            let mut included = false;
            for pool in alternatives {
                let found = if let Some(found) = seen.get(&pool) {
                    *found
                } else {
                    let found = pool_contains(conn, principal, pool, backend).await?;
                    seen.insert(pool, found);
                    found
                };
                if found {
                    included = true;
                    break;
                }
            }
            if !included {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

struct Membership<'a>(&'a BackendRef);

#[async_trait::async_trait]
impl crate::resource_choice::database::Requirements for Membership<'_> {
    async fn model(
        &mut self,
        _: &mut orch_db::Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, crate::EngineError> {
        Ok(Leaf::generation_model(provider, model))
    }
    async fn reason(
        &mut self,
        _: &mut orch_db::Conn,
        _: &orch_authz::Principal,
        leaf: &Leaf,
        _: &[crate::resource_choice::Hop],
    ) -> Result<Option<String>, crate::EngineError> {
        Ok((!includes(leaf, self.0)).then(|| "not_in_list".into()))
    }
}

async fn pool_contains(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    pool: Id,
    backend: &BackendRef,
) -> Result<bool, crate::EngineError> {
    use crate::resource_choice::{Event, Resolver, database::Database};
    let source = Database::new(conn, principal, Membership(backend));
    let target = Target::Pool {
        kind: Kind::Model,
        id: pool,
    };
    let mut members = Resolver::new(source, &target, Kind::Model).await?;
    while let Some(event) = members.next().await? {
        if matches!(event, Event::Candidate { reason: None, .. }) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::ask::AskModelIdentity;
    use orch_types::new_id;

    #[test]
    fn neither_a_parent_nor_a_child_pool_can_widen_another_scopes_exact_ask_model() {
        let first = AskModelIdentity::api(new_id(), "first".into());
        let other = AskModelIdentity::api(new_id(), "other".into());
        let pool = new_id();
        let direct = || vec![Entry::Backend(first.clone())];
        let dynamic = || {
            vec![Entry::Pool {
                target: Target::Pool {
                    kind: Kind::Model,
                    id: pool,
                },
            }]
        };
        for layers in [[direct(), dynamic()], [dynamic(), direct()]] {
            assert_eq!(required_pools(&layers, &other.backend), None);
            assert_eq!(
                required_pools(&layers, &first.backend),
                Some(vec![vec![pool]])
            );
        }
        assert_eq!(required_pools(&[dynamic(), vec![]], &first.backend), None);
        assert_eq!(required_pools(&[], &first.backend), None);
    }

    #[test]
    fn an_api_model_group_does_not_authorize_a_private_program_at_the_same_account_and_model() {
        let identity = AskModelIdentity::api(new_id(), "same".into());
        let mut listing = identity.backend.clone();
        listing.route = "generation.model".into();
        listing.config = serde_json::json!({});
        let leaf = Leaf::Model { backend: listing };
        assert!(includes(&leaf, &identity.backend));
        let mut program = identity.backend;
        program.route = orch_api_contract::ask::PROGRAM_ROUTE.into();
        program.config = serde_json::json!({"environment_id":new_id()});
        assert!(!includes(&leaf, &program));
        assert!(includes(
            &Leaf::Model {
                backend: program.clone()
            },
            &program
        ));
    }
}
