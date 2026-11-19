//! Making a version of a skill: its files checked and written as one archive, the archive kept in the blob store and pinned, and the version recorded as the skill's latest.
//!
//! Every source ends here, whatever it is: an upload, a repository or an address fetched by a host, an
//! environment's directory, a draft. The work is in two halves so that no transaction waits on the
//! store: [`prepare_archive`] and [`prepare_files`] read and check the files and put the archive into
//! the store outside any transaction, and [`record`] registers the archive and writes the version,
//! whose row pins it, inside the caller's. An archive put into the store whose transaction then fails
//! is pinned by nothing and is collected like any other unreferenced blob.

use std::sync::Arc;

use bytes::Bytes;
use orch_api_contract::skills::SkillOrigin;
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_db::queries::resources::skills::{
    self as rows, NewSkill, NewVersion, SkillRow, VersionRow,
};
use orch_db::queries::storage;
use orch_db::{Conn, DbError};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Hash, Id, Time};

use super::archive::{self, Built, File, Limits, SkillFault};
use crate::EngineError;

/// Why a version was not made.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    /// The files are not a skill Orch keeps; the reason is for the person who sent them.
    #[error("{0}")]
    Refused(#[from] SkillFault),
    /// A version keeps the name of its skill.
    #[error(
        "The skill file names the skill {found:?}; this skill is {skill:?}, and every version keeps its name"
    )]
    NameChanged {
        /// The skill's name.
        skill: String,
        /// The name the new files give.
        found: String,
    },
    /// Another live skill of the same home has the name.
    #[error("another skill here is named {name:?} already")]
    NameTaken {
        /// The name.
        name: String,
    },
    /// The skill is not there, or was deleted.
    #[error("the skill is not there")]
    Gone,
    /// The blob store failed.
    #[error("the archive could not be kept: {0}")]
    Store(String),
    /// The engine failed otherwise.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<DbError> for SkillError {
    fn from(error: DbError) -> Self {
        Self::Engine(error.into())
    }
}

/// A version ready to be recorded: checked, with its archive in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// What was made of the files.
    pub built: Built,
    /// The archive's hash in the store.
    pub archive: Hash,
}

/// What a version is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A new skill in a home, first owned by `owner`; its first version is this one.
    NewSkill {
        /// The new skill's identifier.
        id: Id,
        /// Its home: an org or a project.
        home: Id,
        /// Its first owner.
        owner: Id,
    },
    /// Another version of an existing skill.
    Skill(Id),
}

/// A version recorded, with its skill as it now stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The skill.
    pub skill: SkillRow,
    /// The version.
    pub version: VersionRow,
}

/// What one version may hold at a home: `skills.max_bytes` and `skills.max_files` resolved there.
pub async fn limits_at(conn: &mut Conn, home: Id) -> Result<Limits, EngineError> {
    let bytes = registry::get(keys::SKILLS_MAX_BYTES)
        .ok_or_else(|| EngineError::unreadable("the size of a skill is not registered"))?;
    let files = registry::get(keys::SKILLS_MAX_FILES)
        .ok_or_else(|| EngineError::unreadable("the files of a skill are not registered"))?;
    let snapshot = Snapshot::load(conn, Place::Node(home), Wanted::Keys(&[bytes, files])).await?;
    let number = |def: &'static registry::SettingDef| {
        snapshot
            .resolve(def)
            .value
            .as_ref()
            .and_then(serde_json::Value::as_u64)
            .or_else(|| def.default_value().and_then(|value| value.as_u64()))
            .ok_or_else(|| EngineError::unreadable("a limit of a skill does not read"))
    };
    Ok(Limits {
        max_bytes: number(bytes)?,
        max_files: usize::try_from(number(files)?).unwrap_or(usize::MAX),
    })
}

/// Reads an archive's skill files, checks them, and puts the version's archive into the store.
/// The archive is read on a thread that may block.
pub async fn prepare_archive(
    store: &Arc<dyn BlobStore>,
    bytes: Bytes,
    folder: Option<String>,
    limits: Limits,
) -> Result<Prepared, SkillError> {
    let built = tokio::task::spawn_blocking(move || {
        archive::read_archive(&bytes, folder.as_deref(), limits)
            .and_then(|files| archive::build(files, limits))
    })
    .await
    .map_err(EngineError::unreadable)??;
    keep(store, built).await
}

/// Checks the files `files` and puts the version's archive into the store.
pub async fn prepare_files(
    store: &Arc<dyn BlobStore>,
    files: Vec<File>,
    limits: Limits,
) -> Result<Prepared, SkillError> {
    let built = tokio::task::spawn_blocking(move || archive::build(files, limits))
        .await
        .map_err(EngineError::unreadable)??;
    keep(store, built).await
}

async fn keep(store: &Arc<dyn BlobStore>, built: Built) -> Result<Prepared, SkillError> {
    let archive = Hash::of(&built.archive);
    let size = i64::try_from(built.archive.len()).unwrap_or(i64::MAX);
    store
        .put(
            &archive,
            Some(size),
            stream_of(vec![Bytes::from(built.archive.clone())]),
        )
        .await
        .map_err(|error| SkillError::Store(error.to_string()))?;
    Ok(Prepared { built, archive })
}

/// Records a prepared version inside the caller's transaction: for a new skill the skill itself,
/// whose page the caller then starts; for an existing one after locking it, refusing files that
/// name it otherwise. The archive is registered, and the version's row pins it.
pub async fn record(
    conn: &mut Conn,
    target: Target,
    prepared: &Prepared,
    origin: &SkillOrigin,
    by: Option<Id>,
    at: Time,
) -> Result<Made, SkillError> {
    let built = &prepared.built;
    let skill = match target {
        Target::NewSkill { id, home, owner } => {
            if rows::named(conn, home, &built.name).await?.is_some() {
                return Err(SkillError::NameTaken {
                    name: built.name.clone(),
                });
            }
            rows::insert(
                conn,
                &NewSkill {
                    id,
                    home_id: home,
                    owner_id: owner,
                    name: built.name.clone(),
                },
                at.get(),
            )
            .await
            .map_err(|error| match error {
                DbError::Unique { .. } => SkillError::NameTaken {
                    name: built.name.clone(),
                },
                other => other.into(),
            })?;
            id
        }
        Target::Skill(id) => {
            let skill = rows::lock(conn, id).await?.ok_or(SkillError::Gone)?;
            if skill.name != built.name {
                return Err(SkillError::NameChanged {
                    skill: skill.name,
                    found: built.name.clone(),
                });
            }
            id
        }
    };
    let archive_size = i64::try_from(built.archive.len()).unwrap_or(i64::MAX);
    // The version's row pins the archive for as long as it exists.
    storage::register_blob(conn, &prepared.archive, archive_size, at.get()).await?;
    let version = rows::insert_version(
        conn,
        &NewVersion {
            id: orch_types::new_id(),
            skill_id: skill,
            name: built.name.clone(),
            description: built.description.clone(),
            archive: prepared.archive,
            archive_size,
            files: serde_json::to_value(&built.manifest).map_err(EngineError::from)?,
            size: built.size,
            origin: serde_json::to_value(origin).map_err(EngineError::from)?,
            keeps_origin: origin.can_be_taken_again(),
            created_by: by,
        },
        at.get(),
    )
    .await?;
    let skill = rows::lock(conn, skill).await?.ok_or(SkillError::Gone)?;
    Ok(Made { skill, version })
}
