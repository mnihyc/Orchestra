//! Protected transports can confirm the same full native tree without publishing its rows.

use super::*;

#[tokio::test]
async fn deletion_source_packets_keep_their_transaction_actor_and_ordinary_manage_gate()
-> TestResult {
    let area = Area::new(false).await?;
    let child = area
        .document("source-packet.md", "Locked source facts are not authority")
        .await?;
    let command = request(
        domain::Command::Delete,
        vec![domain::Item::Document(child)],
        None,
        "deletion-source-packet",
    );
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let actor = domain::Actor::Agent { thread: area.agent };
    let mut tx = area.world.pool.begin().await?;
    let sources = domain::prepare_deletion_sources_in(&mut tx, actor, &command).await?;
    assert_eq!(
        sources
            .entries()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![child]
    );
    let denied = domain::complete_deletion_plan_in(&mut tx, actor, sources, now, options).await;
    assert!(matches!(denied, Err(domain::Error::Denied)));
    tx.rollback().await?;
    let before = state(&area.world, &[area.topic]).await?;
    let mut tx = area.world.pool.begin().await?;
    let owner =
        orch_authz::Principal::load(&mut tx, orch_authz::Caller::session(area.world.ada), now)
            .await?;
    let sources =
        domain::prepare_deletion_sources_in(&mut tx, domain::Actor::Person(&owner), &command)
            .await?;
    let changed_actor =
        domain::complete_deletion_plan_in(&mut tx, actor, sources, now, options).await;
    assert!(matches!(changed_actor, Err(domain::Error::Denied)));
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    let sources =
        domain::prepare_deletion_sources_in(&mut tx, domain::Actor::Person(&owner), &command)
            .await?;
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    let changed_transaction = domain::complete_deletion_plan_in(
        &mut tx,
        domain::Actor::Person(&owner),
        sources,
        now,
        options,
    )
    .await;
    assert!(matches!(changed_transaction, Err(domain::Error::Stale)));
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}

#[tokio::test]
async fn deletion_digest_keeps_complete_native_membership_pins_and_durable_replay() -> TestResult {
    let area = Area::new(true).await?;
    let folder = area.folder("remove/").await?;
    let child = area
        .document("remove/report.md", "Retained native deletion material")
        .await?;
    let attached = attach(&area.world, child).await?;
    let command = request(
        domain::Command::Delete,
        vec![domain::Item::Container(folder)],
        None,
        "digest-delete",
    );
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let options = domain::Options::for_engine(&area.world.engine());
    let mut tx = area.world.pool.begin().await?;
    let sources = domain::prepare_deletion_sources_in(&mut tx, actor, &command).await?;
    assert_eq!(sources.entries().len(), 2);
    assert!(sources.entries().iter().any(|entry| entry.id == child));
    let plan = domain::complete_deletion_plan_in(&mut tx, actor, sources, now, options).await?;
    assert_eq!(plan.stamp.entries.len(), 2);
    assert!(
        plan.stamp
            .entries
            .iter()
            .any(|entry| entry.item == domain::Item::Document(child))
    );
    let digest = plan.stamp.deletion_digest()?;
    let receipt = domain::execute_confirmed_in(
        &mut tx,
        actor,
        &command,
        Some(domain::DeletionConfirmation::Digest(&digest)),
        now,
        options,
    )
    .await?;
    assert_eq!(receipt.entries.len(), 2);
    assert!(
        receipt
            .entries
            .iter()
            .all(|entry| entry.deleted_at.is_some())
    );
    tx.commit().await?;
    assert_eq!(
        pins(&area.world, attached.hash).await?,
        1,
        "soft deletion keeps the original file pin"
    );
    let before = state(&area.world, &[area.topic]).await?;
    let mut tx = area.world.pool.begin().await?;
    let replay = domain::execute_confirmed_in(
        &mut tx,
        actor,
        &command,
        Some(domain::DeletionConfirmation::Digest(&digest)),
        now,
        options,
    )
    .await?;
    assert_eq!(replay.operation, receipt.operation);
    tx.commit().await?;
    assert_eq!(
        state(&area.world, &[area.topic]).await?,
        before,
        "confirmation replay repeats no material, authority, history, audit or receipt write"
    );
    Ok(())
}

#[tokio::test]
async fn deletion_digest_refuses_new_members_material_or_labels_and_never_substitutes_for_native_authority()
-> TestResult {
    for change in ["member", "material", "title"] {
        let area = Area::new(true).await?;
        let folder = area.folder("bounded/").await?;
        let child = area
            .document("bounded/source.md", "Current native text")
            .await?;
        let command = request(
            domain::Command::Delete,
            vec![domain::Item::Container(folder)],
            None,
            &format!("stale-digest-{change}"),
        );
        let now = Time::new(area.world.clock.now());
        let actor = domain::Actor::Agent { thread: area.agent };
        let options = domain::Options::for_engine(&area.world.engine());
        let mut tx = area.world.pool.begin().await?;
        let plan = domain::plan_delete_in(&mut tx, actor, &command, now, options).await?;
        let digest = plan.stamp.deletion_digest()?;
        tx.commit().await?;
        let mut attached = None;
        match change {
            "member" => {
                area.document("bounded/new.md", "A real new descendant")
                    .await?;
            }
            "material" => {
                attached = Some(attach(&area.world, child).await?);
            }
            _ => {
                sqlx::query("UPDATE documents SET title='Changed native title' WHERE id=$1")
                    .bind(child)
                    .execute(area.world.pool.pool())
                    .await?;
            }
        }
        let before = state(&area.world, &[area.topic]).await?;
        let mut tx = area.world.pool.begin().await?;
        let result = domain::execute_confirmed_in(
            &mut tx,
            actor,
            &command,
            Some(domain::DeletionConfirmation::Digest(&digest)),
            now,
            options,
        )
        .await;
        assert!(matches!(result, Err(domain::Error::Stale)), "{change}");
        tx.commit().await?;
        assert_eq!(state(&area.world, &[area.topic]).await?, before);
        if let Some(attached) = attached {
            assert_eq!(pins(&area.world, attached.hash).await?, 1);
        }
    }
    let area = Area::new(false).await?;
    let child = area
        .document("managed.md", "The digest is not an authority grant")
        .await?;
    let command = request(
        domain::Command::Delete,
        vec![domain::Item::Document(child)],
        None,
        "digest-needs-native-manage",
    );
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let mut tx = area.world.pool.begin().await?;
    let owner =
        orch_authz::Principal::load(&mut tx, orch_authz::Caller::session(area.world.ada), now)
            .await?;
    let plan = domain::plan_delete_in(
        &mut tx,
        domain::Actor::Person(&owner),
        &command,
        now,
        options,
    )
    .await?;
    let digest = plan.stamp.deletion_digest()?;
    tx.commit().await?;
    let before = state(&area.world, &[area.topic]).await?;
    let mut tx = area.world.pool.begin().await?;
    let denied = domain::execute_confirmed_in(
        &mut tx,
        domain::Actor::Agent { thread: area.agent },
        &command,
        Some(domain::DeletionConfirmation::Digest(&digest)),
        now,
        options,
    )
    .await;
    assert!(matches!(denied, Err(domain::Error::Denied)));
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}
