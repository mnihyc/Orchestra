//! Skills, their immutable versions, their drafts, and the versions each turn was given.
//!
//! A version's archive is a blob that the version's row pins, as a draft file's row pins its content
//! (triggers of the schema): whoever inserts one registers the blob first
//! ([`crate::queries::storage::register_blob`]). Versions are never changed or removed while their
//! skill exists; a deleted skill keeps them until it is purged, so that what a turn was given can
//! still be named.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};

/// A skill as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SkillRow {
    /// Its identifier.
    pub id: Id,
    /// The root org of its home.
    pub org_id: Id,
    /// Its home: an org or a project.
    pub home_id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// The name its versions' `SKILL.md` gives.
    pub name: String,
    /// The description of its latest version.
    pub description: String,
    /// Where an update takes it from again (`SkillOrigin`), if anywhere.
    pub origin: Option<Json>,
    /// The skill-owned sealed URL for the current reimportable address, if one is needed.
    pub source_url_secret_id: Option<Id>,
    /// Its latest version.
    pub latest_version_id: Option<Id>,
    /// Whether threads may be given it.
    pub enabled: bool,
    /// Its revision.
    pub revision: i64,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it last changed.
    pub updated_at: DateTime<Utc>,
    /// When it was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
}

const SKILL_COLUMNS: &str =
    "id,org_id,home_id,owner_id,name,description,origin,source_url_secret_id,latest_version_id,enabled,revision,
    created_at,updated_at,deleted_at";

/// A version of a skill as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct VersionRow {
    /// Its identifier.
    pub id: Id,
    /// Its skill.
    pub skill_id: Id,
    /// Its number among the skill's versions, from 1.
    pub number: i32,
    /// The name its `SKILL.md` gives.
    pub name: String,
    /// The description its `SKILL.md` gives.
    pub description: String,
    /// The hash of its archive, as stored.
    pub archive: Vec<u8>,
    /// The size of its archive.
    pub archive_size: i64,
    /// The manifest of its files (`SkillFile[]`).
    pub files: Json,
    /// The bytes its files hold together.
    pub size: i64,
    /// Where it came from (`SkillOrigin`).
    pub origin: Json,
    /// Who made it.
    pub created_by: Option<Id>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
}

impl VersionRow {
    /// The hash of its archive.
    pub fn archive_hash(&self) -> Result<Hash, DbError> {
        Hash::from_slice(&self.archive).map_err(|_| {
            DbError::Other(sqlx::Error::Decode(
                "a skill archive hash that is not 32 bytes".into(),
            ))
        })
    }

    /// How many files its manifest lists.
    #[must_use]
    pub fn file_count(&self) -> i64 {
        self.files
            .as_array()
            .map_or(0, |files| i64::try_from(files.len()).unwrap_or(i64::MAX))
    }
}

const VERSION_COLUMNS: &str = "id,skill_id,number,name,description,archive,archive_size,files,size,origin,created_by,created_at";

/// A skill to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSkill {
    /// Its identifier.
    pub id: Id,
    /// Its home: an org or a project.
    pub home_id: Id,
    /// Its first owner.
    pub owner_id: Id,
    /// Its name.
    pub name: String,
}

/// A version to insert: its archive registered by the caller in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVersion {
    /// Its identifier.
    pub id: Id,
    /// Its skill, locked by the caller.
    pub skill_id: Id,
    /// The name its `SKILL.md` gives.
    pub name: String,
    /// The description its `SKILL.md` gives.
    pub description: String,
    /// Its archive.
    pub archive: Hash,
    /// The size of its archive.
    pub archive_size: i64,
    /// Its manifest (`SkillFile[]`).
    pub files: Json,
    /// The bytes its files hold together.
    pub size: i64,
    /// Where it came from (`SkillOrigin`).
    pub origin: Json,
    /// Whether the origin is one an update can take again, which the skill then keeps as its own.
    pub keeps_origin: bool,
    /// Who made it.
    pub created_by: Option<Id>,
}

/// Live skills by identifier.
pub async fn skills(conn: &mut Conn, ids: &[Id]) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLUMNS} FROM skills WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Skills by identifier, deleted ones among them: what a setting or a turn may still name.
pub async fn any_skills(conn: &mut Conn, ids: &[Id]) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLUMNS} FROM skills WHERE id=ANY($1) ORDER BY id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// The live skills whose home lies in the root org `org`: a person's own skills, for their own org.
pub async fn in_org(conn: &mut Conn, org: Id) -> Result<Vec<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLUMNS} FROM skills WHERE org_id=$1 AND deleted_at IS NULL ORDER BY id"
    )))
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// Locks a live skill before a change of it or of its versions.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<SkillRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SKILL_COLUMNS} FROM skills WHERE id=$1 AND deleted_at IS NULL FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The live skill of the name `name` in the home `home`, if there is one.
pub async fn named(conn: &mut Conn, home: Id, name: &str) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM skills WHERE home_id=$1 AND name=$2 AND deleted_at IS NULL",
    )
    .bind(home)
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

/// Inserts a skill without a version; its first version follows in the same transaction.
pub async fn insert(conn: &mut Conn, skill: &NewSkill, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skills (id,org_id,home_id,owner_id,name,created_at,updated_at)
         SELECT $1,n.org_id,n.id,$3,$4,$5,$5 FROM nodes n WHERE n.id=$2",
    )
    .bind(skill.id)
    .bind(skill.home_id)
    .bind(skill.owner_id)
    .bind(&skill.name)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Switches a locked skill on or off.
pub async fn set_enabled(
    conn: &mut Conn,
    id: Id,
    enabled: bool,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE skills SET enabled=$2,revision=revision+1,updated_at=$3 WHERE id=$1")
        .bind(id)
        .bind(enabled)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Keeps the sealed source URL that the skill's current reimportable origin names. The caller
/// has already locked the skill and recorded its version in the same transaction.
pub async fn set_source_url_secret(
    conn: &mut Conn,
    id: Id,
    secret: Option<Id>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE skills SET source_url_secret_id=$2 WHERE id=$1")
        .bind(id)
        .bind(secret)
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes a locked skill. Its versions stay, with their archives, for the turns that name them
/// until the skill is purged; its draft goes.
pub async fn delete(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    discard_draft(conn, id).await?;
    sqlx::query("UPDATE skills SET deleted_at=$2,updated_at=$2,revision=revision+1 WHERE id=$1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Inserts the next version of a locked skill and makes it the skill's latest. The caller has
/// registered the archive in the same transaction; the version's row pins it.
pub async fn insert_version(
    conn: &mut Conn,
    version: &NewVersion,
    at: DateTime<Utc>,
) -> Result<VersionRow, DbError> {
    let row: VersionRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO skill_versions (id,skill_id,number,name,description,archive,archive_size,files,size,origin,
                                     created_by,created_at)
         SELECT $1,$2,COALESCE(MAX(v.number),0)+1,$3,$4,$5,$6,$7,$8,$9,$10,$11
           FROM (SELECT $2::uuid AS skill) s LEFT JOIN skill_versions v ON v.skill_id=s.skill
         RETURNING {VERSION_COLUMNS}"
    )))
    .bind(version.id)
    .bind(version.skill_id)
    .bind(&version.name)
    .bind(&version.description)
    .bind(version.archive.as_bytes().as_slice())
    .bind(version.archive_size)
    .bind(&version.files)
    .bind(version.size)
    .bind(&version.origin)
    .bind(version.created_by)
    .bind(at)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE skills SET latest_version_id=$2,description=$3,
                origin=CASE WHEN $4 THEN $5 ELSE origin END,
                revision=revision+1,updated_at=$6
          WHERE id=$1",
    )
    .bind(version.skill_id)
    .bind(version.id)
    .bind(&version.description)
    .bind(version.keeps_origin)
    .bind(&version.origin)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(row)
}

/// A page of a skill's versions, the newest first: those numbered below `before` when it is given.
pub async fn versions(
    conn: &mut Conn,
    skill: Id,
    before: Option<i32>,
    limit: i64,
) -> Result<Vec<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VERSION_COLUMNS} FROM skill_versions
          WHERE skill_id=$1 AND ($2::int IS NULL OR number<$2)
          ORDER BY number DESC LIMIT $3"
    )))
    .bind(skill)
    .bind(before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Versions by identifier, of whatever skill.
pub async fn versions_by_id(conn: &mut Conn, ids: &[Id]) -> Result<Vec<VersionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {VERSION_COLUMNS} FROM skill_versions WHERE id=ANY($1) ORDER BY id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// How many versions each of the skills `skills` has.
pub async fn version_counts(conn: &mut Conn, skills: &[Id]) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT skill_id,count(*) FROM skill_versions WHERE skill_id=ANY($1) GROUP BY skill_id",
    )
    .bind(skills)
    .fetch_all(conn)
    .await?)
}

/// The last writer of each row of `agents.skills` on the places `chain`: whoever switched the
/// skills of that row on, whose use of each is asked when a thread is given it.
pub async fn setting_writers(
    conn: &mut Conn,
    chain: &[Id],
) -> Result<Vec<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id,updated_by FROM settings WHERE key='agents.skills' AND node_id=ANY($1)",
    )
    .bind(chain)
    .fetch_all(conn)
    .await?)
}

/// Records the skill versions a turn was given when it started.
pub async fn record_turn(conn: &mut Conn, turn: Id, versions: &[Id]) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE turns SET skill_versions=$2 WHERE id=$1")
            .bind(turn)
            .bind(versions)
            .execute(conn)
            .await?
            .rows_affected()
            == 1,
    )
}

/// The skill versions a turn was given; nothing where none were recorded.
pub async fn turn_versions(conn: &mut Conn, turn: Id) -> Result<Option<Vec<Id>>, DbError> {
    Ok(
        sqlx::query_scalar::<_, Option<Vec<Id>>>("SELECT skill_versions FROM turns WHERE id=$1")
            .bind(turn)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// Removes a skill's draft; its files' rows go with it, and with them their pins. Whether there was
/// one is the answer.
pub async fn discard_draft(conn: &mut Conn, skill: Id) -> Result<bool, DbError> {
    Ok(sqlx::query("DELETE FROM skill_drafts WHERE skill_id=$1")
        .bind(skill)
        .execute(conn)
        .await?
        .rows_affected()
        == 1)
}

/// How a skill's draft stands, without its files.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DraftInfoRow {
    /// The skill.
    pub skill_id: Id,
    /// The version it was started from.
    pub based_on: Option<Id>,
    /// Its revision.
    pub revision: i64,
    /// Who changed it last.
    pub updated_by: Id,
    /// When it was changed last.
    pub updated_at: DateTime<Utc>,
    /// How many files it holds.
    pub file_count: i64,
}

/// The drafts of the skills `skills`, where they have one.
pub async fn draft_infos(conn: &mut Conn, skills: &[Id]) -> Result<Vec<DraftInfoRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.skill_id,d.based_on,d.revision,d.updated_by,d.updated_at,
                (SELECT count(*) FROM skill_draft_files f WHERE f.skill_id=d.skill_id) AS file_count
           FROM skill_drafts d WHERE d.skill_id=ANY($1) ORDER BY d.skill_id",
    )
    .bind(skills)
    .fetch_all(conn)
    .await?)
}

/// Who started the thread `thread`, and their own org: whose own skills the thread may be given.
pub async fn thread_starter(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.created_by,u.personal_org_id FROM nodes n JOIN users u ON u.id=n.created_by
          WHERE n.id=$1 AND n.kind='thread' AND n.deleted_at IS NULL",
    )
    .bind(thread)
    .fetch_optional(conn)
    .await?)
}

/// One file of a draft as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct DraftFileRow {
    /// Where it lies in the skill.
    pub path: String,
    /// The hash of its content, as stored.
    pub blob: Vec<u8>,
    /// Its size.
    pub size: i64,
    /// Whether it is executable.
    pub executable: bool,
    /// Who wrote it last.
    pub updated_by: Id,
    /// When it was written last.
    pub updated_at: DateTime<Utc>,
}

impl DraftFileRow {
    /// The hash of its content.
    pub fn blob_hash(&self) -> Result<Hash, DbError> {
        Hash::from_slice(&self.blob).map_err(|_| {
            DbError::Other(sqlx::Error::Decode(
                "a draft file hash that is not 32 bytes".into(),
            ))
        })
    }
}

/// A file to put into a draft, its content registered as a blob by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftFile {
    /// Where it lies in the skill.
    pub path: String,
    /// Its content.
    pub blob: Hash,
    /// Its size.
    pub size: i64,
    /// Whether it is executable.
    pub executable: bool,
}

/// The draft of the skill `skill`, locked until the transaction ends.
pub async fn lock_draft(conn: &mut Conn, skill: Id) -> Result<Option<DraftInfoRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT d.skill_id,d.based_on,d.revision,d.updated_by,d.updated_at,
                (SELECT count(*) FROM skill_draft_files f WHERE f.skill_id=d.skill_id) AS file_count
           FROM skill_drafts d WHERE d.skill_id=$1 FOR UPDATE",
    )
    .bind(skill)
    .fetch_optional(conn)
    .await?)
}

/// The files of the draft of the skill `skill`, in the order of their paths.
pub async fn draft_files(conn: &mut Conn, skill: Id) -> Result<Vec<DraftFileRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT path,blob,size,executable,updated_by,updated_at FROM skill_draft_files
          WHERE skill_id=$1 ORDER BY path COLLATE \"C\"",
    )
    .bind(skill)
    .fetch_all(conn)
    .await?)
}

/// Starts the draft of a locked skill that has none, holding `files`.
pub async fn start_draft(
    conn: &mut Conn,
    skill: Id,
    based_on: Option<Id>,
    files: &[DraftFile],
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skill_drafts (skill_id,based_on,created_by,created_at,updated_by,updated_at)
         VALUES ($1,$2,$3,$4,$3,$4)",
    )
    .bind(skill)
    .bind(based_on)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    for file in files {
        put_file_row(conn, skill, file, by, at).await?;
    }
    Ok(())
}

async fn put_file_row(
    conn: &mut Conn,
    skill: Id,
    file: &DraftFile,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO skill_draft_files (skill_id,path,blob,size,executable,updated_by,updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         ON CONFLICT (skill_id,path) DO UPDATE SET blob=EXCLUDED.blob,size=EXCLUDED.size,
             executable=EXCLUDED.executable,updated_by=EXCLUDED.updated_by,updated_at=EXCLUDED.updated_at",
    )
    .bind(skill)
    .bind(&file.path)
    .bind(file.blob.as_bytes().as_slice())
    .bind(file.size)
    .bind(file.executable)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Raises the revision of a locked draft after a change of its files.
async fn touch_draft(conn: &mut Conn, skill: Id, by: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE skill_drafts SET revision=revision+1,updated_by=$2,updated_at=$3 WHERE skill_id=$1",
    )
    .bind(skill)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Writes one file of a locked draft, in place of one at its path.
pub async fn put_draft_file(
    conn: &mut Conn,
    skill: Id,
    file: &DraftFile,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    put_file_row(conn, skill, file, by, at).await?;
    touch_draft(conn, skill, by, at).await
}

/// Removes one file of a locked draft; whether it was there is the answer.
pub async fn delete_draft_file(
    conn: &mut Conn,
    skill: Id,
    path: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let gone = sqlx::query("DELETE FROM skill_draft_files WHERE skill_id=$1 AND path=$2")
        .bind(skill)
        .bind(path)
        .execute(&mut *conn)
        .await?
        .rows_affected()
        == 1;
    if gone {
        touch_draft(conn, skill, by, at).await?;
    }
    Ok(gone)
}
