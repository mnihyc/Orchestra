//! A skill's source address: the public location in its origin, the private address sealed for
//! reimport and for the machine that fetches it, and old origins cleaned when they are read.

use super::{ApiError, ApiResult, SkillOrigin, invalid};
use crate::routes::resources::sealing;
use chrono::{DateTime, Utc};
use orch_db::queries::settings::{self as secrets, SecretName};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json, new_id};

/// What a public origin says when some of the address is kept privately.
const HIDDEN_QUERY: &str = "?…";

/// The address a reader may see and whether the original needs sealed custody. Treat every
/// query and fragment as private: an unrecognized parameter name may still be a credential.
#[must_use]
pub(super) fn public_url(raw: &str) -> (String, bool) {
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return ("(source address hidden)".to_owned(), true);
    };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return ("(source address hidden)".to_owned(), true);
    }
    let private = !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some();
    if !private {
        return (raw.to_owned(), false);
    }
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return ("(source address hidden)".to_owned(), true);
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    (format!("{parsed}{HIDDEN_QUERY}"), true)
}

/// The projection is safe even before an older origin has been rewritten in storage.
#[must_use]
pub(super) fn public_origin(mut origin: SkillOrigin) -> SkillOrigin {
    if let SkillOrigin::Url { url, .. } = &mut origin {
        *url = public_url(url).0;
    }
    origin
}

/// Stores a fresh private source for a fetch, before the machine is asked to read it. Its name
/// is unique to the invocation so that a concurrent import never changes an in-flight command.
pub(super) async fn stage(
    conn: &mut Conn,
    skill: Id,
    raw: &str,
    by: Id,
    at: DateTime<Utc>,
) -> ApiResult<(String, Option<SecretName>)> {
    let (shown, private) = public_url(raw);
    if !private {
        return Ok((shown, None));
    }
    if raw.len() > sealing::SECRET_MAX_BYTES {
        return Err(invalid("from.url", "the source address is too long"));
    }
    let name = format!("source_url:{}", new_id());
    let secret = sealing::put_skill_source_secret(conn, skill, &name, raw, Some(by), at).await?;
    Ok((shown, Some(secret)))
}

/// Abandons a source that did not become the skill's current origin.
pub(super) async fn abandon(conn: &mut Conn, skill: Id, secret: &SecretName) -> ApiResult<()> {
    secrets::delete_secret(conn, sealing::SKILL, skill, &secret.name).await?;
    Ok(())
}

struct CurrentMigration {
    skill: Id,
    old: Json,
    public: Json,
    pointer: Option<Id>,
    sealed: Option<Vec<u8>>,
}
struct VersionMigration {
    id: Id,
    skill: Id,
    old: Json,
    public: Json,
    name: String,
    sealed: Vec<u8>,
}
/// Opaque native URL-custody maintenance on its actual transaction and locked source rows.
pub(super) struct Migration {
    transaction: String,
    current: Vec<CurrentMigration>,
    versions: Vec<VersionMigration>,
    audits: Vec<Id>,
}

async fn transaction(conn: &mut Conn) -> ApiResult<String> {
    Ok(sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(conn)
        .await
        .map_err(DbError::from)?)
}
fn seal(skill: Id, name: &str, raw: &str) -> ApiResult<Vec<u8>> {
    sealing::installed_master_key()?
        .seal(
            raw.as_bytes(),
            &sealing::binding(sealing::SKILL, skill, name),
        )
        .map_err(|_| ApiError::internal("a skill source could not be sealed"))
}

/// Acquire the native source/secret/audit footprint and seal supplied legacy values before writing.
pub(super) async fn prepare_migration(conn: &mut Conn, skills: &[Id]) -> ApiResult<Migration> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(ApiError::internal(
            "native skill source maintenance requires its action transaction",
        ));
    }
    let current: Vec<(Id, Option<Json>, Option<Id>)> = sqlx::query_as(
        "SELECT id,origin,source_url_secret_id FROM skills WHERE id=ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(skills)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let versions: Vec<(Id, Id, Json)> = sqlx::query_as(
        "SELECT id,skill_id,origin FROM skill_versions WHERE skill_id=ANY($1) ORDER BY skill_id,id FOR UPDATE")
        .bind(skills).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    sqlx::query("SELECT id FROM secrets WHERE owner_kind='skill' AND owner_id=ANY($1) ORDER BY owner_id,name FOR UPDATE")
        .bind(skills).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let audits: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM audit_log WHERE object_kind='skill' AND object_id=ANY($1) AND details ? 'origin' ORDER BY id FOR UPDATE")
        .bind(skills).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut selected = Vec::new();
    for (skill, old, pointer) in current {
        let Some(old) = old else { continue };
        let origin: SkillOrigin =
            serde_json::from_value(old.clone()).map_err(ApiError::internal)?;
        let SkillOrigin::Url { url, .. } = &origin else {
            continue;
        };
        let (shown, private) = public_url(url);
        if !private || shown == url.as_str() {
            continue;
        }
        let sealed = if pointer.is_none() {
            Some(seal(skill, "source_url:legacy_current", url)?)
        } else {
            None
        };
        let mut public = origin;
        if let SkillOrigin::Url { url, .. } = &mut public {
            *url = shown;
        }
        selected.push(CurrentMigration {
            skill,
            old,
            public: serde_json::to_value(public).map_err(ApiError::internal)?,
            pointer,
            sealed,
        });
    }
    let mut historical = Vec::new();
    for (id, skill, old) in versions {
        let origin: SkillOrigin =
            serde_json::from_value(old.clone()).map_err(ApiError::internal)?;
        let SkillOrigin::Url { url, .. } = &origin else {
            continue;
        };
        let (shown, private) = public_url(url);
        if !private || shown == url.as_str() {
            continue;
        }
        let name = format!("source_url:legacy_version:{id}");
        let sealed = seal(skill, &name, url)?;
        let mut public = origin;
        if let SkillOrigin::Url { url, .. } = &mut public {
            *url = shown;
        }
        historical.push(VersionMigration {
            id,
            skill,
            old,
            public: serde_json::to_value(public).map_err(ApiError::internal)?,
            name,
            sealed,
        });
    }
    Ok(Migration {
        transaction: transaction(conn).await?,
        current: selected,
        versions: historical,
        audits,
    })
}

/// Consume exactly the locked native maintenance packet, checking immediately before its first write.
pub(super) async fn apply_migration<F>(
    conn: &mut Conn,
    plan: Migration,
    at: DateTime<Utc>,
    before: F,
) -> ApiResult<bool>
where
    F: FnOnce() -> ApiResult<()> + Send,
{
    if transaction(conn).await? != plan.transaction {
        return Err(ApiError::internal(
            "the native skill source maintenance belongs to another transaction",
        ));
    }
    let changed = !plan.current.is_empty() || !plan.versions.is_empty() || !plan.audits.is_empty();
    if !changed {
        return Ok(false);
    }
    before()?;
    for row in plan.current {
        let pointer = match row.sealed {
            Some(sealed) => Some(
                secrets::put_secret(
                    conn,
                    sealing::SKILL,
                    row.skill,
                    "source_url:legacy_current",
                    &sealed,
                    None,
                    at,
                )
                .await?
                .id,
            ),
            None => row.pointer,
        };
        sqlx::query("UPDATE skills SET origin=$2,source_url_secret_id=COALESCE(source_url_secret_id,$3) WHERE id=$1 AND origin=$4")
            .bind(row.skill).bind(row.public).bind(pointer).bind(row.old).execute(&mut *conn).await.map_err(DbError::from)?;
    }
    for row in plan.versions {
        secrets::put_secret(
            conn,
            sealing::SKILL,
            row.skill,
            &row.name,
            &row.sealed,
            None,
            at,
        )
        .await?;
        sqlx::query("UPDATE skill_versions SET origin=$2 WHERE id=$1 AND origin=$3")
            .bind(row.id)
            .bind(row.public)
            .bind(row.old)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    if !plan.audits.is_empty() {
        sqlx::query(
            "UPDATE audit_log SET details=details-'origin' WHERE id=ANY($1) AND details ? 'origin'",
        )
        .bind(plan.audits)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    }
    Ok(true)
}

/// Preserve the original legacy maintenance API with one rollbackable native transaction.
pub(super) async fn migrate_many(
    conn: &mut Conn,
    skills: &[Id],
    at: DateTime<Utc>,
) -> ApiResult<()> {
    if skills.is_empty() {
        return Ok(());
    }
    // Complete any deployment-key file access before taking the native source locks.
    sealing::master_key()?;
    use sqlx::Connection;
    let mut tx = conn.begin().await.map_err(DbError::from)?;
    let plan = prepare_migration(&mut tx, skills).await?;
    apply_migration(&mut tx, plan, at, || Ok(())).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::public_url;

    #[test]
    fn a_source_address_hides_userinfo_queries_and_fragments() {
        for raw in [
            "https://alice:password@example.invalid/skill.tar.gz",
            "https://example.invalid/skill.tar.gz?token=test-secret",
            "https://example.invalid/skill.tar.gz?X-Amz-Signature=test-secret",
            "https://example.invalid/skill.tar.gz#test-secret",
        ] {
            let (shown, private) = public_url(raw);
            assert!(private);
            assert!(shown.ends_with("?…"));
            assert!(!shown.contains("test-secret"));
            assert!(!shown.contains("password"));
            assert!(!shown.contains("alice"));
        }
        assert_eq!(
            public_url("https://example.invalid/skill.tar.gz"),
            ("https://example.invalid/skill.tar.gz".to_owned(), false)
        );
    }
}
