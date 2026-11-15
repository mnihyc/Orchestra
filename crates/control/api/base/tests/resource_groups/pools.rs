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

#[derive(Clone)]
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
    fn member_cursor(
        &self,
        member: &orch_engine::resource_choice::Member,
    ) -> Result<String, orch_engine::EngineError> {
        Ok((member.position + 1).to_string())
    }
    fn leaf_cursor(
        &self,
        rule: &orch_provider_contract::pools::MemberRule,
        leaf: &orch_provider_contract::pools::Leaf,
    ) -> Result<Option<String>, orch_engine::EngineError> {
        if matches!(
            rule,
            orch_provider_contract::pools::MemberRule::Single { .. }
        ) {
            return Ok(None);
        }
        Ok(self
            .leaves
            .iter()
            .position(|current| current == leaf)
            .map(|index| (index + 1).to_string()))
    }
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
    let saved = serde_json::to_vec(&resolver.checkpoint()).unwrap();
    let source = resolver.into_source();
    let mut resolver = Resolver::resume(source, serde_json::from_slice(&saved).unwrap())
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

#[tokio::test]
async fn a_pool_made_from_a_home_group_keeps_the_exact_live_group() {
    use orch_provider_contract::pools::{DisplayGroup, Kind, Leaf, MemberRule};
    let world = World::new().await;
    let owner = person(&world, "exact-group-owner").await;
    let root = make_org(&world, &owner, "Exact home", "exact-home")
        .await
        .base
        .id;
    let project = make_project(&world, &owner, root, "Child home")
        .await
        .base
        .id;
    let first = resource(&world, &owner, root, "provider", "At home").await;
    let child = resource(&world, &owner, project, "provider", "In child").await;
    let rule = MemberRule::Grouping {
        kind: Kind::Provider,
        group: DisplayGroup::Home { id: Some(root) },
    };
    let mut conn = world.conn().await;
    let selected = stored::resource_candidates(&mut conn, root, &rule, None, 2)
        .await
        .unwrap();
    assert!(selected.contains(&first));
    assert!(!selected.contains(&child));
    assert!(
        stored::rule_contains(&mut conn, root, &rule, &Leaf::Provider { id: first })
            .await
            .unwrap()
    );
    assert!(
        !stored::rule_contains(&mut conn, root, &rule, &Leaf::Provider { id: child })
            .await
            .unwrap()
    );
    let later = resource(&world, &owner, root, "provider", "Added later").await;
    assert!(
        stored::rule_contains(&mut conn, root, &rule, &Leaf::Provider { id: later })
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn a_lender_can_withdraw_a_consent_after_losing_use_and_replay_its_first_write() {
    use orch_access_contract::{
        decisions::ObjectRef,
        permissions::ObjectKind,
        pools::{LendingMatcher, LendingScope},
    };
    use orch_api_contract::pools::{
        PoolLendingList, PoolLendingListBody, PoolLendingPut, PoolLendingPutBody,
    };
    let world = World::new().await;
    let owner = person(&world, "withdrawing-lender").await;
    let root = make_org(&world, &owner, "Lending", "lending").await.base.id;
    let provider = resource(&world, &owner, root, "provider", "Lent provider").await;
    let pool = resource(&world, &owner, root, "pool", "Borrowing pool").await;
    let entry = orch_types::new_id();
    let scope = LendingScope::Member {
        object: ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    };
    let body = PoolLendingPutBody {
        scope: scope.clone(),
        expected_revision: None,
        matcher: LendingMatcher::Pool { pool_id: pool },
        options: json!({}),
        show_names: false,
        enabled: true,
        expires_at: None,
    };
    let accepted = world
        .app
        .call(PoolLendingPut, &[&entry])
        .token(&owner.token)
        .body(&body)
        .send()
        .await
        .ok();
    let replay = world
        .app
        .call(PoolLendingPut, &[&entry])
        .token(&owner.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(accepted, replay);
    let page = world
        .app
        .call(PoolLendingList, &[])
        .token(&owner.token)
        .body(&PoolLendingListBody {
            scope: scope.clone(),
            after_id: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].can_revoke);
    let mut conn = world.conn().await;
    sqlx::query("UPDATE providers SET owner_id=NULL WHERE id=$1")
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DELETE FROM permission_entries WHERE object_id=$1")
        .bind(provider)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let revoked = world
        .app
        .call(PoolLendingPut, &[&entry])
        .token(&owner.token)
        .body(&PoolLendingPutBody {
            enabled: false,
            expected_revision: Some(accepted.revision),
            ..body
        })
        .send()
        .await
        .ok();
    assert_eq!(revoked.revision, accepted.revision + 1);
    let mut conn = world.conn().await;
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT removed_at IS NOT NULL FROM pool_lending WHERE id=$1"
        )
        .bind(entry)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn a_disabled_environment_pool_keeps_its_live_location_set_without_granting_use() {
    use orch_access_contract::whereabouts::{WhereKind, WhereName};
    let world = World::new().await;
    let owner = person(&world, "location-pool-owner").await;
    let root = make_org(&world, &owner, "Locations", "locations")
        .await
        .base
        .id;
    let environment = resource(&world, &owner, root, "environment", "First location").await;
    let pool = resource(&world, &owner, root, "pool", "Location set").await;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE pools SET kind='environment',definition=jsonb_set(jsonb_set(definition,'{kind}','\"environment\"'),'{enabled}','false') WHERE id=$1").bind(pool).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,0,$3)")
        .bind(pool)
        .bind(orch_types::new_id())
        .bind(json!({"type":"grouping","kind":"environment","group":{"by":"home","id":root}}))
        .execute(&mut *conn)
        .await
        .unwrap();
    let named = WhereName {
        kind: WhereKind::EnvironmentPool,
        id: pool,
    };
    assert!(
        named.covers(
            &orch_authz::usable::place_of(&mut conn, environment)
                .await
                .unwrap()
        )
    );
    drop(conn);
    let later = resource(&world, &owner, root, "environment", "Later location").await;
    let mut conn = world.conn().await;
    assert!(
        named.covers(
            &orch_authz::usable::place_of(&mut conn, later)
                .await
                .unwrap()
        )
    );
    sqlx::query("UPDATE pools SET deleted_at=now() WHERE id=$1")
        .bind(pool)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        !named.covers(
            &orch_authz::usable::place_of(&mut conn, later)
                .await
                .unwrap()
        )
    );
}

#[tokio::test]
async fn a_managed_pool_is_a_typed_inventory_and_name_search_resource() {
    use orch_api_contract::search::{NameKind, NameSearchList, NameSearchListQuery};
    let world = World::new().await;
    let owner = person(&world, "discovering-pools").await;
    let root = make_org(&world, &owner, "Pool discovery", "pool-discovery")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Discoverable build pool").await;
    let inventory = world
        .app
        .call(EverythingList, &[])
        .token(&owner.token)
        .query(&EverythingListQuery {
            parent: Some("resources".into()),
            kinds: Some("pool".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        inventory
            .items
            .iter()
            .any(|item| item.id == pool.to_string())
    );
    let names = world
        .app
        .call(NameSearchList, &[])
        .token(&owner.token)
        .query(&NameSearchListQuery {
            q: Some("Discoverable".into()),
            kinds: Some("pool".into()),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(
        names
            .items
            .iter()
            .any(|item| item.id == pool && item.kind == NameKind::Pool)
    );
}

#[tokio::test]
async fn a_pool_read_key_discloses_a_member_only_with_its_own_read_source() {
    use orch_api_contract::pools::{PoolMembersGet, PoolMembersQuery};
    let world = World::new().await;
    let owner = person(&world, "scoped-pool-reader").await;
    let root = make_org(&world, &owner, "Scoped pools", "scoped-pools")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Readable pool").await;
    let provider = resource(&world, &owner, root, "provider", "Private member").await;
    let member = orch_types::new_id();
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,0,$3)").bind(pool).bind(member).bind(json!({"type":"single","resource":{"kind":"model","backend":{"route":"generation.service-model","offering":{"provider":provider,"offering":"Example"},"config":{}}}})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let narrow = key(
        &world,
        &owner,
        vec![rule("PoolMembersGet", "pool", pool, &["pool"], "read")],
    )
    .await;
    let page = world
        .app
        .call(PoolMembersGet, &[&pool])
        .token(&narrow)
        .query(&PoolMembersQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].rule.is_none());
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains(&provider.to_string())
    );
    let wide = key(
        &world,
        &owner,
        vec![
            rule("PoolMembersGet", "pool", pool, &["pool"], "read"),
            rule(
                "PoolMembersGet",
                "provider",
                provider,
                &["provider"],
                "read",
            ),
        ],
    )
    .await;
    let page = world
        .app
        .call(PoolMembersGet, &[&pool])
        .token(&wide)
        .query(&PoolMembersQuery::default())
        .send()
        .await
        .ok();
    assert!(page.items[0].rule.is_some());
}

#[tokio::test]
async fn a_pool_manager_can_disable_a_hidden_row_without_replacing_its_target() {
    use orch_api_contract::pools::{PoolMemberMetadata, PoolMemberMetadataBody};
    let world = World::new().await;
    let owner = person(&world, "blind-member-manager").await;
    let root = make_org(&world, &owner, "Blind edits", "blind-edits")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Managed pool").await;
    let member = orch_types::new_id();
    let original =
        json!({"type":"provider_models","provider_id":orch_types::new_id(),"group":"private"});
    let mut conn = world.conn().await;
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,0,$3)")
        .bind(pool)
        .bind(member)
        .bind(&original)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let key = key(
        &world,
        &owner,
        vec![rule(
            "PoolMemberMetadata",
            "pool",
            pool,
            &["pool"],
            "manage",
        )],
    )
    .await;
    world
        .app
        .call(PoolMemberMetadata, &[&pool, &member])
        .token(&key)
        .body(&PoolMemberMetadataBody {
            expected_revision: 1,
            enabled: false,
            priority: None,
            weight: None,
            position: 0,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    let stored = stored::member(&mut conn, pool, member)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.rule, original);
    assert!(!stored.enabled);
}

#[tokio::test]
async fn new_pool_rows_append_beyond_unloaded_rows_and_edits_keep_their_position() {
    use orch_api_contract::pools::{PoolMemberPut, PoolMemberPutBody};
    use orch_provider_contract::pools::{Member, MemberRule};
    let world = World::new().await;
    let owner = person(&world, "appending-members").await;
    let root = make_org(&world, &owner, "Append rules", "append-rules")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Ordered members").await;
    let provider = resource(&world, &owner, root, "provider", "Model supplier").await;
    let first = Member {
        id: orch_types::new_id(),
        rule: MemberRule::ProviderModels {
            provider_id: provider,
            group: Some("first".into()),
        },
        enabled: true,
        priority: None,
        weight: None,
    };
    let second = Member {
        id: orch_types::new_id(),
        rule: MemberRule::ProviderModels {
            provider_id: provider,
            group: Some("second".into()),
        },
        enabled: true,
        priority: None,
        weight: None,
    };
    for (revision, member) in [(1, first.clone()), (2, second.clone())] {
        world
            .app
            .call(PoolMemberPut, &[&pool, &member.id])
            .token(&owner.token)
            .body(&PoolMemberPutBody {
                expected_revision: revision,
                member,
                position: None,
            })
            .send()
            .await
            .ok();
    }
    world
        .app
        .call(PoolMemberPut, &[&pool, &first.id])
        .token(&owner.token)
        .body(&PoolMemberPutBody {
            expected_revision: 3,
            member: Member {
                enabled: false,
                ..first.clone()
            },
            position: None,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    assert_eq!(
        stored::member(&mut conn, pool, first.id)
            .await
            .unwrap()
            .unwrap()
            .position,
        0
    );
    assert_eq!(
        stored::member(&mut conn, pool, second.id)
            .await
            .unwrap()
            .unwrap()
            .position,
        1
    );
    assert!(sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM audit_log WHERE object_id=$1 AND action='pool.member_updated' AND details->>'revision'='4')").bind(pool).fetch_one(&mut *conn).await.unwrap());
}

#[tokio::test]
async fn deleting_a_pool_keeps_its_member_resources_and_refuses_a_saved_parent_choice() {
    use orch_api_contract::pools::{PoolDelete, PoolDeleteBody};
    let world = World::new().await;
    let owner = person(&world, "deleting-pool-owner").await;
    let root = make_org(&world, &owner, "Delete pools", "delete-pools")
        .await
        .base
        .id;
    let parent = resource(&world, &owner, root, "pool", "Parent pool").await;
    let child = resource(&world, &owner, root, "pool", "Child pool").await;
    let provider = resource(&world, &owner, root, "provider", "Kept member").await;
    let edge_id = orch_types::new_id();
    let mut tx = world.db.begin().await.unwrap();
    stored::lock_graph(&mut tx).await.unwrap();
    let parent_row = stored::lock(&mut tx, parent).await.unwrap().unwrap();
    stored::put_member(&mut tx, &parent_row, &edge(edge_id, child), Some(child))
        .await
        .unwrap();
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,0,$3)")
        .bind(child)
        .bind(orch_types::new_id())
        .bind(json!({"type":"provider_models","provider_id":provider,"group":null}))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let refused = world
        .app
        .call(PoolDelete, &[&child])
        .token(&owner.token)
        .body(&PoolDeleteBody {
            expected_revision: 1,
        })
        .send_bytes()
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    world
        .app
        .call(PoolDelete, &[&parent])
        .token(&owner.token)
        .body(&PoolDeleteBody {
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    world
        .app
        .call(PoolDelete, &[&child])
        .token(&owner.token)
        .body(&PoolDeleteBody {
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    assert!(
        stored::get(&mut conn, parent)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .is_some()
    );
    drop(conn);
    assert!(row(&world, "provider", provider).await["deleted_at"].is_null());
}

#[tokio::test]
async fn a_pool_continuation_keeps_positions_instead_of_unread_member_payloads() {
    use orch_engine::resource_choice::{Defaults, Event, Member, Pool, Resolver};
    use orch_provider_contract::pools::{Definition, Kind, Leaf, MemberRule, Policy, Target};
    let root = orch_types::new_id();
    let batch = orch_settings_contract::registry::get("pools.read_batch_size")
        .unwrap()
        .default_value()
        .unwrap()
        .as_u64()
        .unwrap();
    let batch = usize::try_from(batch).unwrap();
    let leaves: Vec<_> = (0..=batch)
        .map(|_| Leaf::Host {
            id: orch_types::new_id(),
        })
        .collect();
    let source = Choices {
        pools: std::collections::HashMap::from([(
            root,
            Pool {
                id: root,
                revision: 1,
                definition: Definition {
                    kind: Kind::Host,
                    enabled: true,
                    policy: Some(Policy::Ordered),
                    lend_members: false,
                    members: vec![],
                    config: json!({}),
                },
                defaults: Defaults {
                    max_depth: 3,
                    batch_size: batch,
                    priority: 0,
                    weight: "1".into(),
                },
                reason: None,
            },
        )]),
        members: std::collections::HashMap::from([(
            root,
            vec![Member {
                id: orch_types::new_id(),
                position: 0,
                rule: MemberRule::Matching {
                    kind: Kind::Host,
                    home_id: None,
                    tag_id: None,
                    owner_id: None,
                    machine_id: None,
                },
                enabled: true,
                priority: None,
                weight: None,
            }],
        )]),
        leaves: leaves.clone(),
    };
    let mut walk = Resolver::new(
        source,
        &Target::Pool {
            id: root,
            kind: Kind::Host,
        },
        Kind::Host,
    )
    .await
    .unwrap();
    assert!(matches!(
        walk.next().await.unwrap(),
        Some(Event::Enter { .. })
    ));
    assert!(
        matches!(walk.next().await.unwrap(),Some(Event::Candidate{leaf,..}) if leaf==leaves[0])
    );
    let saved = serde_json::to_string(&walk.checkpoint()).unwrap();
    assert!(!saved.contains(&match &leaves[1] {
        Leaf::Host { id } => id.to_string(),
        _ => unreachable!(),
    }));
    let source = walk.into_source();
    let mut resumed = Resolver::resume(source, serde_json::from_str(&saved).unwrap())
        .await
        .unwrap();
    for expected in leaves.iter().skip(1) {
        assert!(
            matches!(resumed.next().await.unwrap(),Some(Event::Candidate{leaf,..}) if &leaf==expected)
        );
    }
}
