//! Native recording controls exercise real checked archives and the actual SQL writer, not HTTP authority.
use super::*;
use orch_types::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn ledger(world: &World, id: Id, hash: Hash) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('skill',(SELECT to_jsonb(s) FROM skills s WHERE id=$1),'versions',(SELECT jsonb_agg(to_jsonb(v) ORDER BY number) FROM skill_versions v WHERE skill_id=$1),'blob',(SELECT to_jsonb(b) FROM blobs b WHERE hash=$2))").bind(id).bind(hash.as_bytes().as_slice()).fetch_one(world.pool.pool()).await.unwrap()
}
#[tokio::test]
async fn staged_native_recording_has_no_effect_before_the_real_new_or_existing_first_write() {
    let world = World::new().await;
    let store = store().await;
    let existing = skill(
        &world,
        &store,
        world.root,
        world.ada,
        "native-recording-existing",
    )
    .await;
    for new in [true, false] {
        let (id, name, target) = if new {
            let id = orch_types::new_id();
            (
                id,
                "native-recording-new",
                Target::NewSkill {
                    id,
                    home: world.root,
                    owner: world.ada,
                },
            )
        } else {
            (
                existing.skill.id,
                "native-recording-existing",
                Target::Skill(existing.skill.id),
            )
        };
        let prepared = versions::prepare_files(
            &store.store,
            files(name, "the next real checked source"),
            LIMITS,
        )
        .await
        .unwrap();
        let before = ledger(&world, id, prepared.archive).await;
        let mut tx = world.pool.begin().await.unwrap();
        let plan = versions::prepare_record(
            &mut tx,
            target,
            &prepared,
            &SkillOrigin::Draft { based_on: None },
            Some(world.ada),
        )
        .await
        .unwrap();
        assert_eq!(plan.target(), target);
        assert_eq!(ledger(&world, id, prepared.archive).await, before);
        let called = AtomicUsize::new(0);
        let refused = versions::apply_record(
            &mut tx,
            Some(world.ada),
            plan,
            Time::new(world.clock.now()),
            || {
                called.fetch_add(1, Ordering::SeqCst);
                Err("native cut closed")
            },
        )
        .await;
        assert!(matches!(refused, Err("native cut closed")));
        assert_eq!(called.load(Ordering::SeqCst), 1);
        tx.rollback().await.unwrap();
        assert_eq!(ledger(&world, id, prepared.archive).await, before);
        let mut tx = world.pool.begin().await.unwrap();
        let plan = versions::prepare_record(
            &mut tx,
            target,
            &prepared,
            &SkillOrigin::Draft { based_on: None },
            Some(world.ada),
        )
        .await
        .unwrap();
        let made = versions::apply_record(
            &mut tx,
            Some(world.ada),
            plan,
            Time::new(world.clock.now()),
            || {
                called.fetch_add(1, Ordering::SeqCst);
                Ok::<(), &str>(())
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 2);
        assert_eq!(made.skill.id, id);
        assert_eq!(made.skill.latest_version_id, Some(made.version.id));
        assert_eq!(made.version.archive_hash().unwrap(), prepared.archive);
        tx.commit().await.unwrap();
        let after = ledger(&world, id, prepared.archive).await;
        assert_ne!(after, before);
        assert_eq!(
            after["versions"].as_array().unwrap().len(),
            if new { 1 } else { 2 }
        );
        assert_eq!(after["blob"]["pin_count"], 1);
    }
}

#[tokio::test]
async fn native_recording_packets_refuse_another_actor_transaction_and_provisional_promotion_before_callback()
 {
    let world = World::new().await;
    let store = store().await;
    let id = orch_types::new_id();
    let target = Target::NewSkill {
        id,
        home: world.root,
        owner: world.ada,
    };
    let prepared = versions::prepare_files(
        &store.store,
        files("native-recording-identity", "checked material"),
        LIMITS,
    )
    .await
    .unwrap();
    let before = ledger(&world, id, prepared.archive).await;
    let calls = AtomicUsize::new(0);
    let mut tx = world.pool.begin().await.unwrap();
    let plan = versions::prepare_record(
        &mut tx,
        target,
        &prepared,
        &SkillOrigin::Draft { based_on: None },
        Some(world.ada),
    )
    .await
    .unwrap();
    let refused = versions::apply_record(&mut tx, None, plan, Time::new(world.clock.now()), || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok::<(), &str>(())
    })
    .await
    .unwrap();
    assert!(refused.is_err());
    tx.rollback().await.unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let plan = versions::prepare_record(
        &mut tx,
        target,
        &prepared,
        &SkillOrigin::Draft { based_on: None },
        Some(world.ada),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let mut other = world.pool.begin().await.unwrap();
    let refused = versions::apply_record(
        &mut other,
        Some(world.ada),
        plan,
        Time::new(world.clock.now()),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        },
    )
    .await
    .unwrap();
    assert!(refused.is_err());
    other.rollback().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let plan = versions::prepare_record(
        &mut conn,
        target,
        &prepared,
        &SkillOrigin::Draft { based_on: None },
        Some(world.ada),
    )
    .await
    .unwrap();
    let refused = versions::apply_record(
        &mut conn,
        Some(world.ada),
        plan,
        Time::new(world.clock.now()),
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), &str>(())
        },
    )
    .await
    .unwrap();
    assert!(refused.is_err());
    drop(conn);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(ledger(&world, id, prepared.archive).await, before);
}

#[tokio::test]
async fn native_recording_keeps_existing_name_and_home_collision_refusals_before_any_callback() {
    let world = World::new().await;
    let store = store().await;
    let made = skill(
        &world,
        &store,
        world.root,
        world.ada,
        "native-recording-name",
    )
    .await;
    let changed = versions::prepare_files(
        &store.store,
        files("native-recording-other-name", "changed input"),
        LIMITS,
    )
    .await
    .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        versions::prepare_record(
            &mut tx,
            Target::Skill(made.skill.id),
            &changed,
            &SkillOrigin::Draft { based_on: None },
            Some(world.ada)
        )
        .await,
        Err(SkillError::NameChanged { .. })
    ));
    tx.rollback().await.unwrap();
    let collision = versions::prepare_files(
        &store.store,
        files("native-recording-name", "same native home name"),
        LIMITS,
    )
    .await
    .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(matches!(
        versions::prepare_record(
            &mut tx,
            Target::NewSkill {
                id: orch_types::new_id(),
                home: world.root,
                owner: world.ada
            },
            &collision,
            &SkillOrigin::Draft { based_on: None },
            Some(world.ada)
        )
        .await,
        Err(SkillError::NameTaken { .. })
    ));
    tx.rollback().await.unwrap();
    let original = ledger(&world, made.skill.id, made.version.archive_hash().unwrap()).await;
    assert_eq!(original["versions"].as_array().unwrap().len(), 1);
}
