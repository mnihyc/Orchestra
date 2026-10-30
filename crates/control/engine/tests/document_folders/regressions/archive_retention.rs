//! Current archive rules stay distinct from the authority to restore actual retained deletions.

use super::*;

#[tokio::test]
async fn the_domain_refuses_live_archived_agent_mutations_but_can_restore_a_retained_tree_elsewhere()
-> TestResult {
    let area = Area::new(true).await?;
    let retained = area.folder("retained/").await?;
    let retained_document = area
        .document("retained/Original.md", "Retained original material.")
        .await?;
    let old_blocks = area.world.blocks_of(retained_document).await;
    remove(
        &area.world,
        area.agent,
        domain::Item::Container(retained),
        "retained-before-archive",
    )
    .await?;
    let live = area.folder("live/").await?;
    let live_document = area
        .document("live/Plan.md", "Archive is read-only.")
        .await?;
    let destination = area.world.topic("Live destination").await;
    share(
        &area.world,
        "node",
        destination,
        "live",
        "doc.edit",
        "any",
        area.agent,
    )
    .await;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(area.topic)
        .bind(area.world.clock.now())
        .execute(area.world.pool.pool())
        .await?;
    let before = state(&area.world, &[area.topic, destination]).await?;
    let actor = domain::Actor::Agent { thread: area.agent };
    let now = Time::new(area.world.clock.now());
    let options = domain::Options::for_engine(&area.world.engine());
    for item in [
        domain::Item::Container(live),
        domain::Item::Document(live_document),
    ] {
        let read =
            domain::read_in(&mut *area.world.pool.acquire().await?, actor, item, now).await?;
        assert!(read.entries[0].standing.read);
        assert!(!read.entries[0].standing.write && !read.entries[0].standing.manage);
        let mut tx = area.world.pool.begin().await?;
        let move_request = request(
            domain::Command::Move,
            vec![item],
            Some(domain::Directory::NodeDocs(destination)),
            "archived-move",
        );
        assert!(matches!(
            domain::execute_in(&mut tx, actor, &move_request, None, now, options).await,
            Err(domain::Error::Denied)
        ));
        let delete_request = request(domain::Command::Delete, vec![item], None, "archived-delete");
        assert!(matches!(
            domain::plan_delete_in(&mut tx, actor, &delete_request, now, options).await,
            Err(domain::Error::Denied)
        ));
        tx.commit().await?;
        assert_eq!(
            state(&area.world, &[area.topic, destination]).await?,
            before
        );
    }
    for parent in [
        domain::Directory::NodeDocs(area.topic),
        domain::Directory::Container(live),
    ] {
        let mut tx = area.world.pool.begin().await?;
        let mut create = request(
            domain::Command::CreateFolder,
            Vec::new(),
            Some(parent),
            "archived-create",
        );
        create.name = Some("No new folder".into());
        assert!(matches!(
            domain::execute_in(&mut tx, actor, &create, None, now, options).await,
            Err(domain::Error::Denied)
        ));
        assert!(matches!(
            domain::create_text_in(&mut tx, actor, parent, new_id(), "No new text", None, now)
                .await,
            Err(domain::Error::Denied)
        ));
        tx.commit().await?;
        assert_eq!(
            state(&area.world, &[area.topic, destination]).await?,
            before
        );
    }
    let restore = request(
        domain::Command::Restore,
        vec![domain::Item::Container(retained)],
        Some(domain::Directory::NodeDocs(destination)),
        "restore-elsewhere",
    );
    let mut tx = area.world.pool.begin().await?;
    let receipt = domain::execute_in(&mut tx, actor, &restore, None, now, options).await?;
    tx.commit().await?;
    assert_eq!(receipt.entries.len(), 2);
    assert!(
        receipt
            .entries
            .iter()
            .all(|entry| entry.node == destination && entry.deleted_at.is_none())
    );
    assert!(
        receipt
            .entries
            .iter()
            .any(|entry| entry.item == domain::Item::Document(retained_document))
    );
    assert_eq!(
        area.world.blocks_of(retained_document).await,
        old_blocks,
        "actual retained restoration keeps identities and content"
    );
    Ok(())
}

#[tokio::test]
async fn a_preview_uses_the_original_child_promise_after_retention_changes_and_restoration_keeps_it()
-> TestResult {
    // Grant fixtures use the system clock. Fix the domain clock one day beyond those grants,
    // with no expiring entry, so exact deadline equality never depends on elapsed wall time.
    let clock = std::sync::Arc::new(orch_types::ManualClock::new(
        Time::new(chrono::Utc::now()).get() + chrono::Duration::days(1),
    ));
    let world = World::with_clock(clock).await;
    shared::the_agent_takes_tools_again(&world).await;
    let area = Area::on_world(world, true).await?;
    let parent = area.folder("research/").await?;
    let old = area
        .document("research/Old.md", "An independent retained deletion.")
        .await?;
    let current = area
        .document(
            "research/Current.md",
            "This deletion will use current policy.",
        )
        .await?;
    let removed = remove(
        &area.world,
        area.agent,
        domain::Item::Document(old),
        "original-child-deletion",
    )
    .await?;
    let promised = removed.restore_until.ok_or("child promise missing")?;
    let old_cohort = removed.operation;
    let entry = removed.entries[0].clone();
    // Both a longer and a shorter deployment policy must leave the recorded promise exact.
    for days in [90_u64, 1] {
        area.world.set_config(|config| {
            config.soft_delete = std::time::Duration::from_secs(days * 86_400)
        });
        area.world.restart_api().await;
        let before = state(&area.world, &[area.topic]).await?;
        let preview = area
            .organize(json!({"command":"delete","paths":[area.path("research/")]}))
            .await;
        let affected = preview["affects"]
            .as_array()
            .ok_or("complete preview absent")?;
        let old_line = affected
            .iter()
            .find(|line| line["id"] == json!(old))
            .ok_or("independent child absent")?;
        let old_until: Time = serde_json::from_value(old_line["until"].clone())?;
        assert_eq!(old_until.get(), promised);
        let new_line = affected
            .iter()
            .find(|line| line["id"] == json!(current))
            .ok_or("new child absent")?;
        let new_until: Time = serde_json::from_value(new_line["until"].clone())?;
        assert_eq!(
            new_until.get(),
            area.world.clock.now() + chrono::Duration::days(i64::try_from(days)?)
        );
        assert_eq!(
            state(&area.world, &[area.topic]).await?,
            before,
            "reading a preview never rewrites a promise or cohort"
        );
        let options = domain::Options::for_engine(&area.world.engine());
        assert_eq!(
            domain::restore_deadline_in(&mut *area.world.pool.acquire().await?, &entry, options)
                .await?,
            Some(promised)
        );
    }
    // Exercise the cohort fallback too: the native receipt remains authoritative if a legacy
    // lifecycle projection has no recorded purge_at. Restore the projection before the final checks.
    let mut tx = area.world.pool.begin().await?;
    sqlx::query(
        "UPDATE audit_object_state SET purge_at=NULL WHERE object_kind='document' AND object_id=$1",
    )
    .bind(old)
    .execute(&mut *tx)
    .await?;
    assert_eq!(
        domain::restore_deadline_in(
            &mut tx,
            &entry,
            domain::Options::for_engine(&area.world.engine())
        )
        .await?,
        Some(promised)
    );
    tx.rollback().await?;
    let parent_removed = remove(
        &area.world,
        area.agent,
        domain::Item::Container(parent),
        "later-parent-deletion",
    )
    .await?;
    let mut tx = area.world.pool.begin().await?;
    let now = Time::new(area.world.clock.now());
    let actor = domain::Actor::Agent { thread: area.agent };
    let options = domain::Options::for_engine(&area.world.engine());
    let child_first = request(
        domain::Command::Restore,
        vec![domain::Item::Document(old)],
        None,
        "parent-first",
    );
    assert!(matches!(
        domain::execute_in(&mut tx, actor, &child_first, None, now, options).await,
        Err(domain::Error::NotFound)
    ));
    let restore = request(
        domain::Command::Restore,
        vec![domain::Item::Container(parent)],
        None,
        "restore-parent",
    );
    let receipt = domain::execute_in(&mut tx, actor, &restore, None, now, options).await?;
    assert_eq!(receipt.entries.len(), 2);
    assert!(
        receipt
            .entries
            .iter()
            .all(|entry| entry.deleted_at.is_none())
    );
    let old_row = documents::containers::entry(&mut tx, "document", old)
        .await?
        .ok_or("old child absent")?;
    assert_eq!(old_row.deleted_in, Some(old_cohort));
    assert_ne!(old_row.deleted_in, Some(parent_removed.operation));
    assert_eq!(
        domain::restore_deadline_in(&mut tx, &entry, options).await?,
        Some(promised)
    );
    let longer = domain::Options {
        retention: std::time::Duration::from_secs(90 * 86_400),
        ..options
    };
    assert!(
        matches!(
            domain::execute_in(
                &mut tx,
                actor,
                &child_first,
                None,
                Time::new(promised + chrono::Duration::milliseconds(1)),
                longer
            )
            .await,
            Err(domain::Error::Conflict)
        ),
        "a longer current policy cannot extend the stored child promise"
    );
    let restored = domain::execute_in(&mut tx, actor, &child_first, None, now, options).await?;
    assert_eq!(restored.entries[0].item, domain::Item::Document(old));
    assert!(restored.entries[0].deleted_at.is_none());
    tx.commit().await?;
    Ok(())
}
