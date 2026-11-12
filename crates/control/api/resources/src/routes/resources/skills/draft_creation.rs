//! The new-draft branch consumes actual checked creation/blob operands at its one header writer.

use super::*;

use orch_blob_contract::range::BlobMeta;
pub(crate) struct Prepared {
    transaction: String,
    id: Id,
    home: Id,
    owner: Id,
    name: String,
    access: NewAccess,
    files: Vec<DraftFile>,
}

async fn transaction(conn: &mut Conn) -> ApiResult<String> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(conn)
        .await
        .map_err(DbError::from)?)
}

fn taken(name: &str) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictExists,
        format!("another skill here is named {name:?} already"),
    )
    .with("name", name)
}

pub(super) async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    home: Id,
    name: &str,
    access: NewAccess,
    meta: BlobMeta,
) -> ApiResult<Prepared> {
    prepare_files(
        conn,
        principal,
        id,
        home,
        name,
        access,
        vec![DraftFile {
            path: SKILL_FILE.into(),
            blob: meta.hash,
            size: meta.size,
            executable: false,
        }],
    )
    .await
}

/// The same native draft header writer accepts a selected native file set, without reading bytes in a transaction.
pub(crate) async fn prepare_files(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    home: Id,
    name: &str,
    access: NewAccess,
    files: Vec<DraftFile>,
) -> ApiResult<Prepared> {
    may_make_at(conn, principal, home).await?;
    if rows::named(conn, home, name).await?.is_some() {
        return Err(taken(name));
    }
    let blobs = files.iter().map(|file| file.blob).collect::<Vec<_>>();
    orch_db::queries::uploads::lock_blobs(conn, &blobs).await?;
    Ok(Prepared {
        transaction: transaction(conn).await?,
        id,
        home,
        owner: principal.user(),
        name: name.into(),
        access,
        files,
    })
}

/// A real current caller/source guard owns this callback; the packet itself grants no creation right.
pub(crate) async fn apply<F>(
    conn: &mut Conn,
    principal: &Principal,
    plan: Prepared,
    at: chrono::DateTime<chrono::Utc>,
    before: F,
) -> ApiResult<SkillRow>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    if !orch_db::queries::transaction_guard::in_transaction(conn)
        || principal.user() != plan.owner
        || transaction(conn).await? != plan.transaction
    {
        return Err(ApiError::internal(
            "the native skill draft belongs to another actor or transaction",
        ));
    }
    may_make_at(conn, principal, plan.home).await?;
    if rows::named(conn, plan.home, &plan.name).await?.is_some() {
        return Err(taken(&plan.name));
    }
    before()?;
    rows::insert(
        conn,
        &rows::NewSkill {
            id: plan.id,
            home_id: plan.home,
            owner_id: principal.user(),
            name: plan.name,
        },
        at,
    )
    .await?;
    ownership::start_resource(
        conn,
        ObjectRef {
            kind: ResourceKind::Skill.into(),
            id: plan.id,
        },
        principal.user(),
        plan.access,
        Time::new(at),
    )
    .await
    .map_err(ApiError::internal)?;
    for file in &plan.files {
        storage::register_blob(conn, &file.blob, file.size, at).await?;
    }
    rows::start_draft(conn, plan.id, None, &plan.files, principal.user(), at).await?;
    rows::lock(conn, plan.id)
        .await?
        .ok_or_else(ApiError::not_found)
}

#[cfg(any(test, feature = "test-support"))]
#[path = "draft_creation/tests.rs"]
pub mod tests;
