//! Protected metadata from a completed database dump/manifest pair; reconciliation never takes another dump.

use orch_api_contract::admin::{BackupCause, BackupFailure, DatabaseBackupRecord};
use orch_db::queries::installation;
use orch_db::queries::notifications::upkeep;
use orch_db::{Db, DbError};
use orch_types::{Time, new_id};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub mod inventory;
pub mod storage;

/// The one protected publication read on startup and by Management.
pub const RECORD_FILE: &str = "latest-database-backup.json";
/// One MiB is ample for this small record, without accepting an unbounded local response.
const RECORD_MAX_BYTES: u64 = 1_048_576;
/// The inventory may be larger than its small summary: sixteen MiB bounds metadata, not dump bytes.
const MANIFEST_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Streaming hash memory is independent of a database dump's size.
const HASH_CHUNK: usize = 1024 * 1024;

/// The dump named by a completed manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dump {
    /// Plain relative file name beside the manifest.
    pub name: String,
    /// Verified custom-format dump size.
    pub bytes: u64,
    /// BLAKE3 of the completed dump bytes.
    pub hash: String,
}

/// One database point and the external bytes that it explicitly excludes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// First supported format.
    pub format_version: u32,
    /// SHA-256 fingerprint of the installation's authority certificate, stable across service renewals.
    pub system_id: String,
    /// Release at the database point.
    pub release: String,
    /// Applied schema at the same snapshot.
    pub schema: String,
    /// Snapshot time, never a later HTTP timestamp.
    pub database_at: Time,
    /// Completion of the dump and inventory pair.
    pub completed_at: Time,
    /// Original cause, preserved when only the metadata is repaired later.
    pub cause: BackupCause,
    /// The actual dump.
    pub dump: Dump,
    /// Database-only included inventory from that snapshot.
    pub included: serde_json::Value,
    /// External locations/reasons, without reading credentials or remote volumes.
    pub excluded: serde_json::Value,
}

/// A verified successful pair and the protected manifest copy which vouches for its summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Published {
    /// Installation identity at publication.
    pub system_id: String,
    /// Small, safe public projection.
    pub record: DatabaseBackupRecord,
    /// Plain name of an immutable manifest copy in this record directory.
    pub manifest_file: String,
    /// BLAKE3 of that exact manifest file.
    pub manifest_hash: String,
}

/// Atomic local publication: a failed attempt preserves its previous successful pair.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Publication {
    /// Latest verified success, or none before one completed.
    pub success: Option<Published>,
    /// Most recent failed local attempt, without advancing success.
    pub last_failure: Option<BackupFailure>,
}

/// A local metadata refusal or a database failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Safe reason without credential contents.
    #[error("{0}")]
    Local(&'static str),
    /// Persistence failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// Stored metadata could not be decoded.
    #[error("the stored backup metadata does not read")]
    Json(#[from] serde_json::Error),
}

fn plain(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>, Error> {
    let meta = fs::symlink_metadata(path)
        .map_err(|_| Error::Local("the protected backup metadata is unreadable"))?;
    if !meta.is_file() || meta.len() > limit {
        return Err(Error::Local(
            "the protected backup metadata is not a bounded regular file",
        ));
    }
    let mut result = Vec::new();
    File::open(path)
        .map_err(|_| Error::Local("the protected backup metadata is unreadable"))?
        .take(limit + 1)
        .read_to_end(&mut result)
        .map_err(|_| Error::Local("the protected backup metadata is unreadable"))?;
    if result.len() as u64 > limit {
        return Err(Error::Local(
            "the protected backup metadata exceeds its bound",
        ));
    }
    Ok(result)
}

/// The size and BLAKE3 of one completed regular dump, without loading it into memory.
pub fn hash_file(path: &Path) -> Result<(u64, String), Error> {
    let meta =
        fs::symlink_metadata(path).map_err(|_| Error::Local("the completed dump is unreadable"))?;
    if !meta.is_file() {
        return Err(Error::Local("the completed dump must be a regular file"));
    }
    let mut file =
        File::open(path).map_err(|_| Error::Local("the completed dump is unreadable"))?;
    let mut hash = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0; HASH_CHUNK];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| Error::Local("the completed dump could not be hashed"))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok((bytes, hash.finalize().to_hex().to_string()))
}

/// Writes/syncs beside the target and renames once; a torn write never replaces the visible record.
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Local(
        "the protected metadata has no parent directory",
    ))?;
    let temporary = parent.join(format!(".backup-{}.partial", new_id().simple()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| Error::Local("the protected backup record could not be staged"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| Error::Local("the protected backup record could not be synced"))?;
    fs::rename(&temporary, path)
        .map_err(|_| Error::Local("the protected backup record could not be published"))?;
    File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| Error::Local("the protected backup record directory could not be synced"))?;
    Ok(())
}

fn publication(dir: &Path) -> Result<Publication, Error> {
    let path = dir.join(RECORD_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Publication::default()),
        Err(_) => Err(Error::Local("the protected backup record is unreadable")),
        Ok(_) => Ok(serde_json::from_slice(&read(&path, RECORD_MAX_BYTES)?)?),
    }
}

/// Publishes metadata from an already-completed pair. This is also the repair path; it never runs pg_dump.
pub fn publish_pair(
    dir: &Path,
    dump: &Path,
    manifest: &Path,
    system_id: &str,
) -> Result<Published, Error> {
    let bytes = read(manifest, MANIFEST_MAX_BYTES)?;
    let parsed: Manifest = serde_json::from_slice(&bytes)?;
    let name = dump
        .file_name()
        .and_then(|part| part.to_str())
        .ok_or(Error::Local("the dump has no plain relative name"))?;
    if parsed.format_version != 1
        || parsed.system_id != system_id
        || !plain(name)
        || parsed.dump.name != name
        || parsed.completed_at < parsed.database_at
        || !parsed.included.is_object()
        || !parsed.excluded.is_object()
    {
        return Err(Error::Local(
            "the manifest does not match this installation and completed database-only pair",
        ));
    }
    let (size, hash) = hash_file(dump)?;
    if size == 0 || parsed.dump.bytes != size || parsed.dump.hash != hash {
        return Err(Error::Local(
            "the dump bytes do not match their completed manifest",
        ));
    }
    let manifest_hash = blake3::hash(&bytes).to_hex().to_string();
    let manifest_file = format!("backup-{manifest_hash}.manifest.json");
    let success = Published {
        system_id: system_id.to_owned(),
        manifest_file: manifest_file.clone(),
        manifest_hash,
        record: DatabaseBackupRecord {
            completed_at: parsed.completed_at,
            database_at: parsed.database_at,
            release: parsed.release,
            schema: parsed.schema,
            dump_name: name.to_owned(),
            bytes: size,
            hash,
            cause: parsed.cause,
            database_only: true,
            last_failure: None,
        },
    };
    let mut held = publication(dir)?;
    if held
        .success
        .as_ref()
        .is_some_and(|previous| previous.record.completed_at > success.record.completed_at)
    {
        return Err(Error::Local(
            "the pair is older than the last successfully recorded backup",
        ));
    }
    atomic(&dir.join(manifest_file), &bytes)?;
    held.success = Some(success.clone());
    held.last_failure = None;
    atomic(&dir.join(RECORD_FILE), &serde_json::to_vec(&held)?)?;
    Ok(success)
}

/// Records only a safe failed attempt, retaining the previous successful completion.
pub fn failed(dir: &Path, at: Time, reason: &'static str) -> Result<(), Error> {
    let mut held = publication(dir)?;
    held.last_failure = Some(BackupFailure {
        at,
        reason: reason.to_owned(),
    });
    atomic(&dir.join(RECORD_FILE), &serde_json::to_vec(&held)?)
}

fn validated(dir: &Path, system_id: &str) -> Result<Publication, Error> {
    validated_manifest(dir, system_id).map(|(publication, _)| publication)
}

fn validated_manifest(
    dir: &Path,
    system_id: &str,
) -> Result<(Publication, Option<Manifest>), Error> {
    let mut held = publication(dir)?;
    let mut verified = None;
    if let Some(success) = &held.success {
        if success.system_id != system_id || !plain(&success.manifest_file) {
            return Err(Error::Local(
                "the protected backup record belongs to another installation",
            ));
        }
        let bytes = read(&dir.join(&success.manifest_file), MANIFEST_MAX_BYTES)?;
        if blake3::hash(&bytes).to_hex().as_str() != success.manifest_hash {
            return Err(Error::Local(
                "the protected backup manifest hash does not match",
            ));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        if manifest.system_id != system_id
            || manifest.format_version != 1
            || !plain(&manifest.dump.name)
            || manifest.completed_at < manifest.database_at
            || manifest.dump.name != success.record.dump_name
            || manifest.dump.bytes != success.record.bytes
            || manifest.dump.hash != success.record.hash
            || manifest.completed_at != success.record.completed_at
            || manifest.database_at != success.record.database_at
            || manifest.release != success.record.release
            || manifest.schema != success.record.schema
            || manifest.cause != success.record.cause
            || !success.record.database_only
        {
            return Err(Error::Local(
                "the protected backup summary differs from its completed manifest",
            ));
        }
        verified = Some(manifest);
    }
    if let Some(success) = &mut held.success {
        success.record.last_failure.clone_from(&held.last_failure);
    }
    Ok((held, verified))
}

/// Imports the same protected file at startup and on read; invalid/older data never advances success.
pub async fn reconcile(
    db: &Db,
    dir: &Path,
    system_id: &str,
    now: Time,
) -> Result<(Option<DatabaseBackupRecord>, Option<String>), Error> {
    let local = validated(dir, system_id);
    let mut tx = db.begin().await?;
    let acquired = upkeep::try_lock_job(&mut tx, "database_backup_record").await?;
    let mut held = installation::get(&mut tx, "database_backup")
        .await?
        .map(serde_json::from_value::<Publication>)
        .transpose()?
        .unwrap_or_default();
    if !acquired {
        return Ok((held.success.map(|row| row.record), None));
    }
    let error = match local {
        Ok(local) => {
            if let Some(success) = local.success {
                if held.success.as_ref().is_none_or(|previous| {
                    success.record.completed_at >= previous.record.completed_at
                }) {
                    held.success = Some(success);
                    held.last_failure.clone_from(&local.last_failure);
                } else {
                    return Ok((
                        held.success.map(|row| row.record),
                        Some(
                            "the protected backup record is older than the last imported success"
                                .to_owned(),
                        ),
                    ));
                }
            }
            if let Some(failed) = local.last_failure
                && held
                    .last_failure
                    .as_ref()
                    .is_none_or(|previous| failed.at > previous.at)
            {
                held.last_failure = Some(failed);
            }
            None
        }
        Err(error) => Some(error.to_string()),
    };
    if let Some(success) = &mut held.success {
        success.record.last_failure.clone_from(&held.last_failure);
    }
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    installation::put(
        &mut tx,
        "database_backup",
        &serde_json::to_value(&held)?,
        now.get(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok((held.success.map(|row| row.record), error))
}

/// Warn on the last success's age; a later failure cannot reset that age.
#[must_use]
pub fn warn(now: Time, record: Option<&DatabaseBackupRecord>, days: i64) -> bool {
    record.is_none_or(|record| {
        now.get() - record.completed_at.get() > crate::spans::days(days.max(1) as u64)
    })
}
