//! What a transfer between machines holds in the store, and how it lets go of it.
//!
//! Content that passes from one machine to another through the server (a move of an environment or
//! of a machine, a fork, the files of a new environment, a recreated unit) is the transfer's own. It
//! is never recorded as a cached version of a file. The transfer pins what it brings, as soon as the
//! source has uploaded it, so that nothing collects it while the destination fetches it; and it lets
//! go of every pin once, whatever its end, with `release_transfer`. What nothing else holds is then
//! gone from the server at once (`Released::settle`), not after the grace that ordinary content gets.
//!
//! Whether a place relays at all is its setting `storage.relay_transfers` ([`relay_on`]): a place that chose
//! to cache keeps what it always did, the grace.
//!
//! What something else holds stays: a snapshot, an attachment, a document, a cached version of a file
//! and a model input each keep the content they name, by the same conditions the collection looks at
//! ([`storage::lock_blob_retirements`]).

use chrono::{DateTime, Utc};
use orch_db::Conn;
use orch_db::queries::{jobs, storage};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::collections::BTreeSet;

use crate::{Engine, EngineError};

/// The reference-free content a collection is allowed to retire.
#[derive(Debug, Clone, Copy)]
pub enum RetirementWhen {
    /// A transfer ended; a retained private upload remains an independent owner.
    TransferEnded(DateTime<Utc>),
    /// Ordinary unreferenced content has had its configured grace.
    IdleSince(DateTime<Utc>),
}

/// What physical retirement removed from the catalogue in this run.
#[derive(Debug, Default, Clone, Copy)]
pub struct Retired {
    /// The rows removed after their physical file was gone.
    pub blobs: u64,
    /// Their total declared content size.
    pub bytes: i64,
}

/// Retires content under the same publication lifetime used by every real store and wrapper.
/// All lifetime guards are tried without waiting while a DB transaction is held. On contention,
/// both the transaction and the acquired guards go before waiting, avoiding pool and cross-hash
/// upgrade deadlocks. Locked native row writers are skipped by the final metadata check.
pub async fn retire_blobs(
    db: &orch_db::Db,
    store: &dyn orch_blob_contract::store::BlobStore,
    hashes: &[Hash],
    when: RetirementWhen,
) -> Result<Retired, orch_db::DbError> {
    let mut hashes = hashes.to_vec();
    hashes.sort_unstable();
    hashes.dedup();
    let mut done = Retired::default();
    'batch: for batch in hashes.chunks(storage::PIN_BATCH) {
        loop {
            let mut tx = db.begin().await?;
            let mut guarded = Vec::with_capacity(batch.len());
            let mut busy = None;
            for hash in batch {
                match store.lifetimes().try_retiring(*hash) {
                    Some(retirement) => guarded.push((*hash, retirement)),
                    None => {
                        busy = Some(*hash);
                        break;
                    }
                }
            }
            if let Some(hash) = busy {
                drop(guarded);
                drop(tx);
                match store.lifetimes().retiring(hash).await {
                    Ok(retirement) => drop(retirement),
                    Err(error) => {
                        tracing::warn!(%hash, %error, "content retirement could not acquire its physical lifetime");
                        continue 'batch;
                    }
                }
                continue;
            }
            let (idle_since, inclusive, keep_private_uploads, relay_only) = match when {
                RetirementWhen::TransferEnded(at) => (at, true, true, true),
                RetirementWhen::IdleSince(at) => (at, false, false, false),
            };
            let rows = storage::lock_blob_retirements(
                &mut tx,
                batch,
                idle_since,
                inclusive,
                keep_private_uploads,
                relay_only,
            )
            .await?;
            for row in rows {
                let Ok(index) = guarded.binary_search_by_key(&row.hash, |(hash, _)| *hash) else {
                    continue;
                };
                match store.delete_held(&row.hash, &guarded[index].1).await {
                    Ok(_) => {
                        if storage::forget_retired_blob(&mut tx, &row.hash).await? {
                            done.blobs = done.blobs.saturating_add(1);
                            done.bytes = done.bytes.saturating_add(row.size);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(hash=%row.hash, %error, "content retirement kept its catalogue row for another try")
                    }
                }
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            break;
        }
    }
    Ok(done)
}

/// Whether file content that passes through the server for `place` is relayed: carried, never kept, and
/// gone with its transfer. It is the setting `storage.relay_transfers` as it resolves there; a place
/// that nobody has asked, or whose setting cannot be read, relays, which is what the setting ships as.
pub async fn relay_on(conn: &mut Conn, place: Place) -> bool {
    orch_settings::get::<bool>(conn, place, keys::STORAGE_RELAY_TRANSFERS)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

/// Where a job's transfer belongs, for the setting: its environment, else its provider's unit, else the
/// system's own rows.
pub(crate) fn place_of(environment: Option<Id>, provider: Option<Id>) -> Place {
    match (environment, provider) {
        (Some(environment), _) => Place::Environment(environment),
        (None, Some(provider)) => Place::Provider(provider),
        (None, None) => Place::System,
    }
}

/// The content a transfer let go of that nothing else held, still to be removed from the store once the
/// transaction that let go has committed.
#[derive(Debug)]
#[must_use = "content that a transfer let go of is removed only by settling it after the commit"]
pub(crate) struct Released {
    freed: Vec<Hash>,
    at: DateTime<Utc>,
}

/// Takes the transfer's pin off each of `hashes`, in the caller's transaction, and keeps the account
/// of what that freed. Called once at every end of a transfer (success, failure or cancel) and never
/// twice for one pin: the callers record their release in the job's plan in the same transaction. For a
/// place that chose to cache, the account is empty: its content waits out the grace as it always did.
pub(crate) async fn release_transfer(
    conn: &mut Conn,
    place: Place,
    hashes: &[Hash],
    at: DateTime<Utc>,
) -> Result<Released, EngineError> {
    let relayed = relay_on(conn, place).await;
    let freed = storage::release_transfer_blobs(conn, hashes, at).await?;
    Ok(Released {
        freed: if relayed { freed } else { Vec::new() },
        at,
    })
}

impl Released {
    /// Removes from the store what the release freed and nothing else wants. It is the step after the
    /// commit: a process that ends before it leaves the blobs unpinned, and the collection removes them
    /// when their grace has run out, as it would any content nothing points at. It never fails the end of
    /// a transfer.
    pub(crate) async fn settle(self, engine: &Engine) {
        if self.freed.is_empty() {
            return;
        }
        let Some(store) = engine.blobs() else {
            return;
        };
        if let Err(error) = retire_blobs(
            engine.db(),
            store.as_ref(),
            &self.freed,
            RetirementWhen::TransferEnded(self.at),
        )
        .await
        {
            tracing::warn!(%error, "the content of a completed transfer waits for the collection");
        }
    }
}

impl Engine {
    /// Lets go, once, of the content a job held for its transfer, which its plan lists under each of `keys`,
    /// and removes from the store what nothing else holds. The release and its record, the plan's `marker`,
    /// are one transaction: a second call finds the marker and does nothing, so one pin is never let go of
    /// twice (a second release would take the pin of another transfer that holds the same content).
    pub(crate) async fn release_held(
        &self,
        job: Id,
        keys: &[&str],
        marker: &str,
    ) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.db().begin().await?;
        let Some(row) = jobs::lock(&mut tx, job).await? else {
            return Ok(());
        };
        if row.plan.get(marker).and_then(Json::as_bool) == Some(true) {
            return Ok(());
        }
        let place = place_of(row.environment_id, row.provider_id);
        let mut released = Vec::new();
        let mut held = false;
        for key in keys {
            let Some(listed) = row.plan.get(*key).filter(|value| value.is_array()) else {
                continue;
            };
            held = true;
            let hashes: Vec<Hash> = serde_json::from_value(listed.clone())?;
            released.push(release_transfer(&mut tx, place, &hashes, now).await?);
        }
        if !held {
            return Ok(());
        }
        jobs::merge_plan(&mut tx, job, &json!({ marker: true }), now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        for each in released {
            each.settle(self).await;
        }
        Ok(())
    }
}

impl Engine {
    /// Holds in the store, for the job, the list of a tree and every file it names, under `key` of the
    /// job's plan: what the source of a live copy uploaded and the destination has yet to fetch. A hash
    /// is held once however often the passes of the job name it, and only a hash that had a row is
    /// recorded, so the release lets go of exactly what was held. The job's end releases it
    /// ([`Engine::release_held`]).
    pub(crate) async fn hold_for_transfer(
        &self,
        job: Id,
        key: &str,
        manifest: Hash,
    ) -> Result<(), EngineError> {
        let mut wanted: BTreeSet<Hash> = self.read_manifest(&manifest).await?.hashes().collect();
        wanted.insert(manifest);
        let mut tx = self.db().begin().await?;
        let Some(row) = jobs::lock(&mut tx, job).await? else {
            return Ok(());
        };
        let mut held: BTreeSet<Hash> = match row.plan.get(key) {
            Some(listed) => serde_json::from_value(listed.clone())?,
            None => BTreeSet::new(),
        };
        let fresh: Vec<Hash> = wanted.difference(&held).copied().collect();
        if fresh.is_empty() {
            return Ok(());
        }
        held.extend(storage::pin_transfer_blobs(&mut tx, &fresh).await?);
        jobs::merge_plan(&mut tx, job, &json!({ key: held }), self.inner.clock.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

impl Engine {
    /// Lets go of what the commands of a job held for it ([`storage::pin_for_command`]) and removes from the store
    /// what that left nothing else holding: the content the machine of the job's transfer uploaded or was told the
    /// system holds. It is asked at every end of a job, whatever it did, and finds nothing the second time.
    pub async fn release_command_pins(&self, job: Id) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let mut tx = self.db().begin().await?;
        let (released, freed) = storage::release_job_command_pins(&mut tx, job, now).await?;
        if released == 0 {
            return Ok(());
        }
        let place = match jobs::job(&mut tx, job).await? {
            Some(row) => place_of(row.environment_id, row.provider_id),
            None => Place::System,
        };
        let relayed = relay_on(&mut tx, place).await;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Released {
            freed: if relayed { freed } else { Vec::new() },
            at: now,
        }
        .settle(self)
        .await;
        Ok(())
    }
}
