//! The native header handoff exposes the exact selector, not audience, material or a new effect.

use super::*;
use std::collections::BTreeSet;

fn items(rows: &[orch_db::queries::documents::containers::EntryRow]) -> BTreeSet<domain::Item> {
    rows.iter()
        .map(|row| {
            if row.kind == "container" {
                domain::Item::Container(row.id)
            } else {
                domain::Item::Document(row.id)
            }
        })
        .collect()
}

#[tokio::test]
async fn operation_source_stage_keeps_complete_mixed_membership_and_actual_destinations()
-> TestResult {
    let area = Area::new(true).await?;
    let folder = area.folder("source/").await?;
    let document = area
        .document("source/live.md", "Native source words")
        .await?;
    let retained = area
        .document("source/retained.md", "Independently retained words")
        .await?;
    let attached = attach(&area.world, document).await?;
    remove(
        &area.world,
        area.agent,
        domain::Item::Document(retained),
        "retained-before-source-stage",
    )
    .await?;
    let destination = area.folder("destination/").await?;
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let expected: BTreeSet<_> = [
        domain::Item::Container(folder),
        domain::Item::Document(document),
        domain::Item::Document(retained),
    ]
    .into_iter()
    .collect();
    for command in [
        domain::Command::Copy,
        domain::Command::Move,
        domain::Command::Delete,
    ] {
        let before = state(&area.world, &[area.topic]).await?;
        let to = (command != domain::Command::Delete)
            .then_some(domain::Directory::Container(destination));
        let request = request(
            command,
            vec![domain::Item::Container(folder)],
            to,
            &format!("source-stage-{command:?}"),
        );
        let mut tx = area.world.pool.begin().await?;
        let sources = domain::prepare_operation_sources_in(&mut tx, actor, &request).await?;
        assert_eq!(sources.command(), command);
        assert!(!sources.is_replay());
        assert!(sources.replay_items().is_none());
        assert_eq!(
            items(sources.roots().ok_or("native new roots absent")?),
            [domain::Item::Container(folder)].into_iter().collect()
        );
        let mut entries = expected.clone();
        if command == domain::Command::Copy {
            entries.remove(&domain::Item::Document(retained));
        }
        assert_eq!(
            items(sources.entries().ok_or("native new closure absent")?),
            entries
        );
        assert_eq!(
            sources.destination(),
            to.map(|to| (to, area.topic, area.world.root))
        );
        assert!(sources.restoration_destinations().is_none());
        domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
        tx.commit().await?;
        assert_eq!(
            state(&area.world, &[area.topic]).await?,
            before,
            "header/source admission produces no material, authority, audience result, audit or receipt"
        );
        assert_eq!(pins(&area.world, attached.hash).await?, 1);
    }
    for command in [
        domain::Command::CreateFolder,
        domain::Command::Rename,
        domain::Command::RestoreVersion,
    ] {
        let before = state(&area.world, &[area.topic]).await?;
        let mut request = request(
            command,
            if command == domain::Command::CreateFolder {
                vec![]
            } else {
                vec![domain::Item::Document(document)]
            },
            (command == domain::Command::CreateFolder)
                .then_some(domain::Directory::Container(destination)),
            &format!("native-shape-{command:?}"),
        );
        if command == domain::Command::RestoreVersion {
            request.version = Some(1);
        } else {
            request.name = Some("Supplied source-stage name".to_owned());
        }
        let mut tx = area.world.pool.begin().await?;
        let sources = domain::prepare_operation_sources_in(&mut tx, actor, &request).await?;
        assert_eq!(sources.command(), command);
        assert_eq!(
            items(sources.roots().ok_or("new source roots absent")?),
            request.items.iter().copied().collect()
        );
        domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
        tx.commit().await?;
        assert_eq!(state(&area.world, &[area.topic]).await?, before);
    }
    remove(
        &area.world,
        area.agent,
        domain::Item::Container(folder),
        "retain-whole-source-stage",
    )
    .await?;
    let before = state(&area.world, &[area.topic]).await?;
    let mut tx = area.world.pool.begin().await?;
    let request = request(
        domain::Command::Restore,
        vec![domain::Item::Container(folder)],
        None,
        "source-recorded-restoration",
    );
    let sources = domain::prepare_operation_sources_in(&mut tx, actor, &request).await?;
    assert_eq!(
        items(sources.entries().ok_or("retained closure absent")?),
        expected
    );
    assert_eq!(
        sources.restoration_destinations(),
        Some(vec![domain::Directory::NodeDocs(area.topic)])
    );
    assert!(sources.destination().is_none());
    domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    let mut moved = request.clone();
    moved.request_ref = "source-explicit-restoration".into();
    moved.to = Some(domain::Directory::Container(destination));
    let mut tx = area.world.pool.begin().await?;
    let sources = domain::prepare_operation_sources_in(&mut tx, actor, &moved).await?;
    assert_eq!(
        sources.restoration_destinations(),
        Some(vec![domain::Directory::Container(destination)])
    );
    assert_eq!(
        sources.destination(),
        Some((
            domain::Directory::Container(destination),
            area.topic,
            area.world.root
        ))
    );
    domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}

#[tokio::test]
async fn operation_source_stage_keeps_native_authority_actor_transaction_and_provisional_mode()
-> TestResult {
    let area = Area::new(false).await?;
    let document = area
        .document("source.md", "Factual membership grants no authority")
        .await?;
    let request = request(
        domain::Command::Delete,
        vec![domain::Item::Document(document)],
        None,
        "source-native-boundaries",
    );
    let now = Time::new(area.world.clock.now());
    let agent = domain::Actor::Agent { thread: area.agent };
    let before = state(&area.world, &[area.topic]).await?;
    let mut tx = area.world.pool.begin().await?;
    let sources = domain::prepare_operation_sources_in(&mut tx, agent, &request).await?;
    assert_eq!(
        items(sources.entries().ok_or("actual denied source absent")?),
        [domain::Item::Document(document)].into_iter().collect()
    );
    assert!(matches!(
        domain::require_operation_sources_in(&mut tx, agent, &sources, now).await,
        Err(domain::Error::Denied)
    ));
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    // This is the native owner's ordinary test principal, not retained HTTP credential evidence.
    let owner =
        orch_authz::Principal::load(&mut tx, orch_authz::Caller::session(area.world.ada), now)
            .await?;
    let actor = domain::Actor::Person(&owner);
    let sources = domain::prepare_operation_sources_in(&mut tx, actor, &request).await?;
    domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
    assert!(matches!(
        domain::require_operation_sources_in(&mut tx, agent, &sources, now).await,
        Err(domain::Error::Denied)
    ));
    tx.rollback().await?;
    let mut tx = area.world.pool.begin().await?;
    assert!(matches!(
        domain::require_operation_sources_in(&mut tx, actor, &sources, now).await,
        Err(domain::Error::Stale)
    ));
    tx.commit().await?;
    let mut conn = area.world.pool.acquire().await?;
    assert!(matches!(
        domain::require_operation_sources_in(&mut conn, actor, &sources, now).await,
        Err(domain::Error::Stale)
    ));
    let provisional = domain::prepare_operation_sources_in(&mut conn, actor, &request).await?;
    domain::require_operation_sources_in(&mut conn, actor, &provisional, now).await?;
    drop(conn);
    let mut tx = area.world.pool.begin().await?;
    assert!(matches!(
        domain::require_operation_sources_in(&mut tx, actor, &provisional, now).await,
        Err(domain::Error::Stale)
    ));
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}

#[tokio::test]
async fn operation_source_replay_uses_only_its_actual_native_result_without_new_copy_or_material()
-> TestResult {
    let area = Area::new(true).await?;
    let source = area
        .document("source.md", "Material belongs to the original copy only")
        .await?;
    let attached = attach(&area.world, source).await?;
    let mut request = request(
        domain::Command::Copy,
        vec![domain::Item::Document(source)],
        Some(domain::Directory::NodeDocs(area.topic)),
        "source-native-copy-replay",
    );
    request.name = Some("native-copied.md".into());
    let actor = domain::Actor::Agent { thread: area.agent };
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    let mut tx = area.world.pool.begin().await?;
    let receipt = domain::execute_in(&mut tx, actor, &request, None, now, options).await?;
    tx.commit().await?;
    assert_eq!(receipt.entries.len(), 1);
    assert_ne!(receipt.entries[0].item, domain::Item::Document(source));
    remove(
        &area.world,
        area.agent,
        domain::Item::Document(source),
        "retain-original-after-copy",
    )
    .await?;
    let before = state(&area.world, &[area.topic]).await?;
    let pin_count = pins(&area.world, attached.hash).await?;
    let mut tx = area.world.pool.begin().await?;
    let sources = domain::prepare_operation_sources_in(&mut tx, actor, &request).await?;
    assert!(sources.is_replay());
    assert_eq!(sources.command(), domain::Command::Copy);
    assert!(sources.roots().is_none());
    assert!(sources.entries().is_none());
    assert!(sources.destination().is_none());
    assert!(sources.restoration_destinations().is_none());
    assert_eq!(sources.replay_items(), Some(&[receipt.entries[0].item][..]));
    domain::require_operation_sources_in(&mut tx, actor, &sources, now).await?;
    let repeated = domain::execute_in(&mut tx, actor, &request, None, now, options).await?;
    assert_eq!(repeated.operation, receipt.operation);
    assert_eq!(repeated.entries, receipt.entries);
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    assert_eq!(pins(&area.world, attached.hash).await?, pin_count);
    let mut replaced = request.clone();
    replaced.name = Some("substituted-copy.md".into());
    let mut tx = area.world.pool.begin().await?;
    assert!(matches!(
        domain::prepare_operation_sources_in(&mut tx, actor, &replaced).await,
        Err(domain::Error::Conflict)
    ));
    tx.commit().await?;
    assert_eq!(state(&area.world, &[area.topic]).await?, before);
    Ok(())
}
