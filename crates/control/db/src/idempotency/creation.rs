//! Account-scoped creation identities recorded by the native writer in its real transaction.
use super::*;
use orch_types::Hash;
use serde::{Deserialize, Serialize};

/// The same lock-only backend is shared by all keys of one API application.
pub async fn try_lock(conn: &mut Conn, account: Id, key: &str) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(concat!(
        "SELECT pg_try_advisory_lock(",
        creation_lock_key!("$1", "$2"),
        ")"
    ))
    .bind(account)
    .bind(key)
    .fetch_one(conn)
    .await?)
}
/// Release only this key on the backend that acquired it.
pub async fn unlock(conn: &mut Conn, account: Id, key: &str) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(concat!(
        "SELECT pg_advisory_unlock(",
        creation_lock_key!("$1", "$2"),
        ")"
    ))
    .bind(account)
    .bind(key)
    .fetch_one(conn)
    .await?)
}

/// Identity and factual native kind, never response bytes or authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Created {
    pub kind: String,
    pub id: Id,
    #[serde(default)]
    pub data: Json,
}
/// A versioned outcome in the existing idempotency row; older native uses keep their own records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub version: u8,
    pub body_hash: Hash,
    pub owner: Id,
    #[serde(default)]
    pub failure: Option<String>,
    pub native_kind: String,
    pub outcome: Option<Created>,
}
impl Record {
    pub fn pending(body_hash: Hash, native_kind: &str, owner: Id) -> Self {
        Self {
            version: 1,
            body_hash,
            owner,
            failure: None,
            native_kind: native_kind.into(),
            outcome: None,
        }
    }
    pub fn read(value: Json) -> Result<Self, DbError> {
        let value: Self = serde_json::from_value(value).map_err(|_| invalid())?;
        if value.version != 1
            || value
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.kind != value.native_kind)
        {
            return Err(invalid());
        }
        Ok(value)
    }
}
/// Request-owned metadata crosses the native writer call, not an HTTP credential or reusable permit.
#[derive(Debug, Clone)]
pub struct Capture {
    pub account: Id,
    pub key: String,
    pub record: Record,
}
tokio::task_local! {static CURRENT:Capture;}

/// A public native account creator has no principal before its account transaction.
/// The actual account, never a guest or caller-supplied identifier, owns this record.
#[derive(Debug, Clone)]
pub struct Unassigned {
    pub key: String,
    pub method: String,
    pub path: String,
    pub record: Record,
    pub at: DateTime<Utc>,
}
tokio::task_local! {static UNASSIGNED:Unassigned;}

pub async fn scope_unassigned<T>(
    capture: Option<Unassigned>,
    work: impl std::future::Future<Output = T>,
) -> T {
    match capture {
        Some(capture) => UNASSIGNED.scope(capture, work).await,
        None => work.await,
    }
}

/// Account creators call this after creating their real account, in that same transaction.
/// Their native uniqueness/setup lock decides concurrent first attempts. A rolled back
/// attempt keeps neither an account nor replay metadata.
pub async fn observe_account(
    conn: &mut Conn,
    account: Id,
    kind: &str,
    status: u16,
) -> Result<(), DbError> {
    if CURRENT.try_with(|_| ()).is_ok() {
        return observe(conn, Some(account), kind, account).await;
    }
    let Some(capture) = UNASSIGNED.try_with(Clone::clone).ok() else {
        return Ok(());
    };
    if capture.record.native_kind != kind {
        return Ok(());
    }
    if !crate::queries::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    let mut record = capture.record;
    record.outcome = Some(Created {
        kind: kind.into(),
        id: account,
        data: Json::Null,
    });
    super::keep(
        conn,
        account,
        &capture.key,
        &Replay {
            method: capture.method,
            path: capture.path,
            status: i32::from(status),
            response: serde_json::to_value(record).map_err(|_| invalid())?,
        },
        capture.at,
    )
    .await
}

/// Native producers can avoid selecting extra receipt identities outside a tracked request.
pub fn captures(account: Option<Id>, kind: &str) -> bool {
    CURRENT
        .try_with(|capture| account == Some(capture.account) && kind == capture.record.native_kind)
        .unwrap_or(false)
}

pub async fn scope<T>(capture: Option<Capture>, work: impl std::future::Future<Output = T>) -> T {
    match capture {
        Some(capture) => CURRENT.scope(capture, work).await,
        None => work.await,
    }
}
fn invalid() -> DbError {
    DbError::Other(sqlx::Error::Protocol(
        "the native creation identity does not match its idempotency record".into(),
    ))
}

/// Wait for an in-flight creation's row update before deciding whether its outcome committed.
pub async fn locked(
    conn: &mut Conn,
    account: Id,
    key: &str,
    since: DateTime<Utc>,
) -> Result<Option<Replay>, DbError> {
    Ok(sqlx::query_as("SELECT method,path,status,response FROM idempotency_keys WHERE user_id=$1 AND key=$2 AND created_at>$3 FOR UPDATE")
        .bind(account).bind(key).bind(since).fetch_optional(conn).await?)
}
/// Called only at an actual native creation, on the transaction that makes that resource.
/// A process lost after commit still has this identity; rollback removes both together.
pub async fn observe(
    conn: &mut Conn,
    account: Option<Id>,
    kind: &str,
    id: Id,
) -> Result<(), DbError> {
    observe_with(conn, account, kind, id, Json::Null).await
}

/// Native producers can retain minimal accepted identities/counters beside their primary identity.
pub async fn observe_with(
    conn: &mut Conn,
    account: Option<Id>,
    kind: &str,
    id: Id,
    data: Json,
) -> Result<(), DbError> {
    observe_outcome(conn, account, kind, id, data, None, None).await
}

/// Record a native acceptance whose successful status differs from the route default.
/// The real writer supplies it in the same transaction as that actual accepted identity.
pub async fn observe_with_status(
    conn: &mut Conn,
    account: Option<Id>,
    kind: &str,
    id: Id,
    data: Json,
    status: u16,
) -> Result<(), DbError> {
    observe_outcome(conn, account, kind, id, data, Some(status), None).await
}
/// A native writer discarded its own reservation and accepted an actual existing
/// resource instead. Only that exact still-owned reservation can change the root.
pub async fn observe_replacement(
    conn: &mut Conn,
    account: Option<Id>,
    kind: &str,
    reserved: Id,
    actual: Id,
    data: Json,
    status: u16,
) -> Result<(), DbError> {
    observe_outcome(
        conn,
        account,
        kind,
        actual,
        data,
        Some(status),
        Some(reserved),
    )
    .await
}
async fn observe_outcome(
    conn: &mut Conn,
    account: Option<Id>,
    kind: &str,
    id: Id,
    data: Json,
    status: Option<u16>,
    replacement: Option<Id>,
) -> Result<(), DbError> {
    let Some(capture) = CURRENT.try_with(Clone::clone).ok() else {
        return Ok(());
    };
    if account != Some(capture.account) || kind != capture.record.native_kind {
        return Ok(());
    }
    if !crate::queries::transaction_guard::in_transaction(conn) {
        return Err(invalid());
    }
    let value: Option<Json> = sqlx::query_scalar(
        "SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2 FOR UPDATE",
    )
    .bind(capture.account)
    .bind(&capture.key)
    .fetch_optional(&mut *conn)
    .await?;
    let mut record = Record::read(value.ok_or_else(invalid)?)?;
    if record.owner != capture.record.owner {
        return Err(DbError::Rule {
            constraint: "idempotency_owner_changed".into(),
        });
    }
    if record.body_hash != capture.record.body_hash
        || record.native_kind != capture.record.native_kind
    {
        return Err(invalid());
    }
    if let Some(reserved) = replacement
        && record.outcome.as_ref().is_none_or(|old| old.id != reserved)
    {
        return Err(invalid());
    }
    let outcome = Created {
        kind: kind.into(),
        id,
        data,
    };
    if replacement.is_some() {
        record.outcome = Some(outcome);
    } else if let Some(old) = &mut record.outcome {
        // A native tree creator inserts its root before its same-kind descendants.
        if old.id != id {
            return Ok(());
        }
        if !outcome.data.is_null() {
            let mut data = outcome.data;
            // A later native filing may enrich the same identity after command acceptance.
            // Keep its actual accepted command reference; it cannot be replaced by another one.
            if let Some(command) = old.data.get("native_command") {
                let fields = data.as_object_mut().ok_or_else(invalid)?;
                if fields
                    .get("native_command")
                    .is_some_and(|next| next != command)
                {
                    return Err(invalid());
                }
                fields.insert("native_command".into(), command.clone());
            }
            old.data = data;
        }
    } else {
        record.outcome = Some(outcome);
    }
    let value = serde_json::to_value(record).map_err(|_| invalid())?;
    let changed =
        sqlx::query("UPDATE idempotency_keys SET response=$3,status=COALESCE($4,status) WHERE user_id=$1 AND key=$2")
            .bind(capture.account)
            .bind(&capture.key)
            .bind(value)
            .bind(status.map(i32::from))
            .execute(&mut *conn)
            .await?
            .rows_affected();
    if changed != 1 {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};
    #[tokio::test]
    async fn native_status_and_reservation_replacement_preserve_the_exact_owner_and_root()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let db = TestDb::fresh().await;
        let account = orch_types::new_id();
        let actual = orch_types::new_id();
        let key = orch_types::new_id().to_string();
        let record = Record::pending(
            Hash::of(b"native receipt control"),
            "account",
            orch_types::new_id(),
        );
        let mut conn = db.pool.acquire().await?;
        sqlx::query("INSERT INTO users(id,username,display_name,avatar_color) VALUES($1,'native-first','First',0),($2,'native-existing','Existing',0)")
            .bind(account).bind(actual).execute(&mut *conn).await?;
        super::super::keep(
            &mut conn,
            account,
            &key,
            &Replay {
                method: "POST".into(),
                path: "/native-creation".into(),
                status: 201,
                response: serde_json::to_value(&record)?,
            },
            clock::start(),
        )
        .await?;
        drop(conn);
        let capture = Capture {
            account,
            key: key.clone(),
            record: record.clone(),
        };
        scope(Some(capture.clone()), async {
            let mut tx = db.pool.begin().await?;
            observe(&mut tx, Some(account), "account", account).await?;
            // Same-kind descendants cannot replace their accepted native root.
            observe_with_status(&mut tx, Some(account), "account", actual, Json::Null, 200).await?;
            let saved = super::super::find(
                &mut tx,
                account,
                &key,
                clock::start() - chrono::Duration::hours(1),
            )
            .await?
            .ok_or("record missing")?;
            assert_eq!(saved.status, 201);
            assert_eq!(
                Record::read(saved.response)?
                    .outcome
                    .ok_or("root missing")?
                    .id,
                account
            );
            assert!(
                observe_replacement(
                    &mut tx,
                    Some(account),
                    "account",
                    actual,
                    account,
                    Json::Null,
                    200
                )
                .await
                .is_err()
            );
            observe_replacement(
                &mut tx,
                Some(account),
                "account",
                account,
                actual,
                Json::Null,
                200,
            )
            .await?;
            tx.commit().await?;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await?;
        let mut changed = capture;
        changed.record.owner = orch_types::new_id();
        scope(Some(changed),async {
            let mut tx=db.pool.begin().await?;
            assert!(matches!(observe_replacement(&mut tx,Some(account),"account",actual,account,Json::Null,201).await,
                Err(DbError::Rule{constraint}) if constraint=="idempotency_owner_changed"));
            tx.rollback().await?;Ok::<(),Box<dyn std::error::Error+Send+Sync>>(())
        }).await?;
        let mut conn = db.pool.acquire().await?;
        let saved = super::super::find(
            &mut conn,
            account,
            &key,
            clock::start() - chrono::Duration::hours(1),
        )
        .await?
        .ok_or("record missing")?;
        assert_eq!(saved.status, 200);
        assert_eq!(
            Record::read(saved.response)?
                .outcome
                .ok_or("actual identity missing")?
                .id,
            actual
        );
        Ok(())
    }
}
