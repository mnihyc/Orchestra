//! Concrete proof, admission and publication interleavings at the existing native budgets.

use super::observations_tests::Rig;
use super::*;

type Proof<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const PATIENCE: Duration = Duration::from_secs(15);

#[tokio::test]
async fn an_extracted_page_proof_is_invalidated_before_eviction_makes_it_unreachable() -> Proof {
    let rig = Rig::new()?;
    let page = rig.read(None, 1).await?;
    let proof = page.proof();
    assert!(
        lock(&rig.cache.captures).is_empty(),
        "the completed producer is already reaped"
    );
    assert_eq!(lock(&rig.cache.snapshots).len(), 1);
    page.validate()?;
    assert!(
        rig.cache.evict_one(),
        "a proof without a metadata pin may be evicted"
    );
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    assert!(matches!(page.validate(), Err(HostError::Stale { .. })));
    rig.cache.invalidate(
        rig.binding.environment,
        rig.binding.attachment,
        &[rig.files.parse("b")?],
        false,
    );
    assert!(
        matches!(proof.validate(), Err(HostError::Stale { .. })),
        "a known event cannot leave an extracted, no-longer-discoverable proof valid"
    );
    let fresh = rig.read(None, 2).await?;
    assert_eq!(
        fresh
            .entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    fresh.validate()?;
    Ok(())
}

#[tokio::test]
async fn lazy_expiry_invalidates_the_in_hand_proof_as_well_as_its_cursor() -> Proof {
    let rig = Rig::new()?;
    let page = rig.read(None, 1).await?;
    let proof = page.proof();
    rig.clock.advance(chrono::Duration::minutes(5));
    assert!(matches!(
        rig.read(page.next_cursor.as_deref(), 1).await,
        Err(HostError::Stale { .. })
    ));
    assert!(matches!(proof.validate(), Err(HostError::Stale { .. })));
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_arriving_across_the_last_close_starts_fresh_instead_of_inheriting_cancellation()
-> Proof {
    let rig = Rig::new()?;
    let mut capture = rig.cache.testing.hold_capture();
    let first = rig.spawn(1, None);
    capture.entered().await?;
    let mut reservation = rig.cache.testing.hold_reservation();
    let second = rig.spawn(2, None);
    // The matching producer is found, but its reader has not been reserved. Last-reader Drop
    // must be able to close it, and the later atomic reservation must observe that close.
    reservation.entered().await?;
    first.abort();
    assert!(
        tokio::time::timeout(PATIENCE, first)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    reservation.release()?;
    rig.cache.testing.attached(2).await?;
    capture.release()?;
    let page = tokio::time::timeout(PATIENCE, second).await???;
    assert_eq!(
        page.entries
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    page.validate()?;
    assert_eq!(rig.cache.testing.started.load(Ordering::Acquire), 2);
    assert_eq!(rig.cache.testing.exited.load(Ordering::Acquire), 2);
    assert!(lock(&rig.cache.captures).is_empty());
    assert_eq!(
        lock(&rig.cache.snapshots).len(),
        1,
        "only the live reader's producer may publish"
    );
    Ok(())
}

#[tokio::test]
async fn the_actual_last_reader_close_after_the_prelock_check_prevents_orphan_publication() -> Proof
{
    let rig = Rig::new()?;
    let mut publication = rig.cache.testing.hold_publication_lock();
    let task = rig.spawn(1, None);
    publication.entered().await?;
    let builds = lock(&rig.cache.captures).clone();
    assert_eq!(builds.len(), 1);
    task.abort();
    assert!(
        tokio::time::timeout(PATIENCE, task)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    publication.release()?;
    // Do not call forget/invalidate/join_attachment: only the real reader's close supplies
    // cancellation, after the old final pre-lock check and before publication ordering.
    for build in builds {
        tokio::time::timeout(PATIENCE, build.join()).await??;
    }
    rig.cache.reap().await?;
    assert!(lock(&rig.cache.snapshots).is_empty());
    assert!(lock(&rig.cache.captures).is_empty());
    assert_eq!(rig.cache.ledger.used.load(Ordering::Acquire), 0);
    assert_eq!(rig.cache.build.available_permits(), 1);
    assert_eq!(rig.cache.testing.exited.load(Ordering::Acquire), 1);
    Ok(())
}

#[tokio::test]
async fn request_owners_use_one_host_bound_and_a_retired_scope_never_admits_again() -> Proof {
    let rig = Rig::new()?;
    let attachment = Attachment::new();
    let mut owners = Vec::new();
    for _ in 0..SLOTS {
        owners.push(attachment.begin(&rig.cache, None).await?);
    }
    assert!(
        matches!(
            Attachment::new().begin(&rig.cache, None).await,
            Err(HostError::Busy(_))
        ),
        "another attachment cannot create a separate allowance"
    );
    attachment.retire();
    attachment.join().await?;
    assert!(matches!(
        attachment.begin(&rig.cache, None).await,
        Err(HostError::Changed(_))
    ));
    for owner in &owners {
        assert!(matches!(owner.check(), Err(HostError::Changed(_))));
    }
    drop(owners);
    assert_eq!(rig.cache.operations.available(), SLOTS);
    let fresh = Attachment::new();
    fresh.begin(&rig.cache, None).await?.check()?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_native_location_worker_keeps_its_host_owner_until_joined_exit() -> Proof {
    let rig = Rig::new()?;
    let attachment = Attachment::new();
    let mut hold = rig.cache.testing.hold_location();
    let (cache, source, scope) = (
        Arc::clone(&rig.cache),
        rig.files.clone(),
        Arc::clone(&attachment),
    );
    let reading = tokio::spawn(async move {
        let operation = scope.begin(&cache, None).await?;
        let testing = Arc::clone(&cache.testing);
        operation
            .blocking(move || {
                let _location = testing.before_location()?;
                source.directory_location(&RelPath::root())
            })
            .await
    });
    hold.entered().await?;
    reading.abort();
    assert!(
        tokio::time::timeout(PATIENCE, reading)
            .await?
            .is_err_and(|error| error.is_cancelled())
    );
    assert_eq!(
        rig.cache.operations.available(),
        SLOTS - 1,
        "caller cancellation cannot credit a still-running owner"
    );
    assert_eq!(
        rig.cache.testing.locations_exited.load(Ordering::Acquire),
        0
    );
    {
        let joined = attachment.join();
        tokio::pin!(joined);
        assert!(matches!(
            futures::poll!(&mut joined),
            std::task::Poll::Pending
        ));
        // Cancelling an in-progress join must leave the actual handle owned for the next join.
    }
    assert_eq!(rig.cache.operations.available(), SLOTS - 1);
    assert_eq!(
        rig.cache.testing.locations_exited.load(Ordering::Acquire),
        0
    );
    hold.release()?;
    tokio::time::timeout(PATIENCE, attachment.join()).await??;
    assert_eq!(
        rig.cache.testing.locations_exited.load(Ordering::Acquire),
        1
    );
    assert_eq!(rig.cache.operations.available(), SLOTS);
    Ok(())
}
