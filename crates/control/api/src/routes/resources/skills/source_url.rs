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

/// Moves older plain URL origins into skill-owned sealed rows when their skill is next read or
/// written. A current origin has a pointer for reimport; historical versions retain their own
/// sealed source without making it a public version member.
pub(super) async fn migrate_many(
    conn: &mut Conn,
    skills: &[Id],
    at: DateTime<Utc>,
) -> ApiResult<()> {
    if skills.is_empty() {
        return Ok(());
    }
    let current: Vec<(Id, Option<Json>, Option<Id>)> =
        sqlx::query_as("SELECT id,origin,source_url_secret_id FROM skills WHERE id=ANY($1)")
            .bind(skills)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    for (id, old, pointer) in current {
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
        let secret = if pointer.is_none() {
            Some(
                sealing::put_skill_source_secret(
                    conn,
                    id,
                    "source_url:legacy_current",
                    url,
                    None,
                    at,
                )
                .await?
                .id,
            )
        } else {
            pointer
        };
        let mut public = origin;
        if let SkillOrigin::Url { url, .. } = &mut public {
            *url = shown;
        }
        sqlx::query(
            "UPDATE skills SET origin=$2,source_url_secret_id=COALESCE(source_url_secret_id,$3)
              WHERE id=$1 AND origin=$4",
        )
        .bind(id)
        .bind(serde_json::to_value(public).map_err(ApiError::internal)?)
        .bind(secret)
        .bind(old)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    }
    let versions: Vec<(Id, Id, Json)> =
        sqlx::query_as("SELECT id,skill_id,origin FROM skill_versions WHERE skill_id=ANY($1)")
            .bind(skills)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
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
        sealing::put_skill_source_secret(conn, skill, &name, url, None, at).await?;
        let mut public = origin;
        if let SkillOrigin::Url { url, .. } = &mut public {
            *url = shown;
        }
        sqlx::query("UPDATE skill_versions SET origin=$2 WHERE id=$1 AND origin=$3")
            .bind(id)
            .bind(serde_json::to_value(public).map_err(ApiError::internal)?)
            .bind(old)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    // An older audit may have copied the original origin verbatim. Keep the event and its other
    // facts, while dropping that untrusted address before any history view can return it again.
    sqlx::query(
        "UPDATE audit_log SET details=details-'origin'
          WHERE object_kind='skill' AND object_id=ANY($1) AND details ? 'origin'",
    )
    .bind(skills)
    .execute(&mut *conn)
    .await
    .map_err(DbError::from)?;
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
