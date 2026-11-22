//! Reading recorded choices cannot reserve another member or retain private names.
use super::*;
use orch_api_contract::pools::{
    PoolChoiceDetailsGet, PoolChoiceDetailsQuery, PoolChoicesGet, PoolChoicesQuery,
};

async fn read_id(world: &World, choice: Id) -> Id {
    sqlx::query_scalar("SELECT read_id FROM pool_choices WHERE id=$1")
        .bind(choice)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap()
}

#[tokio::test]
async fn reconnecting_a_paged_choice_inspector_never_reserves_another_member() {
    let world = World::new().await;
    let owner = person(&world, "choice-history-owner").await;
    let root = make_org(&world, &owner, "Choice history", "choice-history")
        .await
        .base
        .id;
    let (pool, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    let first = choose(&world, &owner, pool, new_id()).await;
    world.advance(chrono::Duration::milliseconds(1));
    let second = choose(&world, &owner, pool, new_id()).await;
    let first_read = read_id(&world, first.id).await;
    let second_read = read_id(&world, second.id).await;
    assert_ne!(
        first_read, first.id,
        "a read identity cannot disclose the native producer's ID"
    );
    let mut conn = world.conn().await;
    sqlx::query("UPDATE pools SET definition=jsonb_set(definition,'{config,pools.read_batch_size}','1') WHERE id=$1")
        .bind(pool).execute(&mut *conn).await.unwrap();
    drop(conn);
    let query = PoolChoicesQuery::default();
    for _ in 0..2 {
        let page = world
            .app
            .call(PoolChoicesGet, &[&pool])
            .token(&owner.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, second_read);
        assert_eq!(page.items[0].resource, second.leaf);
        let last = world
            .app
            .call(PoolChoicesGet, &[&pool])
            .token(&owner.token)
            .query(&page.next.unwrap())
            .send()
            .await
            .ok();
        assert_eq!(last.items.len(), 1);
        assert_eq!(last.items[0].id, first_read);
        assert!(last.next.is_none());
        let details = world
            .app
            .call(PoolChoiceDetailsGet, &[&pool, &first_read])
            .token(&owner.token)
            .query(&PoolChoiceDetailsQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(details.items.len(), 1);
        assert!(details.next.is_some());
    }
    assert_eq!(
        choose(&world, &owner, pool, new_id()).await.leaf,
        Some(leaves[0].clone())
    );
}

#[tokio::test]
async fn choice_history_rechecks_the_actual_read_sources_and_says_when_details_expired() {
    let world = World::new().await;
    let owner = person(&world, "private-choice-reader").await;
    let root = make_org(&world, &owner, "Private choices", "private-choices")
        .await
        .base
        .id;
    let (pool, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    let choice = choose(&world, &owner, pool, new_id()).await;
    let recorded = read_id(&world, choice.id).await;
    let narrow = key(
        &world,
        &owner,
        vec![
            rule("PoolChoicesGet", "pool", pool, &["pool"], "read"),
            rule("PoolChoiceDetailsGet", "pool", pool, &["pool"], "read"),
        ],
    )
    .await;
    let page = world
        .app
        .call(PoolChoicesGet, &[&pool])
        .token(&narrow)
        .query(&PoolChoicesQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].resource.is_none());
    assert_eq!(page.items[0].reason.as_deref(), Some("member_hidden"));
    let details = world
        .app
        .call(PoolChoiceDetailsGet, &[&pool, &recorded])
        .token(&narrow)
        .query(&PoolChoiceDetailsQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(details.items.len(), 2);
    let text = serde_json::to_string(&details).unwrap();
    for leaf in leaves {
        let Leaf::Host { id } = leaf else {
            unreachable!()
        };
        assert!(!text.contains(&id.to_string()));
    }
    assert!(!text.contains("First"));
    assert!(
        details
            .items
            .iter()
            .all(|item| item.resource.is_none() && item.label.is_none() && item.cost.is_none())
    );
    let visible = world
        .app
        .call(PoolChoicesGet, &[&pool])
        .token(&owner.token)
        .query(&PoolChoicesQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(visible.items[0].resource, choice.leaf);
    let mut conn = world.conn().await;
    sqlx::query("DELETE FROM pool_choice_members WHERE choice_id=$1")
        .bind(choice.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DELETE FROM pool_choice_nodes WHERE choice_id=$1")
        .bind(choice.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE pool_choices SET details_expired_at=$2 WHERE id=$1")
        .bind(choice.id)
        .bind(world.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let expired = world
        .app
        .call(PoolChoiceDetailsGet, &[&pool, &recorded])
        .token(&owner.token)
        .query(&PoolChoiceDetailsQuery::default())
        .send()
        .await
        .ok();
    assert!(expired.expired_at.is_some());
    assert!(expired.items.is_empty());
    assert!(expired.next.is_none());
    assert_eq!(choose(&world, &owner, pool, choice.id).await, choice);
}

#[tokio::test]
async fn reading_a_leaf_does_not_disclose_a_choice_through_a_hidden_inner_pool() {
    use orch_api_contract::pools::{PoolMemberPut, PoolMemberPutBody};
    use orch_provider_contract::pools::{Member, MemberRule};
    let world = World::new().await;
    let owner = person(&world, "nested-choice-reader").await;
    let home = make_org(&world, &owner, "Nested history", "nested-history")
        .await
        .base
        .id;
    let (root, _) = pool(&world, &owner, home, Policy::Ordered).await;
    let (child, leaves) = pool(&world, &owner, home, Policy::RoundRobin).await;
    sqlx::query("DELETE FROM pool_members WHERE pool_id=$1")
        .bind(root)
        .execute(&mut *world.conn().await)
        .await
        .unwrap();
    let member = new_id();
    world
        .app
        .call(PoolMemberPut, &[&root, &member])
        .token(&owner.token)
        .body(&PoolMemberPutBody {
            expected_revision: 1,
            member: Member {
                id: member,
                rule: MemberRule::Pool {
                    id: child,
                    kind: Kind::Host,
                },
                enabled: true,
                priority: None,
                weight: None,
            },
            position: None,
        })
        .send()
        .await
        .ok();
    let choice = choose(&world, &owner, root, new_id()).await;
    assert_eq!(choice.leaf, Some(leaves[0].clone()));
    let Leaf::Host { id: host } = &leaves[0] else {
        unreachable!()
    };
    let reader = key(
        &world,
        &owner,
        vec![
            rule("PoolChoicesGet", "pool", root, &["pool"], "read"),
            rule("PoolChoicesGet", "host", *host, &["host"], "read"),
        ],
    )
    .await;
    let hidden = world
        .app
        .call(PoolChoicesGet, &[&root])
        .token(&reader)
        .query(&PoolChoicesQuery::default())
        .send()
        .await
        .ok();
    assert!(hidden.items[0].resource.is_none());
    assert_eq!(hidden.items[0].reason.as_deref(), Some("member_hidden"));
    let shown = world
        .app
        .call(PoolChoicesGet, &[&root])
        .token(&owner.token)
        .query(&PoolChoicesQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(shown.items[0].resource, choice.leaf);
}

#[tokio::test]
async fn a_saved_load_observation_keeps_its_original_count_and_time_only_for_a_readable_member() {
    use orch_provider_contract::pools::LoadMeasure;

    let world = World::new().await;
    let owner = person(&world, "load-history-owner").await;
    let home = make_org(&world, &owner, "Load history", "load-history")
        .await
        .base
        .id;
    let (pool, _) = pool(
        &world,
        &owner,
        home,
        Policy::LeastLoaded {
            measure: LoadMeasure::Threads,
        },
    )
    .await;
    let observed = Time::new(world.now());
    let choice = choose(&world, &owner, pool, new_id()).await;
    assert!(choice.leaf.is_some());
    let id = read_id(&world, choice.id).await;
    world.advance(chrono::Duration::minutes(3));
    let narrow = key(
        &world,
        &owner,
        vec![rule(
            "PoolChoiceDetailsGet",
            "pool",
            pool,
            &["pool"],
            "read",
        )],
    )
    .await;
    for (token, readable) in [(&owner.token, true), (&narrow, false)] {
        let details = world
            .app
            .call(PoolChoiceDetailsGet, &[&pool, &id])
            .token(token)
            .query(&PoolChoiceDetailsQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(details.items.len(), 2);
        for member in details.items {
            let row = serde_json::to_value(member).unwrap();
            if readable {
                assert_eq!(
                    row["observations"],
                    json!([{"measure":"threads","score":0.0,"at":observed}])
                );
            } else {
                assert_eq!(row["reason"], "member_hidden");
                assert!(row.get("observations").is_none());
            }
        }
    }
}
