//! Pool graphs use the resource world's real rows and native serialized writer.
use super::*;
use orch_db::queries::pools as stored;

fn edge(id: Id, child: Id) -> stored::Member {
    stored::Member {
        id,
        position: 0,
        rule: json!({"type":"pool","id":child,"kind":"model"}),
        enabled: true,
        priority: None,
        weight: None,
    }
}

#[tokio::test]
async fn a_nested_pool_cannot_introduce_a_cycle() {
    let world = World::new().await;
    let owner = person(&world, "graph-owner").await;
    let root = make_org(&world, &owner, "Graph", "graph").await.base.id;
    let a = resource(&world, &owner, root, "pool", "First").await;
    let b = resource(&world, &owner, root, "pool", "Second").await;
    let mut tx = world.db.begin().await.unwrap();
    stored::lock_graph(&mut tx).await.unwrap();
    let first = stored::lock(&mut tx, a).await.unwrap().unwrap();
    let second = stored::lock(&mut tx, b).await.unwrap().unwrap();
    stored::put_member(&mut tx, &first, &edge(orch_types::new_id(), b), Some(b))
        .await
        .unwrap();
    let refused = stored::check_child(&mut tx, &second, a).await.unwrap_err();
    assert!(
        matches!(refused, orch_db::DbError::Rule { constraint } if constraint == "pool_members_no_cycle")
    );
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn written_pool_members_remain_available_beyond_the_first_page() {
    let world = World::new().await;
    let owner = person(&world, "paged-pool-owner").await;
    let root = make_org(&world, &owner, "Paged", "paged").await.base.id;
    let id = resource(&world, &owner, root, "pool", "Models").await;
    let batch = orch_settings_contract::registry::get("pools.read_batch_size")
        .unwrap()
        .default_value()
        .unwrap()
        .as_i64()
        .unwrap();
    let mut tx = world.db.begin().await.unwrap();
    stored::lock_graph(&mut tx).await.unwrap();
    let parent = stored::lock(&mut tx, id).await.unwrap().unwrap();
    let mut expected = Vec::new();
    for position in 0..=batch {
        let member = stored::Member {
            id: orch_types::new_id(),
            position,
            rule: json!({"type":"provider_models","provider_id":orch_types::new_id(),"group":null}),
            enabled: true,
            priority: None,
            weight: None,
        };
        stored::put_member(&mut tx, &parent, &member, None)
            .await
            .unwrap();
        expected.push(member.id);
    }
    let first = stored::members(&mut tx, id, None, batch).await.unwrap();
    let last = first.last().unwrap();
    let second = stored::members(&mut tx, id, Some((last.position, last.id)), batch)
        .await
        .unwrap();
    let actual: Vec<_> = first.iter().chain(&second).map(|row| row.id).collect();
    assert_eq!(actual, expected);
    assert_eq!(second.len(), 1);
    tx.rollback().await.unwrap();
}

struct Choices {
    pools: std::collections::HashMap<Id, orch_engine::resource_choice::Pool>,
    members: std::collections::HashMap<Id, Vec<orch_engine::resource_choice::Member>>,
    leaves: Vec<orch_provider_contract::pools::Leaf>,
}

fn page<T: Clone>(
    rows: &[T],
    after: Option<&str>,
    batch: usize,
) -> orch_engine::resource_choice::Page<T> {
    let start = after
        .map(|cursor| cursor.parse::<usize>().unwrap())
        .unwrap_or_default();
    let end = (start + batch).min(rows.len());
    orch_engine::resource_choice::Page {
        items: rows[start..end].to_vec(),
        next: (end < rows.len()).then(|| end.to_string()),
    }
}

#[async_trait::async_trait]
impl orch_engine::resource_choice::Source for Choices {
    async fn pool(
        &mut self,
        id: Id,
    ) -> Result<Option<orch_engine::resource_choice::Pool>, orch_engine::EngineError> {
        Ok(self.pools.get(&id).cloned())
    }
    async fn members(
        &mut self,
        id: Id,
        after: Option<&str>,
        batch: usize,
    ) -> Result<
        orch_engine::resource_choice::Page<orch_engine::resource_choice::Member>,
        orch_engine::EngineError,
    > {
        Ok(page(
            self.members.get(&id).map(Vec::as_slice).unwrap_or_default(),
            after,
            batch,
        ))
    }
    async fn expand(
        &mut self,
        _pool: Id,
        rule: &orch_provider_contract::pools::MemberRule,
        after: Option<&str>,
        batch: usize,
    ) -> Result<
        orch_engine::resource_choice::Page<orch_provider_contract::pools::Leaf>,
        orch_engine::EngineError,
    > {
        let rows = match rule {
            orch_provider_contract::pools::MemberRule::Single { resource } => {
                vec![resource.clone()]
            }
            _ => self.leaves.clone(),
        };
        Ok(page(&rows, after, batch))
    }
    async fn first_occurrence(
        &mut self,
        _pool: Id,
        _member: Id,
        _leaf: &orch_provider_contract::pools::Leaf,
    ) -> Result<bool, orch_engine::EngineError> {
        Ok(true)
    }
    async fn reason(
        &mut self,
        _leaf: &orch_provider_contract::pools::Leaf,
        _path: &[orch_engine::resource_choice::Hop],
    ) -> Result<Option<String>, orch_engine::EngineError> {
        Ok(None)
    }
}

#[tokio::test]
async fn one_resource_choice_walk_preserves_nested_policies_and_every_supplier_page() {
    use orch_engine::resource_choice::{Defaults, Event, Member, Pool, Resolver};
    use orch_provider_contract::pools::{Definition, Kind, Leaf, MemberRule, Policy, Target};
    let root = orch_types::new_id();
    let child = orch_types::new_id();
    let definition = |policy| Definition {
        kind: Kind::Host,
        enabled: true,
        policy: Some(policy),
        lend_members: false,
        members: vec![],
        config: json!({}),
    };
    // A one-row supplier window forces continuation even in this small native-fact fixture.
    let defaults = Defaults {
        max_depth: 3,
        batch_size: 1,
        priority: 0,
        weight: "1".into(),
    };
    let first = Leaf::Host {
        id: orch_types::new_id(),
    };
    let second = Leaf::Host {
        id: orch_types::new_id(),
    };
    let third = Leaf::Host {
        id: orch_types::new_id(),
    };
    let member = |position, rule| Member {
        id: orch_types::new_id(),
        position,
        rule,
        enabled: true,
        priority: None,
        weight: None,
    };
    let source = Choices {
        pools: std::collections::HashMap::from([
            (
                root,
                Pool {
                    id: root,
                    revision: 1,
                    definition: definition(Policy::Weighted),
                    defaults: defaults.clone(),
                    reason: None,
                },
            ),
            (
                child,
                Pool {
                    id: child,
                    revision: 1,
                    definition: definition(Policy::RoundRobin),
                    defaults,
                    reason: None,
                },
            ),
        ]),
        members: std::collections::HashMap::from([
            (
                root,
                vec![
                    member(
                        0,
                        MemberRule::Pool {
                            id: child,
                            kind: Kind::Host,
                        },
                    ),
                    member(
                        1,
                        MemberRule::Matching {
                            kind: Kind::Host,
                            home_id: None,
                            tag_id: None,
                            owner_id: None,
                            machine_id: None,
                        },
                    ),
                ],
            ),
            (
                child,
                vec![member(
                    0,
                    MemberRule::Single {
                        resource: first.clone(),
                    },
                )],
            ),
        ]),
        leaves: vec![second.clone(), third.clone()],
    };
    let mut resolver = Resolver::new(
        source,
        &Target::Pool {
            id: root,
            kind: Kind::Host,
        },
        Kind::Host,
    )
    .await
    .unwrap();
    let mut policies = Vec::new();
    let mut leaves = Vec::new();
    let mut depths = Vec::new();
    while let Some(event) = resolver.next().await.unwrap() {
        match event {
            Event::Enter { pool, .. } => policies.push(pool.definition.policy.unwrap()),
            Event::Candidate { leaf, path, reason } => {
                assert!(reason.is_none());
                leaves.push(leaf);
                depths.push(path.len());
            }
            Event::Skipped { reason, .. } => {
                panic!("an eligible fixture member was skipped: {reason}")
            }
            Event::Leave { .. } => {}
        }
    }
    assert_eq!(policies, vec![Policy::Weighted, Policy::RoundRobin]);
    assert_eq!(leaves, vec![first, second, third]);
    assert_eq!(depths, vec![2, 1, 1]);
}

#[tokio::test]
async fn a_pool_is_created_as_an_ordinary_resource_with_its_actual_owner() {
    use orch_api_contract::{
        access::NewResourceAccess,
        pools::{PoolCreate, PoolCreateBody, PoolGet},
    };
    use orch_provider_contract::pools::{Definition, Kind, Policy};
    let world = World::new().await;
    let owner = person(&world, "ordinary-pool-owner").await;
    let root = make_org(&world, &owner, "Pool home", "pool-home")
        .await
        .base
        .id;
    let receipt = world
        .app
        .call(PoolCreate, &[])
        .token(&owner.token)
        .body(&PoolCreateBody {
            home_id: root,
            name: "Build hosts".into(),
            description: "Shared build placement".into(),
            access: NewResourceAccess::OnlyMe,
            definition: Definition {
                kind: Kind::Host,
                enabled: true,
                policy: Some(Policy::Ordered),
                lend_members: false,
                members: vec![],
                config: json!({}),
            },
        })
        .send()
        .await
        .ok();
    let pool = world
        .app
        .call(PoolGet, &[&receipt.id])
        .token(&owner.token)
        .send()
        .await
        .ok();
    assert_eq!(pool.id, receipt.id);
    assert_eq!(pool.owner.as_ref().map(|who| who.id), Some(owner.id()));
    assert_eq!(pool.name, "Build hosts");
    assert_eq!(pool.kind, Kind::Host);
    assert!(pool.enabled);
    assert!(
        pool.access
            .permissions
            .iter()
            .any(|permission| permission == "pool.manage")
    );
}
