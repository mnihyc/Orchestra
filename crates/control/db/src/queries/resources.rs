//! Machines, hosts with their owners, load and storages, providers with their endpoints, profiles, harnesses, and models with their two names and what each has enabled.
//!
//! This first part is what a machine's link stands on: the machines
//! themselves, the tokens that make them, and the hosts that are machines of
//! one kind. A machine is known by the fingerprint of its certificate, and
//! revoking it is a state of its row.
//!
//! The sealing key and the pending certificate are protected columns written
//! by enrollment and renewal, independently of machine-reported facts.
//! Older rows can still read the stored [`LINK_MEMBER`] as a compatibility
//! bridge; a successful authority write adopts it and removes it from facts.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// The stored certificate member read only for older rows without protected columns.
pub const LINK_MEMBER: &str = "$link";

/// A replacement certificate that the machine has not yet been seen to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextCertificate {
    /// Its SHA-256, in lowercase hexadecimal.
    pub fingerprint: String,
    /// Its public key, as unpadded base64url of the uncompressed point.
    pub public_key: String,
    /// Its serial number.
    pub serial: String,
    /// The last moment it is good.
    pub not_after: DateTime<Utc>,
    /// Whether this came from re-enrollment and must not be replaced by the old identity renewing.
    #[serde(default)]
    pub again: bool,
}

/// What the link keeps about a machine's certificates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkCertificates {
    /// The public key of the certificate the machine is known by, as
    /// unpadded base64url of the uncompressed point. Empty for a machine
    /// whose row was made without one.
    #[serde(default)]
    pub public_key: String,
    /// The certificate a renewal or re-enrollment issued, until a session shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<NextCertificate>,
}

/// A machine, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct MachineRow {
    /// Its identifier.
    pub id: Id,
    /// What it is called.
    pub name: String,
    /// The person it belongs to; none for a machine the system made.
    pub owner_id: Option<Id>,
    /// `connected` or `managed`.
    pub placement: String,
    /// The host that runs it, for a runtime's connector.
    pub host_id: Option<Id>,
    /// The runtime it is, for a runtime's connector.
    pub runtime_id: Option<Id>,
    /// The roles it has switched on.
    pub roles: Vec<String>,
    /// Its operating system.
    pub platform_os: String,
    /// Its processor architecture.
    pub platform_arch: String,
    /// The connector's version, as of its last session.
    pub connector_version: String,
    /// The protocol version it speaks.
    pub protocol_version: i32,
    /// The serial number of its certificate.
    pub cert_serial: String,
    /// The SHA-256 of its certificate.
    pub cert_fingerprint: Vec<u8>,
    /// The last moment its certificate is good.
    pub cert_not_after: DateTime<Utc>,
    /// The current sealing key, written by enrollment rather than a facts report.
    pub cert_public_key: Option<String>,
    /// The pending certificate's lowercase hexadecimal SHA-256.
    pub cert_next_fingerprint: Option<String>,
    /// The pending certificate's sealing key.
    pub cert_next_public_key: Option<String>,
    /// Its serial number.
    pub cert_next_serial: Option<String>,
    /// The end of its validity.
    pub cert_next_not_after: Option<DateTime<Utc>>,
    /// Whether replacement enrollment issued that pending identity.
    pub cert_next_again: Option<bool>,
    /// `active` or `revoked`.
    pub status: String,
    /// When its identity was permanently declared lost.
    pub lost_at: Option<DateTime<Utc>>,
    /// Whether it had a session when it was last known.
    pub online: bool,
    /// When it last had a session, or still has one.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Stable attachment marker, unchanged by heartbeat observations.
    pub session_started_at: Option<DateTime<Utc>>,
    /// The address of its last session.
    pub last_ip: Option<String>,
    /// How fast it sends, as an average of what it measured.
    pub rate_up_bps: Option<i64>,
    /// How fast it receives, as an average of what it measured.
    pub rate_down_bps: Option<i64>,
    /// What it reports about itself; an older row can still carry [`LINK_MEMBER`].
    pub facts: Json,
    /// When it was enrolled.
    pub created_at: DateTime<Utc>,
    /// When its row last changed.
    pub updated_at: DateTime<Utc>,
    /// When it was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl MachineRow {
    /// The current session marker, with the old observation for an unstamped legacy row.
    #[must_use]
    pub fn session_marker(&self) -> Option<DateTime<Utc>> {
        self.session_started_at.or(self.last_seen_at)
    }

    /// Whether the machine may hold a session: it was not revoked or
    /// deleted.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == "active" && self.deleted_at.is_none()
    }

    /// What the link keeps about its certificates.
    #[must_use]
    pub fn link(&self) -> LinkCertificates {
        let legacy: LinkCertificates = self
            .facts
            .get(LINK_MEMBER)
            .and_then(|member| serde_json::from_value(member.clone()).ok())
            .unwrap_or_default();
        let next = match (
            &self.cert_next_fingerprint,
            &self.cert_next_public_key,
            &self.cert_next_serial,
            self.cert_next_not_after,
            self.cert_next_again,
        ) {
            (Some(fingerprint), Some(public_key), Some(serial), Some(not_after), Some(again)) => {
                Some(NextCertificate {
                    fingerprint: fingerprint.clone(),
                    public_key: public_key.clone(),
                    serial: serial.clone(),
                    not_after,
                    again,
                })
            }
            _ => legacy.next,
        };
        LinkCertificates {
            public_key: self.cert_public_key.clone().unwrap_or(legacy.public_key),
            next,
        }
    }
}

/// The columns of a machine, in the order [`MachineRow`] has them.
macro_rules! machine_columns {
    () => {
        "id, name, owner_id, placement, host_id, runtime_id, roles, platform_os, platform_arch,
         connector_version, protocol_version, cert_serial, cert_fingerprint, cert_not_after,
         cert_public_key, cert_next_fingerprint, cert_next_public_key, cert_next_serial,
         cert_next_not_after, cert_next_again, status, lost_at,
         online, last_seen_at, session_started_at, host(last_ip) AS last_ip, rate_up_bps, rate_down_bps, facts,
         created_at, updated_at, deleted_at"
    };
}

/// A machine that was just enrolled.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMachine<'a> {
    /// Its identifier.
    pub id: Id,
    /// What it is called.
    pub name: &'a str,
    /// The person it belongs to.
    pub owner_id: Option<Id>,
    /// `connected` or `managed`.
    pub placement: &'a str,
    /// The roles it has switched on.
    pub roles: &'a [String],
    /// Its operating system.
    pub platform_os: &'a str,
    /// Its processor architecture.
    pub platform_arch: &'a str,
    /// The connector's version.
    pub connector_version: &'a str,
    /// The protocol version it speaks.
    pub protocol_version: i32,
    /// The serial number of its certificate.
    pub cert_serial: &'a str,
    /// The SHA-256 of its certificate.
    pub cert_fingerprint: &'a [u8],
    /// The last moment its certificate is good.
    pub cert_not_after: DateTime<Utc>,
    /// The public key of its certificate.
    pub public_key: &'a str,
}

/// Stores a machine that was just enrolled.
pub async fn insert_machine(
    conn: &mut Conn,
    machine: &NewMachine<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO machines (id, name, owner_id, placement, roles, platform_os, platform_arch,
                               connector_version, protocol_version, cert_serial, cert_fingerprint,
                               cert_not_after, cert_public_key, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14)",
    )
    .bind(machine.id)
    .bind(machine.name)
    .bind(machine.owner_id)
    .bind(machine.placement)
    .bind(machine.roles)
    .bind(machine.platform_os)
    .bind(machine.platform_arch)
    .bind(machine.connector_version)
    .bind(machine.protocol_version)
    .bind(machine.cert_serial)
    .bind(machine.cert_fingerprint)
    .bind(machine.cert_not_after)
    .bind(machine.public_key)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The machine `id`, deleted or not.
pub async fn machine(conn: &mut Conn, id: Id) -> Result<Option<MachineRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        machine_columns!(),
        " FROM machines WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Notes what a machine said about itself when a session began.
pub async fn record_session(
    conn: &mut Conn,
    id: Id,
    connector_version: &str,
    roles: &[String],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE machines SET connector_version = $2, roles = $3, updated_at = $4
          WHERE id = $1 AND (connector_version IS DISTINCT FROM $2 OR roles IS DISTINCT FROM $3)",
    )
    .bind(id)
    .bind(connector_version)
    .bind(roles)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Notes that a machine has a session, or no longer has one. The address is
/// kept when it is known.
pub async fn set_presence(
    conn: &mut Conn,
    id: Id,
    online: bool,
    ip: Option<IpAddr>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    set_session_presence(conn, id, online, ip, online.then(orch_types::new_id), at).await
}

/// Records the gateway's session identity once; duplicate reports and heartbeats keep its marker.
/// A monotonic microsecond distinguishes reconnects even when the domain clock is held.
pub async fn set_session_presence(
    conn: &mut Conn,
    id: Id,
    online: bool,
    ip: Option<IpAddr>,
    session: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE machines SET online = $2, last_seen_at = $4, last_ip = COALESCE($3::inet, last_ip), updated_at = $4,
            offline_since=CASE WHEN $2 THEN NULL WHEN online THEN $4 ELSE COALESCE(offline_since,$4) END,
            session_started_at=CASE WHEN $2 AND (session_started_at IS NULL OR current_session_id IS DISTINCT FROM $5)
                THEN GREATEST($4,COALESCE(session_started_at + interval '1 microsecond',$4)) ELSE session_started_at END,
            current_session_id=CASE WHEN $2 THEN $5 ELSE current_session_id END
          WHERE id = $1",
    )
    .bind(id)
    .bind(online)
    .bind(ip.map(|ip| ip.to_string()))
    .bind(at)
    .bind(session)
    .execute(conn)
    .await?;
    Ok(())
}

/// Moves the last time these machines were seen to `at`, for those that have a
/// session and were last seen at least `every_secs` before: a machine that holds
/// its session is seen all the time, and its row is written at most once in that
/// time. Says how many rows moved.
pub async fn touch_seen(
    conn: &mut Conn,
    ids: &[Id],
    at: DateTime<Utc>,
    every_secs: i64,
) -> Result<u64, DbError> {
    if ids.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "UPDATE machines SET last_seen_at = $2
          WHERE id = ANY($1) AND online
            AND (last_seen_at IS NULL OR last_seen_at <= $2 - $3::bigint * interval '1 second')",
    )
    .bind(ids)
    .bind(at)
    .bind(every_secs)
    .execute(conn)
    .await?
    .rows_affected())
}

/// Forgets which machines had a session: at a start, none is known to, and
/// each gateway says which it holds when it connects.
pub async fn reset_presence(conn: &mut Conn) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "UPDATE machines SET online = false, offline_since=clock_timestamp() WHERE online",
    )
    .execute(conn)
    .await?
    .rows_affected())
}

/// Replaces machine-reported facts independently of protected certificate state,
/// adopting an older stored certificate member, and folds measured rates into the averages for
/// estimates: a rate of 0 is no measurement, and the first measurement is the
/// average. `weight` is the share of the new rate in the average.
pub async fn set_machine_facts(
    conn: &mut Conn,
    id: Id,
    facts: &Json,
    rates: (i64, i64),
    weight: f64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    // A report may change who may drive the environment's threads. Its writer has
    // taken the authority gate; retain the report and its cache invalidation together.
    // The row is locked as its update locks it: every command and session row that names the
    // machine holds a key-share lock until its transaction ends, and FOR UPDATE waited behind
    // all of them, then held every new one back (deliveries of ten threads on one machine timed out).
    let mut tx = sqlx::Connection::begin(conn).await?;
    let previous: Option<Json> =
        sqlx::query_scalar("SELECT facts FROM machines WHERE id=$1 FOR NO KEY UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let changed = previous.as_ref().is_some_and(|old| {
        crate::queries::environment_facts::environment_delegated(
            crate::queries::environment_facts::policy_in(old),
        ) != crate::queries::environment_facts::environment_delegated(
            crate::queries::environment_facts::policy_in(facts),
        )
    });
    let (up, down) = rates;
    sqlx::query(
        "UPDATE machines SET
            cert_public_key = COALESCE(cert_public_key, facts #>> '{$link,public_key}'),
            cert_next_fingerprint = COALESCE(cert_next_fingerprint, facts #>> '{$link,next,fingerprint}'),
            cert_next_public_key = COALESCE(cert_next_public_key, facts #>> '{$link,next,public_key}'),
            cert_next_serial = COALESCE(cert_next_serial, facts #>> '{$link,next,serial}'),
            cert_next_not_after = COALESCE(cert_next_not_after, (facts #>> '{$link,next,not_after}')::timestamptz),
            cert_next_again = COALESCE(cert_next_again, CASE WHEN facts #>> '{$link,next,fingerprint}' IS NOT NULL
                THEN COALESCE((facts #>> '{$link,next,again}')::boolean, false) END),
            facts = CASE WHEN jsonb_typeof($2::jsonb) = 'object'
                THEN $2::jsonb - '$link' ELSE $2::jsonb END,
            rate_up_bps = CASE WHEN $3 <= 0 THEN rate_up_bps
                               WHEN rate_up_bps IS NULL THEN $3
                               ELSE round(rate_up_bps * (1 - $5) + $3 * $5)::bigint END,
            rate_down_bps = CASE WHEN $4 <= 0 THEN rate_down_bps
                                 WHEN rate_down_bps IS NULL THEN $4
                                 ELSE round(rate_down_bps * (1 - $5) + $4 * $5)::bigint END,
            updated_at = $6
          WHERE id = $1",
    )
    .bind(id)
    .bind(facts)
    .bind(up)
    .bind(down)
    .bind(weight)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    if changed {
        crate::queries::access::bump_epoch(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Records the certificate a renewal issued. Until a session shows it, the
/// machine is known by both.
pub async fn set_next_certificate(
    conn: &mut Conn,
    id: Id,
    next: &NextCertificate,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE machines SET
            cert_public_key = COALESCE(cert_public_key, facts #>> '{$link,public_key}'),
            cert_next_fingerprint = $2, cert_next_public_key = $3, cert_next_serial = $4,
            cert_next_not_after = $5, cert_next_again = $6,
            facts = facts - '$link', updated_at = $7
          WHERE id = $1",
    )
    .bind(id)
    .bind(&next.fingerprint)
    .bind(&next.public_key)
    .bind(&next.serial)
    .bind(next.not_after)
    .bind(next.again)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Makes the renewed certificate the machine's own, because a session showed
/// it. Returns whether the machine had been issued one with that
/// fingerprint (in hexadecimal).
pub async fn promote_next_certificate(
    conn: &mut Conn,
    id: Id,
    fingerprint_hex: &str,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE machines SET
            cert_fingerprint = decode(COALESCE(cert_next_fingerprint, facts #>> '{$link,next,fingerprint}'), 'hex'),
            cert_serial = COALESCE(cert_next_serial, facts #>> '{$link,next,serial}'),
            cert_not_after = COALESCE(cert_next_not_after, (facts #>> '{$link,next,not_after}')::timestamptz),
            cert_public_key = COALESCE(cert_next_public_key, facts #>> '{$link,next,public_key}'),
            cert_next_fingerprint = NULL, cert_next_public_key = NULL, cert_next_serial = NULL,
            cert_next_not_after = NULL, cert_next_again = NULL,
            facts = facts - '$link',
            updated_at = $3
          WHERE id = $1 AND COALESCE(cert_next_fingerprint, facts #>> '{$link,next,fingerprint}') = $2",
    )
    .bind(id)
    .bind(fingerprint_hex)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Marks a machine revoked and takes it offline; returns whether it was active.
pub async fn revoke_machine(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE machines SET status = 'revoked', online = false, offline_since=COALESCE(offline_since,$2), updated_at = $2
          WHERE id = $1 AND status = 'active'",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// An enrollment token, as stored: the hash of its secret and what it was
/// made for.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TokenRow {
    /// The SHA-256 of the secret part of the token.
    pub token_hash: Vec<u8>,
    /// Who made it; the owner of a newly enrolled machine.
    pub created_by: Id,
    /// `connected` for a person's machine, `host` for the node of a host.
    pub purpose: String,
    /// For a host, the org that will be its home; none is the system's.
    pub org_id: Option<Id>,
    /// What the machine is to be called, if the maker said.
    pub name_hint: Option<String>,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// When it was used.
    pub used_at: Option<DateTime<Utc>>,
    /// The machine that consumed it.
    pub machine_id: Option<Id>,
    /// The existing machine this token enrolls again, rather than making another.
    pub again_machine_id: Option<Id>,
}

/// Stores a token. Returns whether it was stored: one whose hash is there
/// already is left as it is.
pub async fn insert_enrollment_token(conn: &mut Conn, token: &TokenRow) -> Result<bool, DbError> {
    let inserted = sqlx::query(
        "INSERT INTO enrollment_tokens (token_hash, created_by, purpose, org_id, name_hint, expires_at, again_machine_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (token_hash) DO NOTHING",
    )
    .bind(&token.token_hash)
    .bind(token.created_by)
    .bind(&token.purpose)
    .bind(token.org_id)
    .bind(&token.name_hint)
    .bind(token.expires_at)
    .bind(token.again_machine_id)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(inserted > 0)
}

/// Binds a newly issued token's role grant in the existing enrollment ledger before it is disclosed.
pub async fn set_enrollment_roles(
    conn: &mut Conn,
    token_hash: &[u8],
    roles: &[String],
) -> Result<(), DbError> {
    sqlx::query("UPDATE enrollment_tokens SET roles=$2 WHERE token_hash=$1 AND used_at IS NULL")
        .bind(token_hash)
        .bind(roles)
        .execute(conn)
        .await?;
    Ok(())
}

/// The token whose secret hashes to `token_hash`, locked until the
/// transaction ends, so that two requests with one token are served one
/// after the other.
pub async fn lock_enrollment_token(
    conn: &mut Conn,
    token_hash: &[u8],
) -> Result<Option<TokenRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT token_hash, created_by, purpose, org_id, name_hint, expires_at, used_at, machine_id, again_machine_id
           FROM enrollment_tokens WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(token_hash)
    .fetch_optional(conn)
    .await?)
}

/// Enrollment identity for its issuer alone; no secret or credential material leaves this query.
pub async fn enrollment_progress(
    conn: &mut Conn,
    issuer: Id,
    receipt: &[u8],
) -> Result<Option<(String, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT purpose,machine_id FROM enrollment_tokens WHERE created_by=$1 AND token_hash=$2",
    )
    .bind(issuer)
    .bind(receipt)
    .fetch_optional(conn)
    .await?)
}

/// Spends a token on the machine it made. Returns whether it was spent now.
pub async fn spend_enrollment_token(
    conn: &mut Conn,
    token_hash: &[u8],
    machine: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let spent = sqlx::query(
        "UPDATE enrollment_tokens SET used_at = $3, machine_id = $2
          WHERE token_hash = $1 AND used_at IS NULL",
    )
    .bind(token_hash)
    .bind(machine)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(spent > 0)
}

/// Binds the host's creation choice to a newly issued, unspent token inside its creator's transaction.
pub async fn set_host_enrollment_access(
    conn: &mut Conn,
    token_hash: &[u8],
    access: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE enrollment_tokens SET access=$2 WHERE token_hash=$1 AND purpose='host' AND used_at IS NULL AND access IS NULL")
        .bind(token_hash).bind(access).execute(conn).await?.rows_affected() == 1)
}

/// Reads the host's creation choice after its token row has been locked for enrollment.
pub async fn host_enrollment_access(
    conn: &mut Conn,
    token_hash: &[u8],
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT access FROM enrollment_tokens WHERE token_hash=$1 AND purpose='host'",
    )
    .bind(token_hash)
    .fetch_one(conn)
    .await?)
}

/// Moves the expiry of a token that was not used, and returns whether there
/// was one: a token the system holds a file of is kept good for as long as
/// that file can be used.
pub async fn extend_unused_token(
    conn: &mut Conn,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query(
        "UPDATE enrollment_tokens SET expires_at = GREATEST(expires_at, $2)
          WHERE token_hash = $1 AND used_at IS NULL",
    )
    .bind(token_hash)
    .bind(expires_at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// The system administrator who has been one longest: who a token the
/// system made for itself, at its first start, is made in the name of.
pub async fn first_system_admin(conn: &mut Conn) -> Result<Option<Id>, DbError> {
    let found: Option<(Id,)> = sqlx::query_as(
        "SELECT id FROM users WHERE is_system_admin AND status = 'active' ORDER BY created_at, id LIMIT 1",
    )
    .fetch_optional(conn)
    .await?;
    Ok(found.map(|(id,)| id))
}

/// A host that was just enrolled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHost<'a> {
    /// Its identifier.
    pub id: Id,
    /// The machine that runs its node.
    pub machine_id: Id,
    /// Who enrolled it.
    pub owner_id: Option<Id>,
    /// Its home: an org at any level, or none for the system's.
    pub home_id: Option<Id>,
    /// What it is called.
    pub name: &'a str,
}

/// Stores a host. The root org of its home is read from the hierarchy.
pub async fn insert_host(
    conn: &mut Conn,
    host: &NewHost<'_>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO hosts (id, machine_id, owner_id, org_id, home_id, name, created_at, updated_at)
         VALUES ($1, $2, $3, (SELECT org_id FROM nodes WHERE id = $4), $4, $5, $6, $6)",
    )
    .bind(host.id)
    .bind(host.machine_id)
    .bind(host.owner_id)
    .bind(host.home_id)
    .bind(host.name)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// What the thread engine reads of providers, agent profiles, harness
/// processes and models, and the harness rows it writes. It is a module of
/// its own so that it stands beside whatever else is written about these
/// resources without sharing a name with it.
pub mod thread_engine {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// A provider, as far as a turn needs it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct ProviderRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// The org it belongs to.
        pub home_id: Id,
        /// `subscription` or `api`.
        pub kind: String,
        /// For a subscription: the harness kind whose login it is.
        pub harness_kind: Option<String>,
        /// For a subscription: the profile whose harness holds the login.
        pub agent_id: Option<Id>,
        /// For an API provider: the protocols it serves.
        pub protocols: Vec<String>,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where its runtime is.
        pub machine_id: Option<Id>,
        /// The machine that keeps the key, for an API provider.
        pub key_machine_id: Option<Id>,
        /// The arrangement types its owner allows.
        pub arrangements: Vec<String>,
        /// The environment its credential is handed over to.
        pub held_by_environment_id: Option<Id>,
        /// `needs_setup`, `ready`, `exhausted`, `error`, `offline` or `disabled`.
        pub status: String,
        /// A sentence about the status.
        pub status_detail: Option<String>,
        /// The selected facts/models sources, learned descriptors and safe per-element errors.
        #[sqlx(default)]
        pub sources: Json,
        /// What the harness or the endpoint last reported.
        pub facts: Json,
        /// The binding and credential revision captured for a correlated observation.
        pub revision: i64,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The provider `id`.
    pub async fn provider(conn: &mut Conn, id: Id) -> Result<Option<ProviderRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id, protocols, provider_placement(machine_id) AS placement, machine_id,
                    key_machine_id, arrangements, held_by_environment_id, status, status_detail, sources, facts, revision, deleted_at
               FROM providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// A model of a provider.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct ModelRow {
        /// The name that is sent.
        pub model_name: String,
        /// The name that is shown; empty when the routing name is.
        pub display_name: String,
        /// What it can be run with here: its efforts, tiers, programs, context and rate.
        pub parameters: Json,
        /// The groups its managers put it in.
        pub groups: Vec<String>,
        /// Whether the provider offers it.
        pub enabled: bool,
    }

    /// The model of a provider that goes by `model_name`.
    pub async fn model(
        conn: &mut Conn,
        provider: Id,
        model_name: &str,
    ) -> Result<Option<ModelRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT model_name, display_name, parameters, groups, enabled FROM provider_models
              WHERE provider_id = $1 AND model_name = $2",
        )
        .bind(provider)
        .bind(model_name)
        .fetch_optional(conn)
        .await?)
    }

    /// An agent profile, as far as a turn needs it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct AgentRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner; none for a profile of the system.
        pub owner_id: Option<Id>,
        /// The org it belongs to; none for a profile of the system.
        pub home_id: Option<Id>,
        /// The harness kind it runs.
        pub harness_kind: String,
        /// The machine it belongs to, for a harness installed on one.
        pub machine_id: Option<Id>,
        /// Which build to run, and how it is configured.
        pub profile: Json,
        /// What a harness started from it last declared.
        pub descriptor: Json,
        /// What the system sets over that.
        pub descriptor_overrides: Json,
        /// Whether it is switched off.
        pub disabled: bool,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The agent profile `id`.
    pub async fn agent(conn: &mut Conn, id: Id) -> Result<Option<AgentRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, owner_id, home_id, harness_kind, machine_id, profile, descriptor, descriptor_overrides,
                    disabled, deleted_at
               FROM agents WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// Reported profiles whose modes may populate a scope's settings choices, read in one batch.
    pub async fn agents(conn: &mut Conn, ids: &[Id]) -> Result<Vec<AgentRow>, DbError> {
        Ok(sqlx::query_as("SELECT id,name,owner_id,home_id,harness_kind,machine_id,profile,descriptor,descriptor_overrides,
            disabled,deleted_at FROM agents WHERE id=ANY($1) AND deleted_at IS NULL AND NOT disabled ORDER BY id")
            .bind(ids).fetch_all(conn).await?)
    }

    /// A harness process: one for a provider and an agent profile in a place.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct HarnessRow {
        /// The provider it serves.
        pub provider_id: Id,
        /// The profile it runs.
        pub agent_id: Id,
        /// The machine it runs on.
        pub machine_id: Id,
        /// `offline`, `starting`, `online` or `error`.
        pub status: String,
        /// A sentence about the status.
        pub status_detail: Option<String>,
        /// The version that runs.
        pub harness_version: Option<String>,
        /// What its adapter declares.
        pub descriptor: Json,
        /// How many sessions it holds open.
        pub sessions_open: i32,
        /// When it last did something.
        pub last_active_at: Option<DateTime<Utc>>,
        /// When its row last changed.
        pub updated_at: DateTime<Utc>,
    }

    /// The harness process of a provider and a profile on a machine.
    pub async fn harness(
        conn: &mut Conn,
        provider: Id,
        agent: Id,
        machine: Id,
    ) -> Result<Option<HarnessRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT provider_id, agent_id, machine_id, status, status_detail, harness_version, descriptor,
                    sessions_open, last_active_at, updated_at
               FROM harnesses WHERE provider_id = $1 AND agent_id = $2 AND machine_id = $3",
        )
        .bind(provider)
        .bind(agent)
        .bind(machine)
        .fetch_optional(conn)
        .await?)
    }

    /// What a harness process says of itself.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct HarnessReport<'a> {
        /// `online`, `starting`, `offline`, `error`: its state.
        pub status: &'a str,
        /// A sentence about it.
        pub detail: Option<&'a str>,
        /// The version of the harness; a row keeps the one it has when none is told.
        pub version: Option<&'a str>,
        /// How many sessions it holds open.
        pub sessions_open: i32,
    }

    /// Records the state of a harness process, making its row if it has
    /// none. The answer is the status it had before, none for a row that
    /// was not there.
    pub async fn set_harness_status(
        conn: &mut Conn,
        provider: Id,
        agent: Id,
        machine: Id,
        report: &HarnessReport<'_>,
        at: DateTime<Utc>,
    ) -> Result<Option<String>, DbError> {
        let before: Option<String> = sqlx::query_scalar(
            "SELECT status FROM harnesses WHERE provider_id = $1 AND agent_id = $2 AND machine_id = $3 FOR UPDATE",
        )
        .bind(provider)
        .bind(agent)
        .bind(machine)
        .fetch_optional(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO harnesses (provider_id, agent_id, machine_id, status, status_detail, harness_version,
                                    sessions_open, last_active_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
             ON CONFLICT (provider_id, agent_id, machine_id) DO UPDATE
                SET status = EXCLUDED.status, status_detail = EXCLUDED.status_detail,
                    harness_version = COALESCE(EXCLUDED.harness_version, harnesses.harness_version),
                    sessions_open = EXCLUDED.sessions_open, last_active_at = EXCLUDED.last_active_at,
                    updated_at = EXCLUDED.updated_at",
        )
        .bind(provider)
        .bind(agent)
        .bind(machine)
        .bind(report.status)
        .bind(report.detail)
        .bind(report.version)
        .bind(report.sessions_open)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(before)
    }

    /// Stores what a harness process declares, on its row and as the
    /// profile's last reported descriptor.
    pub async fn set_harness_descriptor(
        conn: &mut Conn,
        provider: Id,
        agent: Id,
        machine: Id,
        descriptor: &Json,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE harnesses SET descriptor = $4, updated_at = $5
              WHERE provider_id = $1 AND agent_id = $2 AND machine_id = $3",
        )
        .bind(provider)
        .bind(agent)
        .bind(machine)
        .bind(descriptor)
        .bind(at)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE agents SET descriptor = $2, updated_at = $3 WHERE id = $1")
            .bind(agent)
            .bind(descriptor)
            .bind(at)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Marks every harness process on a machine offline (its session went
    /// away, or the system started and no gateway has said which it holds).
    /// The answer is how many changed.
    pub async fn set_machine_harnesses_offline(
        conn: &mut Conn,
        machine: Id,
        at: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        let done = sqlx::query(
            "UPDATE harnesses SET status = 'offline', sessions_open = 0, updated_at = $2
              WHERE machine_id = $1 AND status <> 'offline'",
        )
        .bind(machine)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected())
    }

    /// Sets the status of a provider, as a turn that ended says it is.
    pub async fn set_provider_status(
        conn: &mut Conn,
        provider: Id,
        status: &str,
        detail: &str,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE providers SET status = $2, status_detail = $3, revision = revision + 1, updated_at = $4
              WHERE id = $1 AND status <> $2 AND status <> 'disabled'",
        )
        .bind(provider)
        .bind(status)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// What the engine needs of a machine to know whether a harness can run
    /// on it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct MachinePlace {
        /// Its identifier.
        pub id: Id,
        /// The roles it has switched on.
        pub roles: Vec<String>,
        /// `active` or `revoked`.
        pub status: String,
        /// Whether it had a session when last known.
        pub online: bool,
        /// `connected` or `managed`.
        pub placement: String,
        /// The person it belongs to.
        pub owner_id: Option<Id>,
        /// What it reports about itself, with its local policy.
        pub facts: Json,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// A machine as the name of a resource points at it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct MachineName {
        /// Its identifier.
        pub id: Id,
        /// What it is called.
        pub name: String,
        /// Whether it had a session when last known.
        pub online: bool,
        /// Its operating system.
        pub platform_os: String,
        /// `connected` or `managed`.
        pub placement: String,
    }

    /// A machine, as a reference to it names it.
    pub async fn machine_name(conn: &mut Conn, id: Id) -> Result<Option<MachineName>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, online, platform_os, placement FROM machines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// A machine as a place a harness runs on.
    pub async fn machine_place(conn: &mut Conn, id: Id) -> Result<Option<MachinePlace>, DbError> {
        let row:Option<MachinePlace>=sqlx::query_as(
            "SELECT id, roles, status, online, placement, owner_id, facts, deleted_at FROM machines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
        Ok(row)
    }
}

/// Rows written directly, for tests of what stands on them. A real machine
/// is made by enrolling.
pub mod testing {
    use crate::{Conn, DbError};
    use orch_types::Id;

    /// A machine called `name` with the given placement, active, with a
    /// certificate that nothing matches.
    pub async fn insert_machine(
        conn: &mut Conn,
        name: &str,
        placement: &str,
    ) -> Result<Id, DbError> {
        let id = orch_types::new_id();
        let fingerprint: [u8; 32] = rand::random();
        sqlx::query(
            "INSERT INTO machines (id, name, placement, platform_os, platform_arch, connector_version,
                                   protocol_version, cert_serial, cert_fingerprint, cert_not_after)
             VALUES ($1, $2, $3, 'linux', 'x86_64', '0.1.0', 1, 'serial', $4, now() + interval '1 year')",
        )
        .bind(id)
        .bind(name)
        .bind(placement)
        .bind(fingerprint.as_slice())
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// A person, with nothing else set.
    pub async fn insert_user(conn: &mut Conn, username: &str) -> Result<Id, DbError> {
        let id = orch_types::new_id();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, $2, $2, 3)",
        )
        .bind(id)
        .bind(username)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// A real person for enrollment fixtures: includes the personal home and its root membership.
    pub async fn insert_person_with_home(
        conn: &mut Conn,
        username: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Id, DbError> {
        Ok(crate::queries::identity::create_user(
            conn,
            &crate::queries::identity::NewUser {
                id: orch_types::new_id(),
                username,
                display_name: username,
                email: None,
                email_confirmed: false,
                password_hash: None,
                locale: "en",
                is_system_admin: false,
                created_by: None,
                personal_org: Some(crate::queries::identity::PersonalOrg {
                    closed: true,
                    flat: true,
                }),
            },
            at,
        )
        .await?
        .user
        .id)
    }

    /// Makes a person a system administrator.
    pub async fn make_system_admin(conn: &mut Conn, user: Id) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET is_system_admin = true WHERE id = $1")
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }
}

// ---- What the access engine reads of resources ---------------------------------------------------
//
// One block, kept together: the access engine asks of a resource only what a decision about it
// needs, the same for every kind, in one statement however many kinds are asked about.

/// The kinds of resource, by the words their rows and pages are kept under.
pub const RESOURCE_KINDS: [&str; 9] = [
    "provider",
    "agent",
    "environment",
    "host",
    "machine",
    "template",
    "mcp_server",
    "mount",
    "skill",
];

/// A resource of any kind, with what a decision about it needs and nothing else.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AccessFacts {
    /// `provider`, `agent`, `environment`, `host`, `machine`, `template`, `mcp_server`, `mount` or
    /// `skill`.
    pub kind: String,
    /// Its identifier.
    pub id: Id,
    /// What it is called.
    pub name: String,
    /// Its first owner; none for a resource of the system that nobody enrolled.
    pub owner_id: Option<Id>,
    /// The place its page follows: the org it was made in, or for an environment its home node.
    /// None for a resource of the system.
    pub home_id: Option<Id>,
    /// The root org of that place.
    pub org_id: Option<Id>,
    /// Whether it is switched off: its use is refused, its page remains.
    pub disabled: bool,
    /// Whether it is deleted: its page remains, until it is purged.
    pub deleted: bool,
    /// For a provider and an environment, `managed` or `connected`.
    pub placement: Option<String>,
    /// Whether it is a provider that is a subscription.
    pub subscription: bool,
    /// For a provider that is a subscription, and for an agent profile, the harness it is for.
    pub harness_kind: Option<String>,
    /// For a provider, the arrangement types its owner allows it to be used in.
    pub arrangements: Vec<String>,
    /// The machine it was registered from or runs on, where it has one.
    pub machine_id: Option<Id>,
    /// The person that machine belongs to; none for a machine of the system.
    pub machine_owner_id: Option<Id>,
}

const PROVIDER_FACTS: &str = "SELECT 'provider'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        (x.status = 'disabled') AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        provider_placement(x.machine_id) AS placement, (x.kind = 'subscription') AS subscription, x.harness_kind AS harness_kind,
        x.arrangements AS arrangements, x.machine_id AS machine_id, m.owner_id AS machine_owner_id
   FROM providers x LEFT JOIN machines m ON m.id = x.machine_id";
const AGENT_FACTS: &str =
    "SELECT 'agent'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        x.disabled AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, x.harness_kind AS harness_kind,
        '{}'::text[] AS arrangements, x.machine_id AS machine_id, m.owner_id AS machine_owner_id
   FROM agents x LEFT JOIN machines m ON m.id = x.machine_id";
const ENVIRONMENT_FACTS: &str = "SELECT 'environment'::text AS kind, x.id, x.name, x.owner_id, x.home_node_id AS home_id,
        x.org_id AS org_id, (x.status = 'disabled') AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        x.placement AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, x.machine_id AS machine_id, m.owner_id AS machine_owner_id
   FROM environments x LEFT JOIN machines m ON m.id = x.machine_id";
const HOST_FACTS: &str =
    "SELECT 'host'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        x.disabled AS disabled, (m.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, x.machine_id AS machine_id, m.owner_id AS machine_owner_id
   FROM hosts x JOIN machines m ON m.id = x.machine_id";
const MACHINE_FACTS: &str =
    "SELECT 'machine'::text AS kind,x.id,x.name,x.owner_id,x.home_id,x.org_id,
        (x.status='revoked') AS disabled,(x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement,false AS subscription,NULL::text AS harness_kind,
        '{}'::text[] AS arrangements,x.id AS machine_id,x.owner_id AS machine_owner_id
       FROM machines x WHERE x.runtime_id IS NULL AND NOT EXISTS (SELECT 1 FROM hosts h WHERE h.machine_id=x.id)";
const TEMPLATE_FACTS: &str =
    "SELECT 'template'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        (NOT x.enabled) AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, NULL::uuid AS machine_id, NULL::uuid AS machine_owner_id
   FROM env_templates x";
// A server that belongs to one environment is written on that environment's page and has no page
// of its own.
const MCP_FACTS: &str =
    "SELECT 'mcp_server'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        (NOT x.enabled) AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, NULL::uuid AS machine_id, NULL::uuid AS machine_owner_id
   FROM mcp_servers x LEFT JOIN machines m ON m.id = x.machine_id WHERE x.environment_id IS NULL";
const MOUNT_FACTS: &str =
    "SELECT 'mount'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        (NOT x.enabled) AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, NULL::uuid AS machine_id, NULL::uuid AS machine_owner_id
   FROM mounts x";
// A skill's home is an org or a project; it is never the system's.
const SKILL_FACTS: &str =
    "SELECT 'skill'::text AS kind, x.id, x.name, x.owner_id, x.home_id, x.org_id,
        (NOT x.enabled) AS disabled, (x.deleted_at IS NOT NULL) AS deleted,
        NULL::text AS placement, false AS subscription, NULL::text AS harness_kind,
        '{}'::text[] AS arrangements, NULL::uuid AS machine_id, NULL::uuid AS machine_owner_id
   FROM skills x";

/// The select that reads the facts of the kind `kind`, if it is one.
fn facts_select(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "provider" => PROVIDER_FACTS,
        "agent" => AGENT_FACTS,
        "environment" => ENVIRONMENT_FACTS,
        "host" => HOST_FACTS,
        "machine" => MACHINE_FACTS,
        "template" => TEMPLATE_FACTS,
        "mcp_server" => MCP_FACTS,
        "mount" => MOUNT_FACTS,
        "skill" => SKILL_FACTS,
        _ => return None,
    })
}

/// Live resources in a membership's tree, including private ones the ending must settle.
pub async fn resources_in_root(conn: &mut Conn, root: Id) -> Result<Vec<AccessFacts>, DbError> {
    let facts = RESOURCE_KINDS
        .iter()
        .filter_map(|kind| facts_select(kind))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.* FROM ({facts}) f WHERE f.org_id=$1 AND NOT f.deleted ORDER BY f.id"
    )))
    .bind(root)
    .fetch_all(conn)
    .await?)
}

/// Live resources whose own homes lie in the subtree, excluding merely borrowed resources.
pub async fn resources_under_home(conn: &mut Conn, node: Id) -> Result<Vec<AccessFacts>, DbError> {
    let facts = RESOURCE_KINDS
        .iter()
        .filter_map(|kind| facts_select(kind))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.* FROM ({facts}) f JOIN nodes n ON n.id=f.home_id WHERE $1=ANY(n.path) AND NOT f.deleted ORDER BY f.id"
    ))).bind(node).fetch_all(conn).await?)
}

/// Registrations on the member's personal machines, even if another person also owns the page.
pub async fn registrations_of_member(
    conn: &mut Conn,
    root: Id,
    user: Id,
) -> Result<Vec<AccessFacts>, DbError> {
    let facts = ["provider", "environment", "agent", "mcp_server"]
        .iter()
        .filter_map(|kind| facts_select(kind))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.* FROM ({facts}) f JOIN machines m ON m.id=f.machine_id
            WHERE f.org_id=$1 AND m.owner_id=$2 AND m.placement='connected' AND NOT f.deleted
            AND (f.placement IS NULL OR f.placement='connected') ORDER BY f.id"
    )))
    .bind(root)
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The table that keeps the resources of the kind `kind`.
fn resource_table(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "provider" => "providers",
        "agent" => "agents",
        "environment" => "environments",
        "host" => "hosts",
        "machine" => "machines",
        "template" => "env_templates",
        "mcp_server" => "mcp_servers",
        "mount" => "mounts",
        "skill" => "skills",
        _ => return None,
    })
}

/// The facts of the resources `ids`, of whatever kinds, deleted or not, in one statement. An
/// identifier that is no resource is not among them.
pub async fn resource_facts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<AccessFacts>, DbError> {
    let selects: Vec<String> = RESOURCE_KINDS
        .iter()
        .filter_map(|kind| facts_select(kind))
        .map(|select| {
            let glue = if select.contains(" WHERE ") {
                "AND"
            } else {
                "WHERE"
            };
            format!("{select} {glue} x.id = ANY($1)")
        })
        .collect();
    // Every permission decision about a resource reads it here. In a write whose commit checks the authorization
    // epochs through `transaction_guard`, the epochs of these resources' root orgs are read in this same statement.
    Ok(
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "WITH found AS MATERIALIZED ({}), watched AS MATERIALIZED (
                 SELECT orch_authz_saw(orch_authz_epochs(ARRAY(SELECT DISTINCT org_id FROM found WHERE org_id IS NOT NULL)))
                  WHERE COALESCE(current_setting('orch.authz_seen', true), '') <> ''
             )
             SELECT found.* FROM found, (SELECT count(*) FROM watched) w",
            selects.join(" UNION ALL ")
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?,
    )
}

/// Who is looking, as far as a list of resources is narrowed to what they might see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Looker<'a> {
    /// The person.
    pub user: Id,
    /// The groups they are in.
    pub groups: &'a [Id],
    /// The root orgs they are members of.
    pub roots: &'a [Id],
    /// Every org they are a member of.
    pub member_of: &'a [Id],
    /// Whether they are a system admin who counts as one.
    pub system_admin: bool,
    /// The moment, for what has ended.
    pub now: DateTime<Utc>,
}

/// The resources of the kind `kind`, not deleted, that `looker` might hold something on, those
/// with an identifier after `after` first, at most `limit` of them. It is a superset, narrowed by
/// what a person could hold at all and nothing more: a resource of the system; any resource, for a
/// system admin; one whose home lies in a tree they are a member of or hold a role in; one whose
/// own page names them, a group of theirs, a role, everyone, or the work in a place. Every other
/// resource gives them nothing, whatever the defaults say, because a defaults page names only the
/// members of its own tree.
pub async fn candidate_resources(
    conn: &mut Conn,
    kind: &str,
    looker: &Looker<'_>,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<AccessFacts>, DbError> {
    let Some(select) = facts_select(kind) else {
        return Ok(Vec::new());
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.* FROM ({select}) f
          WHERE NOT f.deleted AND ($1::uuid IS NULL OR f.id > $1) AND (
                f.home_id IS NULL
             OR $2
             OR f.org_id = ANY($3)
             OR (f.kind='environment' AND f.placement='connected' AND EXISTS (
                 SELECT 1 FROM permission_entries own LEFT JOIN hosts h ON h.machine_id=f.machine_id
                 WHERE own.page='live' AND own.permission='access.own' AND own.removed_at IS NULL
                   AND (own.expires_at IS NULL OR own.expires_at>$4)
                   AND ((own.object_kind='machine' AND own.object_id=f.machine_id)
                     OR (own.object_kind='host' AND own.object_id=h.id))
                   AND ((own.holder_kind='user' AND own.holder_id=$5)
                     OR (own.holder_kind='group' AND own.holder_id=ANY($6)))))
             OR EXISTS (SELECT 1 FROM node_roles r JOIN nodes n ON n.id = r.node_id
                         WHERE n.org_id = f.org_id AND r.removed_at IS NULL
                           AND (r.expires_at IS NULL OR r.expires_at > $4)
                           AND ((r.subject_kind = 'user' AND r.subject_id = $5)
                             OR (r.subject_kind = 'group' AND r.subject_id = ANY($6))
                             OR (r.subject_kind = 'members' AND r.subject_id = ANY($7))))
             OR EXISTS (SELECT 1 FROM permission_entries e
                         WHERE e.object_kind = f.kind AND e.object_id = f.id AND e.page = 'live'
                           AND e.removed_at IS NULL AND (e.expires_at IS NULL OR e.expires_at > $4)
                           AND ((e.holder_kind = 'user' AND e.holder_id = $5)
                             OR (e.holder_kind = 'group' AND e.holder_id = ANY($6))
                             OR e.holder_kind IN ('role', 'everyone', 'node', 'members'))))
          ORDER BY f.id
          LIMIT $8"
    )))
    .bind(after)
    .bind(looker.system_admin)
    .bind(looker.roots)
    .bind(looker.now)
    .bind(looker.user)
    .bind(looker.groups)
    .bind(looker.member_of)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Locks the row of a resource until the transaction ends, so that two changes of its page are
/// made one after the other. The answer says whether there is such a resource.
pub async fn lock_resource(conn: &mut Conn, kind: &str, id: Id) -> Result<bool, DbError> {
    let Some(table) = resource_table(kind) else {
        return Ok(false);
    };
    let found: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE id = $1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?;
    Ok(found.is_some())
}

/// Sets the first owner of a resource, which its Owners row names.
pub async fn set_resource_owner(
    conn: &mut Conn,
    kind: &str,
    id: Id,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let Some(table) = resource_table(kind) else {
        return Ok(());
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET owner_id = $2, updated_at = $3, revision = revision + 1 WHERE id = $1"
    )))
    .bind(id)
    .bind(owner)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// A model of a provider, with what a use of it is checked against.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ModelFacts {
    /// Its routing name: the identity of the model for everything that allows it.
    pub model_name: String,
    /// Whether the provider's managers have it enabled.
    pub enabled: bool,
    /// The reasoning efforts it lists; none if it lists none.
    pub efforts: Vec<String>,
    /// The groups its managers put it in: what a list of models names as `@<group>`.
    pub groups: Vec<String>,
}

/// The models of a provider, enabled or not, by name.
pub async fn provider_models(conn: &mut Conn, provider: Id) -> Result<Vec<ModelFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT model_name, enabled,
                CASE WHEN jsonb_typeof(parameters -> 'efforts') = 'array'
                     THEN ARRAY(SELECT jsonb_array_elements_text(parameters -> 'efforts'))
                     ELSE '{}'::text[] END AS efforts,
                groups
           FROM provider_models WHERE provider_id = $1 ORDER BY model_name",
    )
    .bind(provider)
    .fetch_all(conn)
    .await?)
}

/// The resources of the kind `kind` (`environment` or `provider`) that are registered from the
/// machine `machine` and not deleted: what a key of that machine's local policy bounds.
pub async fn registered_from(conn: &mut Conn, kind: &str, machine: Id) -> Result<Vec<Id>, DbError> {
    let Some(table) = resource_table(kind).filter(|_| matches!(kind, "environment" | "provider"))
    else {
        return Ok(Vec::new());
    };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE machine_id = $1 AND deleted_at IS NULL ORDER BY id"
    )))
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// A thing that a person owns, with whom else owns it and where it lies.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OwnedRow {
    /// `node`, `document`, or the kind of resource.
    pub object_kind: String,
    /// The thing.
    pub object_id: Id,
    /// The person's entry in its Owners row.
    pub entry_id: Id,
    /// What it is: for a node `org`, `project`, `topic` or `thread`; otherwise as `object_kind`.
    pub thing_kind: String,
    /// Its name.
    pub name: String,
    /// Its first owner.
    pub first_owner: Option<Id>,
    /// Where it lies: the node a node lies in, the node a document is written beside, a resource's
    /// home (an environment's home node). None for a root org.
    pub anchor: Option<Id>,
    /// How many other entries of its Owners row count: people and groups.
    pub others: i64,
    /// Whether it was registered from a machine of this person.
    pub registered: bool,
}

/// Everything `user` owns in the tree of the root org `root` that is not deleted: nodes, documents
/// and resources, whose Owners row names them in person. Each says how many others own it.
pub async fn owned_by(
    conn: &mut Conn,
    root: Id,
    user: Id,
    now: DateTime<Utc>,
) -> Result<Vec<OwnedRow>, DbError> {
    let facts: Vec<String> = RESOURCE_KINDS
        .iter()
        .filter_map(|kind| facts_select(kind))
        .map(str::to_owned)
        .collect();
    let facts = facts.join(" UNION ALL ");
    // The facts of a kind that has a WHERE of its own are wrapped, so that the union stays one.
    let others = "(SELECT count(*) FROM permission_entries o
                    WHERE o.object_kind = e.object_kind AND o.object_id = e.object_id AND o.page = 'live'
                      AND o.permission = 'access.own' AND o.removed_at IS NULL AND o.id <> e.id
                      AND (o.expires_at IS NULL OR o.expires_at > $3))";
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT e.object_kind, e.object_id, e.id AS entry_id, n.kind AS thing_kind, n.name,
                n.owner_id AS first_owner, n.parent_id AS anchor, {others} AS others, false AS registered
           FROM permission_entries e JOIN nodes n ON n.id = e.object_id
          WHERE e.object_kind = 'node' AND e.page = 'live' AND e.permission = 'access.own'
            AND e.removed_at IS NULL AND e.holder_kind = 'user' AND e.holder_id = $2
            AND n.org_id = $1 AND n.deleted_at IS NULL
         UNION ALL
         SELECT e.object_kind, e.object_id, e.id, 'document', d.title, d.owner_id, d.node_id, {others}, false
           FROM permission_entries e JOIN documents d ON d.id = e.object_id
          WHERE e.object_kind = 'document' AND e.page = 'live' AND e.permission = 'access.own'
            AND e.removed_at IS NULL AND e.holder_kind = 'user' AND e.holder_id = $2
            AND d.org_id = $1 AND d.deleted_at IS NULL
         UNION ALL
         SELECT e.object_kind,e.object_id,e.id,'document_container',c.name,c.owner_id,c.node_id,{others},false
           FROM permission_entries e JOIN document_containers c ON c.id=e.object_id
          WHERE e.object_kind='document_container' AND e.page='live' AND e.permission='access.own'
            AND e.removed_at IS NULL AND e.holder_kind='user' AND e.holder_id=$2
            AND c.org_id=$1 AND c.deleted_at IS NULL
         UNION ALL
         SELECT e.object_kind, e.object_id, e.id, f.kind, f.name, f.owner_id, f.home_id, {others},
                COALESCE(f.machine_owner_id = $2 AND f.kind IN ('provider', 'environment', 'agent', 'mcp_server')
                    AND EXISTS(SELECT 1 FROM machines m WHERE m.id=f.machine_id AND m.placement='connected')
                    AND (f.placement IS NULL OR f.placement='connected'), false)
           FROM permission_entries e JOIN ({facts}) f ON f.kind = e.object_kind AND f.id = e.object_id
          WHERE e.page = 'live' AND e.permission = 'access.own' AND e.removed_at IS NULL
            AND e.holder_kind = 'user' AND e.holder_id = $2 AND f.org_id = $1 AND NOT f.deleted
          ORDER BY 2"
    )))
    .bind(root)
    .bind(user)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// An entry that stopped naming a person.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct EntryTaken {
    /// The entry.
    pub id: Id,
    /// What its page is of.
    pub object_kind: String,
    /// Which one.
    pub object_id: Id,
    /// The row it was in.
    pub permission: String,
}

/// Takes every entry that names `user` in person out of the pages of the tree of the root org
/// `root`: the nodes in it, its documents and its resources, the Owners rows left alone. The entries
/// are kept for the record and count for nothing from then on.
pub async fn take_entries_naming(
    conn: &mut Conn,
    root: Id,
    user: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<Vec<EntryTaken>, DbError> {
    let facts: Vec<String> = RESOURCE_KINDS
        .iter()
        .filter_map(|kind| facts_select(kind))
        .map(str::to_owned)
        .collect();
    let facts = facts.join(" UNION ALL ");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE permission_entries e SET removed_at = $4, removed_by = $3, revision = e.revision + 1
          WHERE e.removed_at IS NULL AND e.holder_kind = 'user' AND e.holder_id = $2
            AND e.permission <> 'access.own'
            AND ((e.object_kind = 'node' AND EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.object_id AND n.org_id = $1))
              OR (e.object_kind = 'document' AND EXISTS (SELECT 1 FROM documents d WHERE d.id = e.object_id AND d.org_id = $1))
              OR (e.object_kind = 'document_container' AND EXISTS (SELECT 1 FROM document_containers c WHERE c.id=e.object_id AND c.org_id=$1))
              OR EXISTS (SELECT 1 FROM ({facts}) f WHERE f.kind = e.object_kind AND f.id = e.object_id AND f.org_id = $1))
      RETURNING e.id, e.object_kind, e.object_id, e.permission"
    )))
    .bind(root)
    .bind(user)
    .bind(by)
    .bind(at)
    .fetch_all(conn)
    .await?)
}

/// Sets the first owner of a document, which its Owners row names.
pub async fn set_document_owner(
    conn: &mut Conn,
    id: Id,
    owner: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE documents SET owner_id = $2, updated_at = $3 WHERE id = $1")
        .bind(id)
        .bind(owner)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// A thread that is bound to a provider, and who is its first owner.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BoundThread {
    /// The thread.
    pub thread: Id,
    /// Its name.
    pub name: String,
    /// The provider it runs on.
    pub provider: Id,
}

/// The threads of the tree of `root` that `owner` is the first owner of and that run on a provider
/// (not deleted), in the order of their identifiers.
pub async fn threads_first_owned_by(
    conn: &mut Conn,
    root: Id,
    owner: Id,
) -> Result<Vec<BoundThread>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.id AS thread, n.name, t.provider_id AS provider
           FROM nodes n JOIN threads t ON t.node_id = n.id
          WHERE n.org_id = $1 AND n.owner_id = $2 AND n.deleted_at IS NULL AND t.provider_id IS NOT NULL
          ORDER BY n.id",
    )
    .bind(root)
    .bind(owner)
    .fetch_all(conn)
    .await?)
}

/// Resources written directly, for the tests of the access engine. A real resource is made by the
/// request that makes it.
pub mod access_fixtures {
    use super::testing;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, new_id};

    /// A resource of `kind` named `name`, first owned by `owner`, whose home is `home`: an org, or
    /// for an environment its home node; none for the system's. Everything else is as a resource
    /// starts: an API provider that is ready, a managed environment, an enabled template, a host
    /// on a machine of its own whose node has reported the drivers it offers, an agent profile for
    /// the fake harness.
    pub async fn insert_resource(
        conn: &mut Conn,
        kind: &str,
        owner: Option<Id>,
        home: Option<Id>,
        name: &str,
    ) -> Result<Id, DbError> {
        let id = new_id();
        match kind {
            "machine" => {
                let machine = testing::insert_machine(conn, name, "connected").await?;
                sqlx::query("UPDATE machines SET id=$1,owner_id=$2,home_id=$3,org_id=(SELECT path[1] FROM nodes WHERE id=$3) WHERE id=$4")
                    .bind(id).bind(owner).bind(home).bind(machine).execute(conn).await?;
            }
            "provider" => {
                sqlx::query(
                    "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams,
                                            arrangements, status)
                     SELECT $1, $2, $3, n.org_id, n.id, 'api',
                            '[{\"protocol\":\"openai-responses\",\"base_url\":\"http://api.test\",\"options\":{}}]'::jsonb,
                            ARRAY['separated','key_separated'], 'ready'
                       FROM nodes n WHERE n.id = $4",
                )
                .bind(id)
                .bind(name)
                .bind(owner)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "agent" => {
                sqlx::query(
                    "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind)
                     SELECT $1, $2, $3, n.org_id, n.id, 'fake'
                       FROM (SELECT $4::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home",
                )
                .bind(id)
                .bind(name)
                .bind(owner)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "template" => {
                sqlx::query(
                    "INSERT INTO env_templates (id, name, owner_id, org_id, home_id, driver, spec)
                     SELECT $1, $2, $3, n.org_id, n.id, 'test', '{}'::jsonb
                       FROM (SELECT $4::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home",
                )
                .bind(id)
                .bind(name)
                .bind(owner)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "host" => {
                let machine = testing::insert_machine(conn, name, "managed").await?;
                // Its node has reported the drivers its operator lets it offer, as an enrolled one does.
                let policy = serde_json::json!({
                    "drivers": orch_link_contract::policy::MachineDrivers::managed()
                });
                sqlx::query(
                    "UPDATE machines SET facts = jsonb_set(facts, '{policy}', $2) WHERE id = $1",
                )
                .bind(machine)
                .bind(policy)
                .execute(&mut *conn)
                .await?;
                sqlx::query(
                    "INSERT INTO hosts (id, machine_id, owner_id, org_id, home_id, name)
                     SELECT $1, $2, $3, n.org_id, n.id, $4
                       FROM (SELECT $5::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home",
                )
                .bind(id)
                .bind(machine)
                .bind(owner)
                .bind(name)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "mcp_server" => {
                sqlx::query(
                    "INSERT INTO mcp_servers (id, org_id, home_id, owner_id, name, spec)
                     SELECT $1, n.org_id, n.id, $2, $3, '{}'::jsonb
                       FROM (SELECT $4::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home",
                )
                .bind(id)
                .bind(owner)
                .bind(name)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "mount" => {
                sqlx::query(
                    "INSERT INTO mounts (id, org_id, home_id, owner_id, name, target)
                     SELECT $1, n.org_id, n.id, $2, $3, '/mnt/data'
                       FROM (SELECT $4::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home",
                )
                .bind(id)
                .bind(owner)
                .bind(name)
                .bind(home)
                .execute(conn)
                .await?;
            }
            "skill" => {
                // A skill always has a home and a first owner; its name follows the skill format.
                sqlx::query(
                    "INSERT INTO skills (id, org_id, home_id, owner_id, name)
                     SELECT $1, n.org_id, n.id, $2, lower(regexp_replace($3, '[^A-Za-z0-9]+', '-', 'g'))
                       FROM nodes n WHERE n.id = $4",
                )
                .bind(id)
                .bind(owner)
                .bind(name)
                .bind(home)
                .execute(conn)
                .await?;
            }
            _ => {
                return insert_environment(
                    conn,
                    owner.unwrap_or_else(new_id),
                    home,
                    name,
                    "managed",
                )
                .await;
            }
        }
        Ok(id)
    }

    /// An environment of the given placement (`managed` or `connected`) named `name`, first owned
    /// by `owner`, whose home node is `home`. A managed one is made from a template of the
    /// system's; a connected one is a directory on a machine of its owner's.
    pub async fn insert_environment(
        conn: &mut Conn,
        owner: Id,
        home: Option<Id>,
        name: &str,
        placement: &str,
    ) -> Result<Id, DbError> {
        let id = new_id();
        let template: Option<Id> = if placement == "managed" {
            let template = new_id();
            sqlx::query(
                "INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'test', 'test', '{}')",
            )
            .bind(template)
            .execute(&mut *conn)
            .await?;
            Some(template)
        } else {
            None
        };
        let machine = if placement == "connected" {
            let machine = testing::insert_machine(conn, name, "connected").await?;
            sqlx::query("UPDATE machines SET owner_id = $2 WHERE id = $1")
                .bind(machine)
                .bind(owner)
                .execute(&mut *conn)
                .await?;
            Some(machine)
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, template_id, machine_id)
             SELECT $1, $2, $3, n.org_id, $4, n.id, $5, $6 FROM nodes n WHERE n.id = $7",
        )
        .bind(id)
        .bind(name)
        .bind(owner)
        .bind(placement)
        .bind(template)
        .bind(machine)
        .bind(home)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// A subscription provider of the harness `harness_kind`, signed in through a profile made for
    /// it, first owned by `owner`, at home `home`.
    pub async fn insert_subscription(
        conn: &mut Conn,
        owner: Id,
        home: Id,
        name: &str,
        harness_kind: &str,
    ) -> Result<Id, DbError> {
        let agent = insert_resource(conn, "agent", Some(owner), Some(home), name).await?;
        sqlx::query("UPDATE agents SET harness_kind = $2 WHERE id = $1")
            .bind(agent)
            .bind(harness_kind)
            .execute(&mut *conn)
            .await?;
        let id = new_id();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    arrangements, status)
             SELECT $1, $2, $3, n.org_id, n.id, 'subscription', $5, $6,
                    ARRAY['separated','key_separated'], 'ready'
               FROM nodes n WHERE n.id = $4",
        )
        .bind(id)
        .bind(name)
        .bind(owner)
        .bind(home)
        .bind(harness_kind)
        .bind(agent)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// A model of a provider that lists the efforts `efforts`.
    pub async fn insert_model(
        conn: &mut Conn,
        provider: Id,
        model: &str,
        efforts: &[&str],
        enabled: bool,
        by: Id,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, jsonb_build_object('efforts', $4::text[]), $5, 'manual', $6)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(model)
        .bind(efforts)
        .bind(enabled)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Switches a resource off or on.
    pub async fn set_disabled(
        conn: &mut Conn,
        kind: &str,
        id: Id,
        disabled: bool,
    ) -> Result<(), DbError> {
        let sql = match kind {
            "provider" => {
                "UPDATE providers SET status = CASE WHEN $2 THEN 'disabled' ELSE 'ready' END WHERE id = $1"
            }
            "agent" => "UPDATE agents SET disabled = $2 WHERE id = $1",
            "machine" => {
                "UPDATE machines SET status=CASE WHEN $2 THEN 'revoked' ELSE 'active' END WHERE id=$1"
            }
            "environment" => {
                "UPDATE environments SET status = CASE WHEN $2 THEN 'disabled' ELSE 'none' END WHERE id = $1"
            }
            "template" => "UPDATE env_templates SET enabled = NOT $2 WHERE id = $1",
            "mcp_server" => "UPDATE mcp_servers SET enabled = NOT $2 WHERE id = $1",
            "mount" => "UPDATE mounts SET enabled = NOT $2 WHERE id = $1",
            "skill" => "UPDATE skills SET enabled = NOT $2 WHERE id = $1",
            _ => return Ok(()),
        };
        sqlx::query(sql)
            .bind(id)
            .bind(disabled)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Marks a resource deleted at `at`.
    pub async fn set_deleted(
        conn: &mut Conn,
        kind: &str,
        id: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let sql = match kind {
            "provider" => "UPDATE providers SET deleted_at = $2 WHERE id = $1",
            "agent" => "UPDATE agents SET deleted_at = $2 WHERE id = $1",
            "environment" => "UPDATE environments SET deleted_at = $2 WHERE id = $1",
            "machine" => "UPDATE machines SET deleted_at=$2 WHERE id=$1",
            "host" => {
                "UPDATE machines SET deleted_at = $2 WHERE id = (SELECT machine_id FROM hosts WHERE id = $1)"
            }
            "template" => "UPDATE env_templates SET deleted_at = $2 WHERE id = $1",
            "mcp_server" => "UPDATE mcp_servers SET deleted_at = $2 WHERE id = $1",
            "mount" => "UPDATE mounts SET deleted_at = $2 WHERE id = $1",
            "skill" => "UPDATE skills SET deleted_at = $2 WHERE id = $1",
            _ => return Ok(()),
        };
        sqlx::query(sql).bind(id).bind(at).execute(conn).await?;
        Ok(())
    }

    /// Sets the arrangement types the owner of a provider allows.
    pub async fn set_arrangements(
        conn: &mut Conn,
        provider: Id,
        arrangements: &[&str],
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE providers SET arrangements = $2 WHERE id = $1")
            .bind(provider)
            .bind(arrangements)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// A machine of `owner`'s that is not a host, called `name`, with its page started as enrolling
    /// starts it: `owner` alone in its Owners row, and nobody else using it.
    pub async fn insert_persons_machine(
        conn: &mut Conn,
        owner: Id,
        name: &str,
    ) -> Result<Id, DbError> {
        let machine = testing::insert_machine(conn, name, "connected").await?;
        sqlx::query("UPDATE machines SET owner_id=$2,home_id=n.id,org_id=n.org_id FROM users u,nodes n
                     WHERE machines.id=$1 AND u.id=$2 AND n.id=COALESCE(u.personal_org_id,u.service_org_id)")
            .bind(machine)
            .bind(owner)
            .execute(&mut *conn)
            .await?;
        // As its enrollment starts its page: the owner is the first entry of its Owners row, which
        // is what owning it is, and its Use row follows nothing, so that it is its owner's alone.
        sqlx::query(
            "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, holder_id,
                                             created_by, source)
             VALUES ($1, 'machine', $2, 'live', 'access.own', 'user', $3, $3, 'creation')",
        )
        .bind(new_id())
        .bind(machine)
        .bind(owner)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO permission_rows (object_kind, object_id, page, profile, permission, inherit, updated_by)
             VALUES ('machine', $1, 'live', 'any', 'machine.use', false, $2)",
        )
        .bind(machine)
        .bind(owner)
        .execute(&mut *conn)
        .await?;
        Ok(machine)
    }

    /// Registers a provider, an environment, a profile or an MCP server from the machine
    /// `machine`.
    pub async fn register_on(
        conn: &mut Conn,
        kind: &str,
        id: Id,
        machine: Id,
    ) -> Result<(), DbError> {
        let sql = match kind {
            "provider" => "UPDATE providers SET machine_id = $2 WHERE id = $1",
            "environment" => {
                "UPDATE environments SET machine_id = $2, placement = 'connected', template_id = NULL WHERE id = $1"
            }
            "agent" => "UPDATE agents SET machine_id = $2 WHERE id = $1",
            "mcp_server" => "UPDATE mcp_servers SET machine_id = $2, imported=true WHERE id = $1",
            _ => return Ok(()),
        };
        sqlx::query(sql)
            .bind(id)
            .bind(machine)
            .execute(&mut *conn)
            .await?;
        if kind == "mcp_server" {
            let at: chrono::DateTime<chrono::Utc> =
                sqlx::query_scalar("SELECT updated_at FROM machines WHERE id=$1")
                    .bind(machine)
                    .fetch_one(&mut *conn)
                    .await?;
            super::mcp::offer(conn, id, machine, None, &serde_json::json!({}), at).await?;
        }
        Ok(())
    }

    /// Registers a provider, an environment, a profile or an MCP server from a machine of
    /// `owner`'s, and returns the machine.
    pub async fn register_from_a_machine(
        conn: &mut Conn,
        kind: &str,
        id: Id,
        owner: Id,
    ) -> Result<Id, DbError> {
        let machine = insert_persons_machine(conn, owner, "laptop").await?;
        register_on(conn, kind, id, machine).await?;
        Ok(machine)
    }

    /// Puts the thread `thread` in the environment `environment`.
    pub async fn put_thread_into(
        conn: &mut Conn,
        thread: Id,
        environment: Id,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(environment)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Notes that the data of `mount` is on `machine`: a `path` or a `volume` of `source`.
    pub async fn insert_mount_location(
        conn: &mut Conn,
        mount: Id,
        machine: Id,
        kind: &str,
        source: &str,
        storage: Option<&str>,
        by: Id,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO mount_locations (mount_id, machine_id, kind, source, storage, created_by)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(mount)
        .bind(machine)
        .bind(kind)
        .bind(source)
        .bind(storage)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod access_reads_tests {
    use super::access_fixtures::*;
    use super::*;
    use crate::queries::access::testing::{give_role, insert_member};
    use crate::queries::hierarchy::testing::insert_node;
    use crate::queries::identity::testing::insert_user;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    struct World {
        ada: Id,
        bob: Id,
        acme: Id,
        research: Id,
        web: Id,
        other: Id,
    }

    async fn world(conn: &mut Conn) -> World {
        let (ada, bob) = (new_id(), new_id());
        insert_user(conn, ada, "ada").await.unwrap();
        insert_user(conn, bob, "bob").await.unwrap();
        let (acme, research, web, other) = (new_id(), new_id(), new_id(), new_id());
        insert_node(conn, acme, "org", None, "Acme", ada)
            .await
            .unwrap();
        insert_node(conn, research, "org", Some(acme), "Research", ada)
            .await
            .unwrap();
        insert_node(conn, web, "project", Some(research), "Web", ada)
            .await
            .unwrap();
        insert_node(conn, other, "org", None, "Globex", bob)
            .await
            .unwrap();
        World {
            ada,
            bob,
            acme,
            research,
            web,
            other,
        }
    }

    #[tokio::test]
    async fn the_facts_of_every_kind_are_read_in_one_statement_with_their_homes() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let mut ids = Vec::new();
        for kind in RESOURCE_KINDS {
            let home = if kind == "environment" {
                w.web
            } else {
                w.research
            };
            ids.push(
                insert_resource(
                    &mut conn,
                    kind,
                    Some(w.ada),
                    Some(home),
                    &format!("{kind}-1"),
                )
                .await
                .unwrap(),
            );
        }
        let facts = resource_facts(&mut conn, &ids).await.unwrap();
        let expected_kinds = [
            "agent",
            "environment",
            "host",
            "machine",
            "mcp_server",
            "mount",
            "provider",
            "skill",
            "template",
        ];
        let mut actual_kinds: Vec<_> = facts.iter().map(|fact| fact.kind.as_str()).collect();
        actual_kinds.sort_unstable();
        assert_eq!(actual_kinds, expected_kinds);
        let mut actual_ids: Vec<_> = facts.iter().map(|fact| fact.id).collect();
        actual_ids.sort_unstable();
        ids.sort_unstable();
        assert_eq!(actual_ids, ids, "every requested resource exactly once");
        let by_kind = |kind: &str| facts.iter().find(|f| f.kind == kind).unwrap();
        assert_eq!(by_kind("provider").home_id, Some(w.research));
        assert_eq!(by_kind("provider").org_id, Some(w.acme));
        assert_eq!(by_kind("environment").home_id, Some(w.web), "its home node");
        assert_eq!(by_kind("environment").org_id, Some(w.acme));
        assert_eq!(by_kind("environment").placement.as_deref(), Some("managed"));
        assert_eq!(by_kind("machine").home_id, Some(w.research));
        assert_eq!(by_kind("machine").org_id, Some(w.acme));
        assert!(facts.iter().all(|f| !f.disabled && !f.deleted));
        // Something that is no resource is not among them.
        assert!(
            resource_facts(&mut conn, &[new_id()])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_resource_of_the_system_has_no_home_and_a_disabled_or_deleted_one_says_so() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let system = insert_resource(&mut conn, "template", None, None, "base")
            .await
            .unwrap();
        let provider = insert_resource(&mut conn, "provider", Some(w.ada), Some(w.acme), "pro")
            .await
            .unwrap();
        set_disabled(&mut conn, "provider", provider, true)
            .await
            .unwrap();
        let host = insert_resource(&mut conn, "host", Some(w.ada), Some(w.acme), "gpu")
            .await
            .unwrap();
        set_deleted(&mut conn, "host", host, clock::start())
            .await
            .unwrap();
        let facts = resource_facts(&mut conn, &[system, provider, host])
            .await
            .unwrap();
        let get = |id: Id| facts.iter().find(|f| f.id == id).unwrap();
        assert_eq!((get(system).home_id, get(system).org_id), (None, None));
        assert!(get(provider).disabled && !get(provider).deleted);
        assert!(get(host).deleted);
    }

    #[tokio::test]
    async fn a_subscription_tells_its_harness_and_a_registered_one_its_machines_owner() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let sub = insert_subscription(&mut conn, w.ada, w.acme, "plan", "codex")
            .await
            .unwrap();
        // This person has no personal organization; use their actual team home.
        let machine = insert_resource(&mut conn, "machine", Some(w.bob), Some(w.other), "laptop")
            .await
            .unwrap();
        register_on(&mut conn, "provider", sub, machine)
            .await
            .unwrap();
        let facts = resource_facts(&mut conn, &[sub]).await.unwrap();
        assert!(facts[0].subscription);
        assert_eq!(facts[0].harness_kind.as_deref(), Some("codex"));
        assert_eq!(facts[0].machine_id, Some(machine));
        assert_eq!(facts[0].machine_owner_id, Some(w.bob));
        assert_eq!(facts[0].placement.as_deref(), Some("connected"));
    }

    #[tokio::test]
    async fn candidates_are_the_resources_a_person_might_hold_something_on_and_page_by_identifier()
    {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        insert_member(&mut conn, w.acme, w.bob, w.acme)
            .await
            .unwrap();
        let in_tree = insert_resource(&mut conn, "provider", Some(w.ada), Some(w.research), "a")
            .await
            .unwrap();
        let elsewhere = insert_resource(&mut conn, "provider", Some(w.ada), Some(w.other), "b")
            .await
            .unwrap();
        let deleted = insert_resource(&mut conn, "provider", Some(w.ada), Some(w.research), "d")
            .await
            .unwrap();
        set_deleted(&mut conn, "provider", deleted, clock::start())
            .await
            .unwrap();
        let carol = new_id();
        insert_user(&mut conn, carol, "carol").await.unwrap();
        let l = Looker {
            user: w.bob,
            groups: &[],
            roots: &[w.acme],
            member_of: &[w.acme],
            system_admin: false,
            now: clock::start(),
        };
        let found = candidate_resources(&mut conn, "provider", &l, None, 10)
            .await
            .unwrap();
        let ids: Vec<Id> = found.iter().map(|f| f.id).collect();
        assert!(ids.contains(&in_tree), "a tree they are a member of");
        assert!(
            !ids.contains(&elsewhere),
            "another tree, naming nobody of theirs"
        );
        assert!(!ids.contains(&deleted), "deleted");
        // A stranger with a role in the tree is a candidate there; one without anything is none.
        give_role(&mut conn, w.web, "user", carol, "viewer", w.ada, None)
            .await
            .unwrap();
        let stranger = Looker {
            user: carol,
            groups: &[],
            roots: &[],
            member_of: &[],
            system_admin: false,
            now: clock::start(),
        };
        let found = candidate_resources(&mut conn, "provider", &stranger, None, 10)
            .await
            .unwrap();
        assert_eq!(found.iter().map(|f| f.id).collect::<Vec<_>>(), [in_tree]);
        // Paged by identifier.
        let first = candidate_resources(&mut conn, "provider", &l, None, 1)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let rest = candidate_resources(&mut conn, "provider", &l, Some(first[0].id), 10)
            .await
            .unwrap();
        assert!(rest.iter().all(|f| f.id > first[0].id));
    }

    #[tokio::test]
    async fn the_models_of_a_provider_list_their_efforts() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let provider = insert_resource(&mut conn, "provider", Some(w.ada), Some(w.acme), "pro")
            .await
            .unwrap();
        insert_model(&mut conn, provider, "big", &["low", "high"], true, w.ada)
            .await
            .unwrap();
        insert_model(&mut conn, provider, "small", &[], false, w.ada)
            .await
            .unwrap();
        let models = provider_models(&mut conn, provider).await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_name, "big");
        assert_eq!(models[0].efforts, ["low", "high"]);
        assert!(models[0].enabled);
        assert!(!models[1].enabled && models[1].efforts.is_empty());
    }

    #[tokio::test]
    async fn the_first_owner_of_a_resource_is_set_and_locked_for_the_change() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let w = world(&mut conn).await;
        let host = insert_resource(&mut conn, "host", Some(w.ada), Some(w.acme), "gpu")
            .await
            .unwrap();
        let mount = insert_resource(&mut conn, "mount", Some(w.ada), Some(w.acme), "data")
            .await
            .unwrap();
        assert!(lock_resource(&mut conn, "host", host).await.unwrap());
        assert!(!lock_resource(&mut conn, "host", new_id()).await.unwrap());
        assert!(!lock_resource(&mut conn, "nonsense", host).await.unwrap());
        for (kind, id) in [("host", host), ("mount", mount)] {
            set_resource_owner(&mut conn, kind, id, w.bob, clock::start())
                .await
                .unwrap();
        }
        let facts = resource_facts(&mut conn, &[host, mount]).await.unwrap();
        assert!(facts.iter().all(|f| f.owner_id == Some(w.bob)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;
    use serde_json::json;

    fn enrolled<'a>(id: Id, fingerprint: &'a [u8], roles: &'a [String]) -> NewMachine<'a> {
        NewMachine {
            id,
            name: "laptop",
            owner_id: None,
            placement: "connected",
            roles,
            platform_os: "linux",
            platform_arch: "x86_64",
            connector_version: "0.1.0",
            protocol_version: 1,
            cert_serial: "01",
            cert_fingerprint: fingerprint,
            cert_not_after: clock::start() + Duration::days(365),
            public_key: "cHVibGlj",
        }
    }

    #[tokio::test]
    async fn an_enrolled_machine_is_read_back_with_the_key_its_secrets_are_sealed_to() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (id, fingerprint) = (new_id(), [7u8; 32]);
        let roles = vec!["agent".to_owned()];
        insert_machine(
            &mut conn,
            &enrolled(id, &fingerprint, &roles),
            clock::start(),
        )
        .await
        .unwrap();
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert!(row.is_active());
        assert_eq!(row.cert_fingerprint, fingerprint);
        assert_eq!(row.roles, roles);
        assert_eq!(row.link().public_key, "cHVibGlj");
        assert!(!row.online);
    }

    #[tokio::test]
    async fn the_facts_a_machine_reports_replace_the_old_and_keep_what_the_link_keeps() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (id, fingerprint) = (new_id(), [7u8; 32]);
        insert_machine(&mut conn, &enrolled(id, &fingerprint, &[]), clock::start())
            .await
            .unwrap();
        let now = clock::start();
        set_machine_facts(
            &mut conn,
            id,
            &json!({ "hostname": "a" }),
            (1000, 4000),
            0.25,
            now,
        )
        .await
        .unwrap();
        set_machine_facts(
            &mut conn,
            id,
            &json!({ "hostname": "b" }),
            (2000, 0),
            0.25,
            now,
        )
        .await
        .unwrap();
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.facts["hostname"], "b");
        assert_eq!(row.link().public_key, "cHVibGlj");
        assert_eq!(
            row.rate_up_bps,
            Some(1250),
            "a quarter of the way to the new rate"
        );
        assert_eq!(
            row.rate_down_bps,
            Some(4000),
            "a rate of 0 is no measurement"
        );
    }

    #[tokio::test]
    async fn a_renewed_certificate_is_known_beside_the_old_until_a_session_shows_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (id, fingerprint) = (new_id(), [7u8; 32]);
        insert_machine(&mut conn, &enrolled(id, &fingerprint, &[]), clock::start())
            .await
            .unwrap();
        let next = NextCertificate {
            fingerprint: "ab".repeat(32),
            public_key: "bmV4dA".to_owned(),
            serial: "02".to_owned(),
            not_after: clock::start() + Duration::days(730),
            again: false,
        };
        set_next_certificate(&mut conn, id, &next, clock::start())
            .await
            .unwrap();
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(
            row.cert_fingerprint, fingerprint,
            "the old one is still the machine's"
        );
        assert_eq!(row.link().next, Some(next.clone()));
        assert_eq!(
            row.cert_next_fingerprint.as_deref(),
            Some(next.fingerprint.as_str())
        );
        assert!(row.facts.get(LINK_MEMBER).is_none());
        assert!(
            !promote_next_certificate(&mut conn, id, &"cd".repeat(32), clock::start())
                .await
                .unwrap()
        );
        assert!(
            promote_next_certificate(&mut conn, id, &next.fingerprint, clock::start())
                .await
                .unwrap()
        );
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.cert_fingerprint, vec![0xab; 32]);
        assert_eq!(row.cert_serial, "02");
        assert_eq!(
            row.cert_public_key.as_deref(),
            Some(next.public_key.as_str())
        );
        assert!(row.cert_next_fingerprint.is_none());
        assert!(row.cert_next_again.is_none());
        assert_eq!(row.link().public_key, "bmV4dA");
        assert_eq!(row.link().next, None);
    }

    #[tokio::test]
    async fn older_certificate_state_survives_reload_and_cannot_be_replaced_by_reported_facts() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let id = new_id();
        insert_machine(&mut conn, &enrolled(id, &[7u8; 32], &[]), clock::start())
            .await
            .unwrap();
        let pending = NextCertificate {
            fingerprint: "ab".repeat(32),
            public_key: "bmV4dA".into(),
            serial: "02".into(),
            not_after: clock::start() + Duration::days(730),
            again: true,
        };
        let old = LinkCertificates {
            public_key: "cHVibGlj".into(),
            next: Some(pending.clone()),
        };
        // This is a retained row from the older encoding, on the regenerated schema.
        sqlx::query("UPDATE machines SET cert_public_key=NULL, facts=$2 WHERE id=$1")
            .bind(id)
            .bind(json!({LINK_MEMBER:old,"hostname":"old"}))
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let mut conn = test.pool.acquire().await.unwrap();
        let before = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(before.link(), old);
        set_machine_facts(
            &mut conn,
            id,
            &json!({"hostname":"new",
            LINK_MEMBER:{"public_key":"spoof","next":null}}),
            (0, 0),
            0.25,
            clock::start(),
        )
        .await
        .unwrap();
        let after = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(
            after.link(),
            old,
            "a peer's report cannot rotate an enrollment key or discard a successor"
        );
        assert_eq!(after.facts, json!({"hostname":"new"}));
        assert_eq!(after.cert_public_key.as_deref(), Some("cHVibGlj"));
        assert_eq!(after.cert_next_again, Some(true));
        drop(conn);
        let mut conn = test.pool.acquire().await.unwrap();
        assert!(
            promote_next_certificate(&mut conn, id, &pending.fingerprint, clock::start())
                .await
                .unwrap()
        );
        let promoted = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(promoted.cert_fingerprint, vec![0xab; 32]);
        assert_eq!(promoted.link().public_key, pending.public_key);
        assert!(promoted.link().next.is_none());
        assert!(promoted.facts.get(LINK_MEMBER).is_none());
    }

    #[tokio::test]
    async fn a_revoked_machine_stays_revoked() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let id = testing::insert_machine(&mut conn, "m", "connected")
            .await
            .unwrap();
        assert!(revoke_machine(&mut conn, id, clock::start()).await.unwrap());
        assert!(!revoke_machine(&mut conn, id, clock::start()).await.unwrap());
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert!(!row.is_active());
    }

    #[tokio::test]
    async fn presence_keeps_the_last_known_address_and_is_forgotten_at_a_start() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let id = testing::insert_machine(&mut conn, "m", "connected")
            .await
            .unwrap();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        set_presence(&mut conn, id, true, Some(ip), clock::start())
            .await
            .unwrap();
        set_presence(&mut conn, id, false, None, clock::start())
            .await
            .unwrap();
        let row = machine(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(row.last_ip.as_deref(), Some("203.0.113.7"));
        assert!(!row.online);
        set_presence(&mut conn, id, true, None, clock::start())
            .await
            .unwrap();
        assert_eq!(reset_presence(&mut conn).await.unwrap(), 1);
        assert!(!machine(&mut conn, id).await.unwrap().unwrap().online);
    }

    #[tokio::test]
    async fn a_token_is_spent_once_and_its_row_is_locked_while_it_is_looked_at() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let creator = testing::insert_user(&mut conn, "ada").await.unwrap();
        let machine_id = testing::insert_machine(&mut conn, "m", "connected")
            .await
            .unwrap();
        let token = TokenRow {
            token_hash: vec![9; 32],
            created_by: creator,
            purpose: "connected".to_owned(),
            org_id: None,
            name_hint: Some("laptop".to_owned()),
            expires_at: clock::start() + Duration::minutes(15),
            used_at: None,
            machine_id: None,
            again_machine_id: None,
        };
        assert!(insert_enrollment_token(&mut conn, &token).await.unwrap());
        assert!(!insert_enrollment_token(&mut conn, &token).await.unwrap());
        let found = lock_enrollment_token(&mut conn, &token.token_hash)
            .await
            .unwrap();
        assert_eq!(found.as_ref(), Some(&token));
        assert!(
            spend_enrollment_token(&mut conn, &token.token_hash, machine_id, clock::start())
                .await
                .unwrap()
        );
        assert!(
            !spend_enrollment_token(&mut conn, &token.token_hash, machine_id, clock::start())
                .await
                .unwrap()
        );
        assert!(
            !extend_unused_token(
                &mut conn,
                &token.token_hash,
                clock::start() + Duration::days(1)
            )
            .await
            .unwrap(),
            "a spent token is not made good again"
        );
    }

    #[tokio::test]
    async fn the_first_system_administrator_is_who_the_system_acts_for() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        assert_eq!(first_system_admin(&mut conn).await.unwrap(), None);
        let ada = testing::insert_user(&mut conn, "ada").await.unwrap();
        testing::make_system_admin(&mut conn, ada).await.unwrap();
        assert_eq!(first_system_admin(&mut conn).await.unwrap(), Some(ada));
    }

    #[tokio::test]
    async fn a_host_of_the_system_has_no_home_and_no_org() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let machine_id = testing::insert_machine(&mut conn, "node", "managed")
            .await
            .unwrap();
        let owner = testing::insert_user(&mut conn, "ada").await.unwrap();
        let host = NewHost {
            id: new_id(),
            machine_id,
            owner_id: Some(owner),
            home_id: None,
            name: "node",
        };
        insert_host(&mut conn, &host, clock::start()).await.unwrap();
        let (org, home): (Option<Id>, Option<Id>) =
            sqlx::query_as("SELECT org_id, home_id FROM hosts WHERE id = $1")
                .bind(host.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!((org, home), (None, None));
    }
}

// ---------------------------------------------------------------- what the routes of threads read of providers

/// A model of a provider as a thread's parameters show it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ProviderModelRow {
    /// The name that is sent.
    pub model_name: String,
    /// The name that is shown; empty when the routing name is.
    pub display_name: String,
    /// What it can be run with here: its efforts, tiers, programs, context and rate.
    pub parameters: Json,
    /// The groups its managers put it in.
    pub groups: Vec<String>,
    /// Whether the provider's managers have it enabled.
    pub enabled: bool,
}

/// Every model of a provider, by name.
pub async fn models_of_provider(
    conn: &mut Conn,
    provider: Id,
) -> Result<Vec<ProviderModelRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT model_name, display_name, parameters, groups, enabled FROM provider_models
          WHERE provider_id = $1 ORDER BY model_name",
    )
    .bind(provider)
    .fetch_all(conn)
    .await?)
}

/// The groups that the managers of each provider put each of the models in `pairs` (a provider, a routing
/// name) in. A pair that has no entry is not among the answer; one whose entry is in no group has an empty
/// list. A list of models names `@<group>` as the models whose entry at the provider concerned is in it.
pub async fn groups_of(
    conn: &mut Conn,
    pairs: &[(Id, String)],
) -> Result<std::collections::HashMap<(Id, String), Vec<String>>, DbError> {
    if pairs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let providers: Vec<Id> = pairs.iter().map(|pair| pair.0).collect();
    let names: Vec<String> = pairs.iter().map(|pair| pair.1.clone()).collect();
    let found: Vec<(Id, String, Vec<String>)> = sqlx::query_as(
        "SELECT pm.provider_id, pm.model_name, pm.groups
           FROM provider_models pm
           JOIN UNNEST($1::uuid[], $2::text[]) AS wanted(provider_id, model_name)
             ON pm.provider_id = wanted.provider_id AND pm.model_name = wanted.model_name",
    )
    .bind(providers)
    .bind(names)
    .fetch_all(conn)
    .await?;
    Ok(found
        .into_iter()
        .map(|(provider, model, groups)| ((provider, model), groups))
        .collect())
}

// ---- What the HTTP routes of resources read and write ---------------------------------------------
//
// One block, kept together at the end of the file: the rows the requests of machines, profiles,
// providers, hosts and templates list, read and change. Nothing here decides anything.

/// What the routes of resources read and write.
pub mod routes {
    use super::MachineRow;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    // ---- Machines ---------------------------------------------------------------------------------

    /// The machines of `owner` that are not deleted, a person's own machines in the order they were
    /// enrolled. A host's machine belongs to no person and is not among them.
    pub async fn machines_of(conn: &mut Conn, owner: Id) -> Result<Vec<MachineRow>, DbError> {
        Ok(sqlx::query_as(concat!(
            "SELECT ",
            machine_columns!(),
            " FROM machines WHERE owner_id = $1 AND deleted_at IS NULL ORDER BY created_at, id"
        ))
        .bind(owner)
        .fetch_all(conn)
        .await?)
    }

    /// Physical machines considered for resource-page visibility; internal unit connectors are excluded.
    pub async fn physical_machines(conn: &mut Conn) -> Result<Vec<MachineRow>, DbError> {
        Ok(sqlx::query_as(concat!(
            "SELECT ",
            machine_columns!(),
            " FROM machines WHERE runtime_id IS NULL AND deleted_at IS NULL ORDER BY created_at,id"
        ))
        .fetch_all(conn)
        .await?)
    }

    /// Just the physical machines selected by a bounded inventory page.
    pub async fn physical_machines_among(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<MachineRow>, DbError> {
        Ok(sqlx::query_as(concat!("SELECT ", machine_columns!(), " FROM machines WHERE id=ANY($1) AND runtime_id IS NULL AND deleted_at IS NULL ORDER BY id"))
            .bind(ids).fetch_all(conn).await?)
    }

    /// Gives a machine another name. The row's `updated_at` stays: it dates what the machine last
    /// reported, and a new name is nothing it reported.
    pub async fn rename_machine(conn: &mut Conn, id: Id, name: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE machines SET name = $2 WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .bind(name)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Give the physical machine's permission page its enrollment home without changing its roles.
    pub async fn machine_home(conn: &mut Conn, id: Id, home: Option<Id>) -> Result<(), DbError> {
        sqlx::query("UPDATE machines SET home_id=$2,org_id=(SELECT path[1] FROM nodes WHERE id=$2) WHERE id=$1")
            .bind(id).bind(home).execute(conn).await?;
        Ok(())
    }

    /// Stores the local policy a machine reported back after a change from Orch, as the policy it
    /// now reports.
    pub async fn set_machine_policy(
        conn: &mut Conn,
        id: Id,
        policy: &Json,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE machines SET facts = jsonb_set(facts, '{policy}', $2::jsonb, true), updated_at = $3
              WHERE id = $1",
        )
        .bind(id)
        .bind(policy)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    // ---- References to resources ---------------------------------------------------------------

    /// A machine as another thing points at it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct MachineNameRow {
        /// Its identifier.
        pub id: Id,
        /// What it is called.
        pub name: String,
        /// Whether it had a session when it was last known.
        pub online: bool,
        /// Its operating system.
        pub platform_os: String,
        /// `connected` or `managed`.
        pub placement: String,
    }

    /// The machines `ids` that are not deleted, in one statement.
    pub async fn machine_names(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<MachineNameRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, online, platform_os, placement FROM machines
              WHERE id = ANY($1) AND deleted_at IS NULL",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// A provider as another thing points at it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct ProviderRefRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// `subscription` or `api`.
        pub kind: String,
        /// For a subscription, the harness it is a login of.
        pub harness_kind: Option<String>,
        /// What the harness called itself in the last descriptor the profile it is a login of reported.
        pub harness_name: Option<String>,
        /// `managed` or `connected`.
        pub placement: String,
        /// `needs_setup`, `ready`, `exhausted`, `error`, `offline` or `disabled`.
        pub status: String,
        /// The machine registered from or run on.
        pub machine_id: Option<Id>,
    }

    /// The providers that are not deleted and are among `ids` or run on one of `machines`, in the
    /// order they were made.
    pub async fn providers_among(
        conn: &mut Conn,
        ids: &[Id],
        machines: &[Id],
    ) -> Result<Vec<ProviderRefRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, kind, harness_kind,
                    (SELECT a.descriptor #>> '{harness,name}' FROM agents a WHERE a.id = providers.agent_id) AS harness_name,
                    provider_placement(machine_id) AS placement, status, machine_id FROM providers
              WHERE deleted_at IS NULL AND (id = ANY($1) OR machine_id = ANY($2))
              ORDER BY created_at, id",
        )
        .bind(ids)
        .bind(machines)
        .fetch_all(conn)
        .await?)
    }

    /// An agent profile as another thing points at it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct AgentRefRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// The harness it runs.
        pub harness_kind: String,
        /// What its harness called itself in the last descriptor it reported.
        pub harness_name: Option<String>,
        /// The machine its harness is on.
        pub machine_id: Option<Id>,
        /// Whether it is switched off.
        pub disabled: bool,
    }

    /// The profiles that are not deleted and are among `ids` or belong to one of `machines`, in the
    /// order they were made.
    pub async fn agents_among(
        conn: &mut Conn,
        ids: &[Id],
        machines: &[Id],
    ) -> Result<Vec<AgentRefRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, harness_kind, descriptor #>> '{harness,name}' AS harness_name, machine_id, disabled FROM agents
              WHERE deleted_at IS NULL AND (id = ANY($1) OR machine_id = ANY($2))
              ORDER BY created_at, id",
        )
        .bind(ids)
        .bind(machines)
        .fetch_all(conn)
        .await?)
    }

    /// An environment as another thing points at it.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct EnvironmentRefRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where it stands.
        pub status: String,
        /// The machine it is a directory of.
        pub machine_id: Option<Id>,
    }

    /// The environments that are not deleted and are among `ids` or registered from one of
    /// `machines`, in the order they were made.
    pub async fn environments_among(
        conn: &mut Conn,
        ids: &[Id],
        machines: &[Id],
    ) -> Result<Vec<EnvironmentRefRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, owner_id, placement, status, machine_id FROM environments
              WHERE deleted_at IS NULL AND (id = ANY($1) OR machine_id = ANY($2))
              ORDER BY created_at, id",
        )
        .bind(ids)
        .bind(machines)
        .fetch_all(conn)
        .await?)
    }

    // ---- Agent profiles -----------------------------------------------------------------------

    /// An agent profile, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct AgentRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner; none for a profile of the system.
        pub owner_id: Option<Id>,
        /// The root org of its home; none for a profile of the system.
        pub org_id: Option<Id>,
        /// The org it belongs to; none for a profile of the system.
        pub home_id: Option<Id>,
        /// The harness it runs.
        pub harness_kind: String,
        /// The machine its harness is installed on.
        pub machine_id: Option<Id>,
        /// Which build to run, and how it is configured.
        pub profile: Json,
        /// What a harness started from it last declared.
        pub descriptor: Json,
        /// What the system sets over that.
        pub descriptor_overrides: Json,
        /// Whether it is switched off.
        pub disabled: bool,
        /// The revision a change names.
        pub revision: i64,
        /// When it was made.
        pub created_at: DateTime<Utc>,
        /// When it was last changed.
        pub updated_at: DateTime<Utc>,
    }

    const AGENT_COLUMNS: &str =
        "id, name, owner_id, org_id, home_id, harness_kind, machine_id, profile, descriptor,
        descriptor_overrides, disabled, revision, created_at, updated_at";

    /// The profiles `ids` that are not deleted, in the order they were made.
    pub async fn agents(conn: &mut Conn, ids: &[Id]) -> Result<Vec<AgentRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {AGENT_COLUMNS} FROM agents WHERE id = ANY($1) AND deleted_at IS NULL
              ORDER BY created_at, id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// A profile to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewAgent<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its first owner.
        pub owner_id: Id,
        /// The org it belongs to.
        pub home_id: Id,
        /// The harness it runs.
        pub harness_kind: &'a str,
        /// The machine its harness is installed on, for one that belongs to a person's machine.
        pub machine_id: Option<Id>,
        /// Which build to run, and how.
        pub profile: &'a Json,
    }

    /// Makes a profile at home in an org; its root org is read from the hierarchy. Returns whether
    /// the home is there.
    pub async fn insert_agent(
        conn: &mut Conn,
        agent: &NewAgent<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let made = sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, machine_id, profile,
                                 created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, n.id, $5, $6, $7, $8, $8 FROM nodes n
              WHERE n.id = $4 AND n.deleted_at IS NULL",
        )
        .bind(agent.id)
        .bind(agent.name)
        .bind(agent.owner_id)
        .bind(agent.home_id)
        .bind(agent.harness_kind)
        .bind(agent.machine_id)
        .bind(agent.profile)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(made > 0)
    }

    /// What a change of a profile sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct AgentChange<'a> {
        /// A new name.
        pub name: Option<&'a str>,
        /// A new profile.
        pub profile: Option<&'a Json>,
        /// New overrides of the descriptor.
        pub descriptor_overrides: Option<&'a Json>,
        /// Switched off or on.
        pub disabled: Option<bool>,
    }

    /// Changes a profile that is at revision `expected`. Returns whether it was: nothing changes
    /// when the profile was changed meanwhile, or is gone.
    pub async fn change_agent(
        conn: &mut Conn,
        id: Id,
        expected: i64,
        change: &AgentChange<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE agents SET name = COALESCE($3, name), profile = COALESCE($4, profile),
                    descriptor_overrides = COALESCE($5, descriptor_overrides),
                    disabled = COALESCE($6, disabled), revision = revision + 1, updated_at = $7
              WHERE id = $1 AND revision = $2 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(expected)
        .bind(change.name)
        .bind(change.profile)
        .bind(change.descriptor_overrides)
        .bind(change.disabled)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Deletes a profile: it stays as a record, with its page, until it is purged.
    pub async fn delete_agent(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE agents SET deleted_at = $2, revision = revision + 1, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// What a profile is still used by.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
    pub struct AgentUse {
        /// The threads bound to it that are not deleted.
        pub threads: i64,
        /// The providers that are signed in through it and not deleted.
        pub providers: i64,
    }

    /// What the profile `id` is still used by.
    pub async fn agent_use(conn: &mut Conn, id: Id) -> Result<AgentUse, DbError> {
        Ok(sqlx::query_as(
            "SELECT (SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
                      WHERE t.agent_id = $1 AND n.deleted_at IS NULL) AS threads,
                    (SELECT count(*) FROM providers WHERE agent_id = $1 AND deleted_at IS NULL) AS providers",
        )
        .bind(id)
        .fetch_one(conn)
        .await?)
    }

    // ---- Homes ------------------------------------------------------------------------------------

    /// Of the nodes `homes`, those that are the org `org` or lie inside it: what a list narrowed to
    /// an org keeps.
    pub async fn homes_under(conn: &mut Conn, org: Id, homes: &[Id]) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM nodes WHERE id = ANY($1) AND $2 = ANY(path) AND deleted_at IS NULL",
        )
        .bind(homes)
        .bind(org)
        .fetch_all(conn)
        .await?)
    }

    /// How many people are in the Owners row of each of the resources `ids`: a person counts while
    /// their entry stands.
    pub async fn owner_counts(
        conn: &mut Conn,
        ids: &[Id],
        now: DateTime<Utc>,
    ) -> Result<Vec<(Id, i64)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT object_id, count(*) FROM permission_entries
              WHERE object_id = ANY($1) AND page = 'live' AND permission = 'access.own'
                AND removed_at IS NULL AND (expires_at IS NULL OR expires_at > $2)
              GROUP BY object_id",
        )
        .bind(ids)
        .bind(now)
        .fetch_all(conn)
        .await?)
    }

    // ---- Providers ----------------------------------------------------------------------------

    /// A provider, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct ProviderRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// The org it belongs to.
        pub home_id: Id,
        /// `subscription` or `api`.
        pub kind: String,
        /// For a subscription, the harness it is a login of.
        pub harness_kind: Option<String>,
        /// For a subscription, the profile whose harness holds the login.
        pub agent_id: Option<Id>,
        /// For an API provider, its endpoints.
        pub upstreams: Json,
        /// What its adapter's options allow.
        pub config: Json,
        /// What it can serve.
        pub protocols: Vec<String>,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where its runtime is.
        pub machine_id: Option<Id>,
        /// The machine that keeps the key.
        pub key_machine_id: Option<Id>,
        /// The types its owner allows.
        pub arrangements: Vec<String>,
        /// The environment its credential is with.
        pub held_by_environment_id: Option<Id>,
        /// Since when.
        pub held_since: Option<DateTime<Utc>>,
        /// Its runtime's own values.
        pub runtime_config: Json,
        /// Where it stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// The lost or explicitly removed runtime awaits a reviewed replacement.
        pub runtime_unavailable: Option<String>,
        /// The selected facts/models sources, learned descriptors and safe per-element errors.
        #[sqlx(default)]
        pub sources: Json,
        /// What its harness or endpoint reported.
        pub facts: Json,
        /// What was written in by hand over that.
        pub facts_overrides: Json,
        /// When the facts were taken.
        pub facts_at: Option<DateTime<Utc>>,
        /// The revision a change names.
        pub revision: i64,
        /// When it was made.
        pub created_at: DateTime<Utc>,
        /// When it was last changed.
        pub updated_at: DateTime<Utc>,
    }

    const PROVIDER_COLUMNS: &str = "id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id, upstreams,
        config, protocols, provider_placement(machine_id) AS placement, machine_id, key_machine_id, arrangements, held_by_environment_id, held_since,
        runtime_config, status, status_detail, runtime_unavailable, sources, facts, facts_overrides, facts_at, revision, created_at, updated_at";

    /// The providers `ids` that are not deleted, in the order they were made.
    pub async fn providers(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ProviderRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {PROVIDER_COLUMNS} FROM providers WHERE id = ANY($1) AND deleted_at IS NULL
              ORDER BY created_at, id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// An API provider to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewProvider<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its first owner.
        pub owner_id: Id,
        /// Its home: an org.
        pub home_id: Id,
        /// Its endpoints.
        pub upstreams: &'a Json,
        /// What it can serve.
        pub protocols: &'a [String],
        /// Its selected directory runtime; none requests a unit from its template.
        pub machine_id: Option<Id>,
        /// The machine that keeps the key.
        pub key_machine_id: Option<Id>,
        /// The types its owner allows.
        pub arrangements: &'a [String],
        /// Its runtime's own values.
        pub runtime_config: &'a Json,
        /// Where it stands to begin with.
        pub status: &'a str,
    }

    /// Makes an API provider at home in an org; its root org is read from the hierarchy. Returns
    /// whether the home is there.
    pub async fn insert_provider(
        conn: &mut Conn,
        provider: &NewProvider<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let made = sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, protocols,
                                    machine_id, key_machine_id, arrangements, runtime_config, status,
                                    created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, n.id, 'api', $5, $6, $7, $8, $9, $10, $11, $12, $12
               FROM nodes n WHERE n.id = $4 AND n.deleted_at IS NULL",
        )
        .bind(provider.id)
        .bind(provider.name)
        .bind(provider.owner_id)
        .bind(provider.home_id)
        .bind(provider.upstreams)
        .bind(provider.protocols)
        .bind(provider.machine_id)
        .bind(provider.key_machine_id)
        .bind(provider.arrangements)
        .bind(provider.runtime_config)
        .bind(provider.status)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(made > 0)
    }

    /// What a change of a provider sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct ProviderChange<'a> {
        /// A new name.
        pub name: Option<&'a str>,
        /// New endpoints, with the protocols they serve.
        pub upstreams: Option<(&'a Json, &'a [String])>,
        /// New options of its adapter.
        pub config: Option<&'a Json>,
        /// New arrangement types.
        pub arrangements: Option<&'a [String]>,
        /// New values of its runtime.
        pub runtime_config: Option<&'a Json>,
        /// New facts written in by hand.
        pub facts_overrides: Option<&'a Json>,
    }

    /// Changes a provider that is at revision `expected`. Returns whether it was.
    pub async fn change_provider(
        conn: &mut Conn,
        id: Id,
        expected: i64,
        change: &ProviderChange<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let (upstreams, protocols) = match change.upstreams {
            Some((upstreams, protocols)) => (Some(upstreams), Some(protocols)),
            None => (None, None),
        };
        let changed = sqlx::query(
            "UPDATE providers SET name = COALESCE($3, name), upstreams = COALESCE($4, upstreams),
                    protocols = COALESCE($5, protocols), config = COALESCE($6, config),
                    arrangements = COALESCE($7, arrangements), runtime_config = COALESCE($8, runtime_config),
                    facts_overrides = COALESCE($9, facts_overrides), revision = revision + 1, updated_at = $10
              WHERE id = $1 AND revision = $2 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(expected)
        .bind(change.name)
        .bind(upstreams)
        .bind(protocols)
        .bind(change.config)
        .bind(change.arrangements)
        .bind(change.runtime_config)
        .bind(change.facts_overrides)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Sets where a provider stands, with a sentence about it, and counts that as a change.
    pub async fn set_provider_standing(
        conn: &mut Conn,
        id: Id,
        status: &str,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE providers SET status = $2, status_detail = $3, revision = revision + 1, updated_at = $4
              WHERE id = $1 AND deleted_at IS NULL AND (status <> $2 OR status_detail IS DISTINCT FROM $3)",
        )
        .bind(id)
        .bind(status)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Deletes a provider, disabled: it stays as a record, with its page, until it is purged.
    pub async fn delete_provider(
        conn: &mut Conn,
        id: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE providers SET deleted_at = $2, status = 'disabled', login_pending=NULL, login_pending_at=NULL, login_state=NULL, revision = revision + 1, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// How many threads that are not deleted are bound to a provider, and how many of them run.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
    pub struct ProviderUse {
        /// Bound to it.
        pub threads: i64,
        /// Running a turn on it.
        pub running: i64,
    }

    /// What the provider `id` is used by.
    pub async fn provider_use(conn: &mut Conn, id: Id) -> Result<ProviderUse, DbError> {
        Ok(sqlx::query_as(
            "SELECT count(*) AS threads, count(*) FILTER (WHERE t.state = 'running') AS running
               FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.provider_id = $1 AND n.deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(conn)
        .await?)
    }

    /// How many threads run on each of the providers `ids`.
    pub async fn running_on(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, i64)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.provider_id, count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.provider_id = ANY($1) AND t.state = 'running' AND n.deleted_at IS NULL
              GROUP BY t.provider_id",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The names of the secrets that providers `ids` have, never their values: `api_key`, and
    /// `header:<name>` for a secret header.
    pub async fn provider_secret_names(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<(Id, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT owner_id, name FROM secrets WHERE owner_kind = 'provider' AND owner_id = ANY($1)
              ORDER BY owner_id, name",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// Takes away every secret of a provider whose name is not in `keep`.
    pub async fn delete_provider_secrets_except(
        conn: &mut Conn,
        id: Id,
        keep: &[String],
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "DELETE FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1 AND NOT (name = ANY($2))",
        )
        .bind(id)
        .bind(keep)
        .execute(conn)
        .await?
        .rows_affected())
    }

    /// A harness process started for a provider, with the profile and the machine.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct HarnessProcessRow {
        /// The provider.
        pub provider_id: Id,
        /// The profile it runs.
        pub agent_id: Id,
        /// The machine it runs on.
        pub machine_id: Id,
        /// Where it stands.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// Its version.
        pub harness_version: Option<String>,
        /// How many sessions it holds open.
        pub sessions_open: i32,
        /// When it last did something.
        pub last_active_at: Option<DateTime<Utc>>,
        /// The latest reported program failure survives subsequent online reports.
        pub last_error_at: Option<DateTime<Utc>>,
        /// Its diagnostic reason, where the program provided one.
        pub last_error_detail: Option<String>,
    }

    /// The harness processes of the providers `ids`.
    pub async fn harness_processes(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<HarnessProcessRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT provider_id, agent_id, machine_id, status, status_detail, harness_version, sessions_open,
                    last_active_at, last_error_at, last_error_detail
               FROM harnesses WHERE provider_id = ANY($1) ORDER BY provider_id, agent_id, machine_id",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    // ---- Models of a provider -----------------------------------------------------------------

    /// A model of a provider, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct ModelRow {
        /// The entry's identifier.
        pub id: Id,
        /// The provider.
        pub provider_id: Id,
        /// The name that is sent.
        pub model_name: String,
        /// The name that is shown; empty when the routing name is.
        pub display_name: String,
        /// What it can be run with here.
        pub parameters: Json,
        /// The groups its managers put it in.
        pub groups: Vec<String>,
        /// Whether it may be chosen.
        pub enabled: bool,
        /// `manual` or `suggested`.
        pub source: String,
    }

    const MODEL_COLUMNS: &str =
        "id, provider_id, model_name, display_name, parameters, groups, enabled, source";

    /// The models of the providers `ids`, in the order they were added.
    pub async fn models_of(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ModelRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {MODEL_COLUMNS} FROM provider_models WHERE provider_id = ANY($1)
              ORDER BY provider_id, created_at, id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The model `model` of the provider `provider`.
    pub async fn model_of(
        conn: &mut Conn,
        provider: Id,
        model: Id,
    ) -> Result<Option<ModelRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {MODEL_COLUMNS} FROM provider_models WHERE provider_id = $1 AND id = $2"
        )))
        .bind(provider)
        .bind(model)
        .fetch_optional(conn)
        .await?)
    }

    /// A model to add.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewModel<'a> {
        /// The entry's identifier.
        pub id: Id,
        /// The provider.
        pub provider_id: Id,
        /// The name that is sent.
        pub model_name: &'a str,
        /// The name that is shown.
        pub display_name: &'a str,
        /// What it can be run with here.
        pub parameters: &'a Json,
        /// The groups its managers put it in.
        pub groups: &'a [String],
        /// `manual` or `suggested`.
        pub source: &'a str,
        /// Who added it.
        pub created_by: Id,
    }

    /// Adds a model, enabled. A clash of the routing name or the display name is a
    /// [`DbError::Unique`] naming the constraint.
    pub async fn insert_model(
        conn: &mut Conn,
        model: &NewModel<'_>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, groups, enabled,
                                          source, created_by, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $9, true, $6, $7, $8, $8)",
        )
        .bind(model.id)
        .bind(model.provider_id)
        .bind(model.model_name)
        .bind(model.display_name)
        .bind(model.parameters)
        .bind(model.source)
        .bind(model.created_by)
        .bind(at)
        .bind(model.groups)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// What a change of a model sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct ModelChange<'a> {
        /// A new display name.
        pub display_name: Option<&'a str>,
        /// New parameters.
        pub parameters: Option<&'a Json>,
        /// The groups it is in now, as a whole.
        pub groups: Option<&'a [String]>,
        /// Enabled or not.
        pub enabled: Option<bool>,
    }

    /// Changes a model of a provider.
    pub async fn change_model(
        conn: &mut Conn,
        provider: Id,
        model: Id,
        change: &ModelChange<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE provider_models SET display_name = COALESCE($3, display_name),
                    parameters = COALESCE($4, parameters), enabled = COALESCE($5, enabled), updated_at = $6,
                    groups = COALESCE($7, groups)
              WHERE provider_id = $1 AND id = $2",
        )
        .bind(provider)
        .bind(model)
        .bind(change.display_name)
        .bind(change.parameters)
        .bind(change.enabled)
        .bind(at)
        .bind(change.groups)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Removes a model from a provider.
    pub async fn delete_model(conn: &mut Conn, provider: Id, model: Id) -> Result<bool, DbError> {
        Ok(
            sqlx::query("DELETE FROM provider_models WHERE provider_id = $1 AND id = $2")
                .bind(provider)
                .bind(model)
                .execute(conn)
                .await?
                .rows_affected()
                > 0,
        )
    }

    // ---- Templates a provider's runtime may be made from -----------------------------------------

    /// A template of the pool of providers' runtimes.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct RuntimeTemplateRow {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// The driver that makes units from it.
        pub driver: String,
        /// Its first owner; none for the system's.
        pub owner_id: Option<Id>,
    }

    /// Of the templates `ids`, those that make the runtimes of providers and are enabled.
    pub async fn runtime_templates(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<RuntimeTemplateRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, driver, owner_id FROM env_templates
              WHERE id = ANY($1) AND kind = 'provider' AND enabled AND deleted_at IS NULL
              ORDER BY created_at, id",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    // ---- Templates ------------------------------------------------------------------------------

    /// A template, as stored, with how many units were made from it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct TemplateRow {
        /// Its identifier.
        pub id: Id,
        /// The root org of its home; none for a template of the system.
        pub org_id: Option<Id>,
        /// Its home; none for a template of the system.
        pub home_id: Option<Id>,
        /// Its first owner.
        pub owner_id: Option<Id>,
        /// Its name.
        pub name: String,
        /// What it is for.
        pub description: String,
        /// `environment` or `provider`.
        pub kind: String,
        /// The driver that reads its spec.
        pub driver: String,
        /// The recipe.
        pub spec: Json,
        /// Whether units may be made from it.
        pub enabled: bool,
        /// The revision a change names.
        pub revision: i64,
        /// How many units were made from it and still stand: environments for a template of
        /// environments, the runtimes of providers that are not retired for one of providers.
        pub units: i64,
    }

    /// The templates `ids` that are not deleted, in the order they were made.
    pub async fn templates(conn: &mut Conn, ids: &[Id]) -> Result<Vec<TemplateRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT t.id, t.org_id, t.home_id, t.owner_id, t.name, t.description, t.kind, t.driver, t.spec,
                    t.enabled, t.revision,
                    CASE t.kind
                      WHEN 'environment' THEN
                        (SELECT count(*) FROM environments e WHERE e.template_id = t.id AND e.deleted_at IS NULL)
                      ELSE
                        (SELECT count(*) FROM runtimes r WHERE r.template_id = t.id AND r.kind = 'provider'
                            AND r.state NOT IN ('retired', 'removed'))
                    END AS units
               FROM env_templates t WHERE t.id = ANY($1) AND t.deleted_at IS NULL
              ORDER BY t.created_at, t.id",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// A template to make.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewTemplate<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// What it is for.
        pub description: &'a str,
        /// Its first owner.
        pub owner_id: Id,
        /// Its home: an org; none for the system's.
        pub home_id: Option<Id>,
        /// `environment` or `provider`.
        pub kind: &'a str,
        /// The driver that reads its spec.
        pub driver: &'a str,
        /// The recipe.
        pub spec: &'a Json,
    }

    /// Makes a template; the root org of its home is read from the hierarchy. Returns whether the
    /// home is there.
    pub async fn insert_template(
        conn: &mut Conn,
        template: &NewTemplate<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let made = sqlx::query(
            "INSERT INTO env_templates (id, org_id, home_id, owner_id, name, description, kind, driver, spec,
                                        created_at, updated_at)
             SELECT $1, n.org_id, n.id, $3, $4, $5, $6, $7, $8, $9, $9
               FROM (SELECT $2::uuid AS home) h LEFT JOIN nodes n ON n.id = h.home AND n.deleted_at IS NULL
              WHERE h.home IS NULL OR n.id IS NOT NULL",
        )
        .bind(template.id)
        .bind(template.home_id)
        .bind(template.owner_id)
        .bind(template.name)
        .bind(template.description)
        .bind(template.kind)
        .bind(template.driver)
        .bind(template.spec)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(made > 0)
    }

    /// What a change of a template sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct TemplateChange<'a> {
        /// A new name.
        pub name: Option<&'a str>,
        /// A new description.
        pub description: Option<&'a str>,
        /// A new recipe.
        pub spec: Option<&'a Json>,
        /// Enabled or not.
        pub enabled: Option<bool>,
    }

    /// Changes a template that is at revision `expected`. Returns whether it was.
    pub async fn change_template(
        conn: &mut Conn,
        id: Id,
        expected: i64,
        change: &TemplateChange<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE env_templates SET name = COALESCE($3, name), description = COALESCE($4, description),
                    spec = COALESCE($5, spec), enabled = COALESCE($6, enabled), revision = revision + 1,
                    updated_at = $7
              WHERE id = $1 AND revision = $2 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(expected)
        .bind(change.name)
        .bind(change.description)
        .bind(change.spec)
        .bind(change.enabled)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Deletes a template: it stays as a record, with its page, until it is purged.
    pub async fn delete_template(
        conn: &mut Conn,
        id: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE env_templates SET deleted_at = $2, revision = revision + 1, updated_at = $2
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    // ---- Hosts ----------------------------------------------------------------------------------

    /// A host, as stored, with the units that run there.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct HostRow {
        /// The last measured load, distinct from resource declarations.
        pub load: Json,
        /// When that load was observed.
        pub load_at: Option<DateTime<Utc>>,
        /// Existing stopped environment units.
        pub stopped_environments: i64,
        /// Its identifier.
        pub id: Id,
        /// The machine that runs its node.
        pub machine_id: Id,
        /// Its machine's recorded offline transition, without guessing from last activity.
        pub offline_since: Option<DateTime<Utc>>,
        /// When its physical machine was explicitly declared lost.
        pub lost_at: Option<DateTime<Utc>>,
        /// Its first owner; none for the system's.
        pub owner_id: Option<Id>,
        /// The root org of its home; none for the system's.
        pub org_id: Option<Id>,
        /// Its home; none for the system's.
        pub home_id: Option<Id>,
        /// Its name.
        pub name: String,
        /// Its labels.
        pub labels: Vec<String>,
        /// The drivers it reports, by name.
        pub drivers: Json,
        /// What it reports that it has.
        pub capacity: Json,
        /// Its named storages, as it reports them.
        pub storages: Json,
        /// Whether it takes new environments.
        pub schedulable: bool,
        /// Its own answer to being full: `warn` or `block`; none follows the system.
        pub when_full: Option<String>,
        /// Its saved declaration of host ports, as `HostPorts`; none until a manager saves one.
        pub ports: Option<Json>,
        /// The revision of that declaration its node acknowledged.
        pub ports_revision: i64,
        /// The stored counter advanced by configuration and ownership changes.
        pub revision: i64,
        /// How many runtimes of providers run there.
        pub provider_units: i64,
        /// How many environments run there.
        pub environment_units: i64,
        /// How many of those environments are the caller's.
        pub mine: i64,
        /// What the environments that run there declare they need: `cpu_millis`, `memory_mb` and
        /// `disk_gb`, each summed over the values of the environments themselves.
        pub declared: Json,
    }

    /// The hosts `ids` whose machine is not removed, in the order they were enrolled, with what runs
    /// on each; `caller` is who the count of their own environments is for.
    pub async fn hosts(conn: &mut Conn, ids: &[Id], caller: Id) -> Result<Vec<HostRow>, DbError> {
        let mut rows:Vec<HostRow>=sqlx::query_as(
            "SELECT h.id, h.machine_id, m.offline_since, m.lost_at, h.owner_id, h.org_id, h.home_id, h.name, h.labels, h.drivers, h.capacity,
                    h.load, h.load_at,
                    COALESCE(u.stopped_environments, 0) AS stopped_environments,
                    h.storages, (h.schedulable AND NOT h.disabled) AS schedulable, h.when_full, h.ports, h.ports_revision,
                    h.revision,
                    COALESCE(u.providers, 0) AS provider_units,
                    COALESCE(u.environments, 0) AS environment_units,
                    COALESCE(u.mine, 0) AS mine,
                    jsonb_build_object('cpu_millis', COALESCE(u.cpu_millis, 0), 'memory_mb', COALESCE(u.memory_mb, 0),
                                       'disk_gb', COALESCE(u.disk_gb, 0)) AS declared
               FROM hosts h
               JOIN machines m ON m.id = h.machine_id AND m.deleted_at IS NULL
               LEFT JOIN (
                 SELECT r.host_id,
                        count(*) FILTER (WHERE r.kind = 'provider' AND r.state IN ('creating', 'running')) AS providers,
                        count(*) FILTER (WHERE r.kind = 'environment' AND r.state IN ('creating', 'running')) AS environments,
                        count(*) FILTER (WHERE r.kind = 'environment' AND r.state = 'stopped') AS stopped_environments,
                        count(*) FILTER (WHERE e.owner_id = $2) AS mine,
                        COALESCE(sum(CASE WHEN r.state IN ('creating', 'running') AND e.config -> 'env.needs' ->> 'cpu_millis' ~ '^[0-9]{1,15}$'
                                          THEN (e.config -> 'env.needs' ->> 'cpu_millis')::bigint END), 0)::bigint
                          AS cpu_millis,
                        COALESCE(sum(CASE WHEN r.state IN ('creating', 'running') AND e.config -> 'env.needs' ->> 'memory_mb' ~ '^[0-9]{1,15}$'
                                          THEN (e.config -> 'env.needs' ->> 'memory_mb')::bigint END), 0)::bigint
                          AS memory_mb,
                        COALESCE(sum(CASE WHEN r.state IN ('creating', 'running') AND e.config -> 'env.needs' ->> 'disk_gb' ~ '^[0-9]{1,15}$'
                                          THEN (e.config -> 'env.needs' ->> 'disk_gb')::bigint END), 0)::bigint
                          AS disk_gb
                   FROM runtimes r LEFT JOIN environments e ON e.id = r.environment_id
                  WHERE r.host_id = ANY($1) AND r.state IN ('creating', 'running', 'stopped') AND r.successor_of IS NULL
                  GROUP BY r.host_id) u ON u.host_id = h.id
              WHERE h.id = ANY($1)
              ORDER BY h.created_at, h.id",
        )
        .bind(ids)
        .bind(caller)
        .fetch_all(&mut *conn)
        .await?;
        let mut declarations: std::collections::HashMap<_, _> =
            crate::queries::environment_facts::host_ports(conn, ids)
                .await?
                .into_iter()
                .map(|row| (row.host_id, row.publish))
                .collect();
        for row in &mut rows {
            row.ports = declarations.remove(&row.id).flatten();
        }
        Ok(rows)
    }

    /// What a change of a host sets; what is left out stays.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    pub struct HostChange<'a> {
        /// A new name.
        pub name: Option<&'a str>,
        /// New labels.
        pub labels: Option<&'a [String]>,
        /// Whether it takes new environments.
        pub schedulable: Option<bool>,
        /// Its own answer to being full (`warn` or `block`), or none to follow the system again.
        pub when_full: Option<Option<&'a str>>,
    }

    /// Changes a host that is at the revision `expected` (see [`HostRow::revision`]) and moves the
    /// revision on by one however little time has passed. Returns whether it was
    /// at that revision.
    pub async fn change_host(
        conn: &mut Conn,
        id: Id,
        expected: i64,
        change: &HostChange<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE hosts SET name = COALESCE($3, name), labels = COALESCE($4, labels),
                    schedulable = COALESCE($5, schedulable),
                    disabled = CASE WHEN $5::boolean THEN false ELSE disabled END,
                    when_full = CASE WHEN $6 THEN $7 ELSE when_full END,
                    updated_at = GREATEST($8, updated_at), revision = revision + 1
              WHERE id = $1 AND revision = $2",
        )
        .bind(id)
        .bind(expected)
        .bind(change.name)
        .bind(change.labels)
        .bind(change.schedulable)
        .bind(change.when_full.is_some())
        .bind(change.when_full.flatten())
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }

    /// Saves a host's declaration of host ports, which its node accepted at `revision`, or clears it (with
    /// `ports` none), and moves the host's revision on; at the host's revision `expected` where one is given
    /// (none when a change of the same request has just moved it). Returns whether it was saved.
    pub async fn set_host_ports(
        conn: &mut Conn,
        id: Id,
        expected: Option<i64>,
        ports: Option<&Json>,
        revision: i64,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let changed = sqlx::query(
            "UPDATE hosts SET ports = $3, ports_revision = $4,
                    updated_at = GREATEST($5, updated_at), revision = hosts.revision + 1
              WHERE id = $1
                AND ($2::bigint IS NULL OR hosts.revision = $2)",
        )
        .bind(id)
        .bind(expected)
        .bind(ports)
        .bind(revision)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(changed > 0)
    }
}

// ---- What the engine does with a provider that keeps a key -----------------------------------------
//
// One block, kept together at the end of the file: starting a provider where its key is kept, the
// references to its secrets (never their values), its status as the machine that keeps it and its
// presence move it, the providers that were switched off, and the rate catalogue. Nothing here decides
// anything.

/// What the engine reads and writes to run a provider that keeps a key.
pub mod keeper {
    use crate::queries::threads::LeaseRow;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// A provider as `provider.start` is made from it: its endpoints and where it runs, deleted or not.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct StartRow {
        /// Its identifier.
        pub id: Id,
        /// The name given to a selected credential presenter.
        pub name: String,
        /// Choices whose secret references are resolved beside the key on this start.
        pub sources: Json,
        /// `subscription` or `api`.
        pub kind: String,
        /// Its endpoints: `[{ protocol, base_url, options }]`.
        pub upstreams: Json,
        /// The protocols it serves.
        pub protocols: Vec<String>,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where its runtime is.
        pub machine_id: Option<Id>,
        /// The machine that keeps its key; none when the server holds it.
        pub key_machine_id: Option<Id>,
        /// Where it stands.
        pub status: String,
        /// Its revision.
        pub revision: i64,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The provider `id` as `provider.start` is made from it, also when it is deleted.
    pub async fn start_row(conn: &mut Conn, id: Id) -> Result<Option<StartRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, sources, kind, upstreams, protocols, provider_placement(machine_id) AS placement, machine_id, key_machine_id, status, revision,
                    deleted_at
               FROM providers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// A secret of a provider, without its value: what a command refers to.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct SecretRef {
        /// The secret's own identifier, which a command names it by.
        pub id: Id,
        /// `api_key`, or `header:<name>` for a secret header of an endpoint.
        pub name: String,
        /// When its value was last written: it changes when the value does.
        pub version: DateTime<Utc>,
    }

    /// The secrets of a provider, by name, without their values.
    pub async fn secret_refs(conn: &mut Conn, provider: Id) -> Result<Vec<SecretRef>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, name, COALESCE(rotated_at, created_at) AS version FROM secrets
              WHERE owner_kind = 'provider' AND owner_id = $1 ORDER BY name",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }

    /// Moves a provider from one of the states `from` to `to`, with a sentence about it, as a change of the
    /// provider. The answer says whether it moved: a provider in another state, or one that is switched off
    /// or deleted, is left alone.
    pub async fn move_status(
        conn: &mut Conn,
        provider: Id,
        from: &[&str],
        to: &str,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let from: Vec<String> = from.iter().map(|word| (*word).to_owned()).collect();
        let done = sqlx::query(
            "UPDATE providers SET status = $3, status_detail = $4, revision = revision + 1, updated_at = $5
              WHERE id = $1 AND deleted_at IS NULL AND status = ANY($2)
                AND (status <> $3 OR status_detail IS DISTINCT FROM $4)",
        )
        .bind(provider)
        .bind(from)
        .bind(to)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// The locked facts snapshot supplies the measurement and audit context of one observation.
    #[derive(Debug, sqlx::FromRow)]
    struct PreviousProviderFacts {
        facts: Json,
        facts_overrides: Json,
        facts_at: Option<DateTime<Utc>>,
        home_id: Id,
        org_id: Id,
    }

    /// Stores what the machine that keeps a provider reports of it. Nothing is written when it says what is
    /// stored. The answer says whether something was.
    pub async fn set_facts(
        conn: &mut Conn,
        provider: Id,
        facts: &Json,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        use sqlx::Connection;
        // A savepoint inside an existing transaction keeps the reading, its samples and its audit atomic.
        let mut tx = conn.begin().await?;
        let previous: Option<PreviousProviderFacts> = sqlx::query_as(
            "SELECT facts,facts_overrides,facts_at,home_id,org_id FROM providers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
        ).bind(provider).fetch_optional(&mut *tx).await?;
        let Some(PreviousProviderFacts {
            facts: before,
            facts_overrides: overrides,
            facts_at: previous_at,
            home_id: home,
            org_id: org,
        }) = previous
        else {
            tx.commit().await?;
            return Ok(false);
        };
        let done = sqlx::query(
            "UPDATE providers SET facts = $2, facts_at = $3, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND (facts IS DISTINCT FROM $2 OR facts_at IS DISTINCT FROM $3)",
        )
        .bind(provider)
        .bind(facts)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        let changed = done.rows_affected() == 1;
        if changed {
            crate::queries::provider_plan::record(
                &mut tx,
                provider,
                home,
                org,
                &before,
                facts,
                &overrides,
                previous_at,
                at,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }

    /// The providers that are `exhausted` and whose facts name a time for the limit, with their facts, the
    /// soonest first: the engine reads the time itself, so that one that does not read cannot stop the others
    /// from being looked at.
    pub async fn exhausted_with_a_time(
        conn: &mut Conn,
        limit: i64,
    ) -> Result<Vec<(Id, Json)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, facts FROM providers
              WHERE deleted_at IS NULL AND status = 'exhausted'
                AND (facts #>> '{limit_reached,resets_at}') IS NOT NULL
              ORDER BY (facts #>> '{limit_reached,resets_at}'), id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Which providers presence moved.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct PresenceMoves {
        /// The providers that were ready and whose machine is away: they are `offline` now.
        pub away: Vec<Id>,
        /// The providers that were `offline` and whose machine is there again: they are `ready` now.
        pub back: Vec<Id>,
    }

    /// The sentence a provider is `offline` with.
    pub const OFFLINE_DETAIL: &str = "the machine that keeps it is offline";

    /// Puts the providers on a key whose machine (the one that keeps the key, else the one its runtime is on
    /// when that is a person's machine) is away into `offline`, and brings back the ones whose machine is there
    /// again. A provider that is in another state is not touched, and neither is a subscription: its sign-in is
    /// a harness's, and what its machine's absence means for a turn is told by the check of the place the
    /// harness runs.
    pub async fn sync_presence(
        conn: &mut Conn,
        at: DateTime<Utc>,
    ) -> Result<PresenceMoves, DbError> {
        let away: Vec<Id> = sqlx::query_scalar(
            "UPDATE providers p SET status = 'offline', status_detail = $1, revision = p.revision + 1,
                    updated_at = $2
               FROM machines m
              WHERE p.kind = 'api' AND p.status = 'ready' AND p.deleted_at IS NULL
                AND m.id = p.key_machine_id AND m.placement = 'connected'
                AND NOT m.online
             RETURNING p.id",
        )
        .bind(OFFLINE_DETAIL)
        .bind(at)
        .fetch_all(&mut *conn)
        .await?;
        let back: Vec<Id> = sqlx::query_scalar(
            "UPDATE providers p SET status = 'ready', status_detail = NULL, revision = p.revision + 1,
                    updated_at = $1
               FROM machines m
              WHERE p.kind = 'api' AND p.status = 'offline' AND p.deleted_at IS NULL
                AND m.id = p.key_machine_id AND m.placement = 'connected'
                AND m.online AND m.status = 'active' AND m.deleted_at IS NULL
             RETURNING p.id",
        )
        .bind(at)
        .fetch_all(conn)
        .await?;
        Ok(PresenceMoves { away, back })
    }

    /// A provider that was switched off or deleted, and where it was started.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct Switched {
        /// Its identifier.
        pub id: Id,
        /// The machine that keeps its key, else the one its runtime is on.
        pub machine_id: Option<Id>,
        /// Its revision when it was switched off: the next change is another act.
        pub revision: i64,
    }

    /// The providers switched off or deleted after `since` that have a machine, oldest first. A provider is
    /// switched off (and a deleted one is) by a change that moves its `updated_at`.
    pub async fn switched_off_since(
        conn: &mut Conn,
        since: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Switched>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, key_machine_id AS machine_id, revision FROM providers
              WHERE status = 'disabled' AND updated_at > $1
                AND key_machine_id IS NOT NULL
              ORDER BY updated_at, id LIMIT $2",
        )
        .bind(since)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The leases of a provider that are open.
    pub async fn open_leases_of(conn: &mut Conn, provider: Id) -> Result<Vec<LeaseRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                    opened_at, closed_at, close_reason
               FROM leases WHERE provider_id = $1 AND closed_at IS NULL ORDER BY opened_at, id",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }

    /// The rate catalogue as it was last loaded: the file's document, none while none was.
    pub async fn rate_catalogue(conn: &mut Conn) -> Result<Option<Json>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT doc FROM rate_catalogue WHERE id")
                .fetch_optional(conn)
                .await?,
        )
    }
}

// ---- What putting a thread to work and registering from a machine read and write ---------------------
//
// One block, kept together at the end of the file: the harness kinds, the drivers and the capabilities
// the dialogs read, what a person's machine already has registered from it, and the subscription found
// signed in on it. Nothing here decides anything.

/// What the dialogs of a binding and the registration of what a person's machine has read and write.
pub mod binding {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// An agent profile as far as the list of harness kinds needs it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct AgentKindRow {
        /// The harness it runs.
        pub harness_kind: String,
        /// What it is called.
        pub name: String,
        /// Whether it is the profile of the system, which the adapter was seeded as.
        pub of_the_system: bool,
        /// What its harness last declared; empty while none did.
        pub descriptor: Json,
        /// When that was.
        pub updated_at: DateTime<Utc>,
    }

    /// Every agent profile that is not deleted, as the list of harness kinds is made from them.
    pub async fn agent_kinds(conn: &mut Conn) -> Result<Vec<AgentKindRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT harness_kind, name, (org_id IS NULL AND machine_id IS NULL) AS of_the_system,
                    descriptor, updated_at
               FROM agents WHERE deleted_at IS NULL ORDER BY created_at, id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The profiles of the system itself (those of no org and no machine), which name the adapters even before
    /// a person has a usable profile of their own.
    pub async fn system_agent_kinds(conn: &mut Conn) -> Result<Vec<AgentKindRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT harness_kind, name, true AS of_the_system, descriptor, updated_at
               FROM agents WHERE deleted_at IS NULL AND org_id IS NULL AND machine_id IS NULL
              ORDER BY created_at, id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The agent profiles that the list of harness kinds is made from, however many there are: for each kind the first
    /// profile, the system's first profile and the first of its that declared a descriptor, and the newest profile
    /// that declared one. Everything the list reads of the others is what these say, so the others are not read.
    pub async fn agent_kind_summaries(conn: &mut Conn) -> Result<Vec<AgentKindRow>, DbError> {
        Ok(sqlx::query_as(
            "WITH live AS (
                SELECT id, harness_kind, name, (org_id IS NULL AND machine_id IS NULL) AS of_the_system,
                       descriptor, updated_at, created_at,
                       (jsonb_typeof(descriptor) = 'object' AND descriptor <> '{}'::jsonb) AS declared
                  FROM agents WHERE deleted_at IS NULL
             ), picked AS (
                (SELECT DISTINCT ON (harness_kind) * FROM live
                  ORDER BY harness_kind, created_at, id)
                UNION
                (SELECT DISTINCT ON (harness_kind) * FROM live WHERE of_the_system
                  ORDER BY harness_kind, created_at, id)
                UNION
                (SELECT DISTINCT ON (harness_kind) * FROM live WHERE of_the_system AND declared
                  ORDER BY harness_kind, created_at, id)
                UNION
                (SELECT DISTINCT ON (harness_kind) * FROM live WHERE declared
                  ORDER BY harness_kind, updated_at DESC, created_at, id)
             )
             SELECT harness_kind, name, of_the_system, descriptor, updated_at FROM picked
              ORDER BY created_at, id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The kinds of adapter that the registered machines say they can drive, each once.
    pub async fn machine_adapter_kinds(conn: &mut Conn) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT adapter ->> 'kind'
               FROM machines m
              CROSS JOIN LATERAL jsonb_array_elements(
                    CASE WHEN jsonb_typeof(m.facts -> 'adapters') = 'array'
                         THEN m.facts -> 'adapters' ELSE '[]'::jsonb END) AS adapter
              WHERE m.deleted_at IS NULL AND m.status = 'active' AND adapter ->> 'kind' IS NOT NULL",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The builds of harnesses that the registered machines found installed, by kind and version, each once.
    pub async fn machine_harness_versions(
        conn: &mut Conn,
    ) -> Result<Vec<(String, String)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT found ->> 'kind', found ->> 'version'
               FROM machines m
              CROSS JOIN LATERAL jsonb_array_elements(
                    CASE WHEN jsonb_typeof(m.facts -> 'harnesses') = 'array'
                         THEN m.facts -> 'harnesses' ELSE '[]'::jsonb END) AS found
              WHERE m.deleted_at IS NULL AND m.status = 'active'
                AND found ->> 'kind' IS NOT NULL AND found ->> 'version' IS NOT NULL",
        )
        .fetch_all(conn)
        .await?)
    }

    /// Whether the harness of each kind says it can take over a conversation another's history was neutral-imported
    /// into (`import_neutral`), as the profile that speaks for the kind last declared it: the system's own profile
    /// of the kind, else the newest of any. One row for each kind that has declared a descriptor, and only that
    /// member of it, so what is read does not grow with the profiles' descriptors.
    pub async fn neutral_import_of_kinds(conn: &mut Conn) -> Result<Vec<(String, bool)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT ON (harness_kind) harness_kind,
                    COALESCE(descriptor #> '{capabilities,import_neutral}' = 'true'::jsonb, false)
               FROM agents
              WHERE deleted_at IS NULL AND jsonb_typeof(descriptor) = 'object' AND descriptor <> '{}'::jsonb
              ORDER BY harness_kind, (org_id IS NULL AND machine_id IS NULL) DESC, updated_at DESC,
                       created_at, id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// What the machines that are registered report of themselves: their facts, as last reported.
    pub async fn machine_facts(conn: &mut Conn) -> Result<Vec<Json>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT facts FROM machines WHERE deleted_at IS NULL AND status = 'active' ORDER BY created_at, id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The drivers each host reports, for the hosts whose machine is not removed: one object for each,
    /// by the driver's name.
    pub async fn host_drivers(conn: &mut Conn) -> Result<Vec<Json>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT h.drivers FROM hosts h JOIN machines m ON m.id = h.machine_id AND m.deleted_at IS NULL
              ORDER BY h.created_at, h.id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// A kind of capability an environment may be given.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct CapabilityRow {
        /// Its key.
        pub key: String,
        /// What it is called.
        pub label: String,
        /// What it lets an environment do.
        pub description: String,
        /// How much harm granting it could do.
        pub risk: String,
        /// What it comes to for each driver.
        pub driver_patch: Json,
        /// What happens when one is asked for and nothing else decides.
        pub default_decision: String,
        /// Whose approvers answer when the decision is to ask.
        pub default_approver_scope: String,
        /// Whether it ships with the system.
        pub builtin: bool,
        /// Its revision.
        pub revision: i64,
    }

    /// The capabilities the system knows, by key.
    pub async fn capability_defs(conn: &mut Conn) -> Result<Vec<CapabilityRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT key, label, description, risk, driver_patch, default_decision, default_approver_scope,
                    builtin, revision
               FROM capability_defs ORDER BY key",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The profile of `harness_kind` that was registered from `machine` in the org `home` for the harness's own
    /// default home, if one was. A profile pointed at another home keeps another sign-in and is never reused here.
    pub async fn machine_agent(
        conn: &mut Conn,
        machine: Id,
        home: Id,
        harness_kind: &str,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM agents
              WHERE machine_id = $1 AND home_id = $2 AND harness_kind = $3 AND deleted_at IS NULL
                AND (profile #>> '{attach,home}') IS NULL
              ORDER BY created_at, id LIMIT 1",
        )
        .bind(machine)
        .bind(home)
        .bind(harness_kind)
        .fetch_optional(conn)
        .await?)
    }

    /// The subscription that was registered from `machine` in the org `home` through the profile
    /// `agent`, if one was.
    pub async fn machine_subscription(
        conn: &mut Conn,
        machine: Id,
        home: Id,
        agent: Id,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers
              WHERE key_machine_id = $1 AND home_id = $2 AND agent_id = $3 AND kind = 'subscription'
                AND deleted_at IS NULL
              ORDER BY created_at, id LIMIT 1",
        )
        .bind(machine)
        .bind(home)
        .bind(agent)
        .fetch_optional(conn)
        .await?)
    }

    /// A subscription found signed in on a person's machine, to register.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewMachineSubscription<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its first owner: the machine's owner.
        pub owner_id: Id,
        /// The org it is registered in.
        pub home_id: Id,
        /// The harness whose login it is.
        pub harness_kind: &'a str,
        /// The profile whose harness holds the login.
        pub agent_id: Id,
        /// The machine: its runtime, and the only place it is used.
        pub machine_id: Id,
        /// The types its owner allows.
        pub arrangements: &'a [String],
        /// What the harness reported of its plan and its limits.
        pub facts: &'a Json,
    }

    /// Registers a subscription from a person's machine: connected, its runtime that machine, in the
    /// org `home_id`, whose root is read from the hierarchy. It stands `needs_setup` until it has a
    /// model. Returns whether the home is there.
    pub async fn insert_machine_subscription(
        conn: &mut Conn,
        new: &NewMachineSubscription<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let made = sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status, facts, facts_at,
                                    created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, n.id, 'subscription', $5, $6, $7, $8, 'needs_setup',
                    $9, $10, $10, $10
               FROM nodes n WHERE n.id = $4 AND n.deleted_at IS NULL",
        )
        .bind(new.id)
        .bind(new.name)
        .bind(new.owner_id)
        .bind(new.home_id)
        .bind(new.harness_kind)
        .bind(new.agent_id)
        .bind(new.machine_id)
        .bind(new.arrangements)
        .bind(new.facts)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(made > 0)
    }

    /// A profile of a person's machine, as registering a subscription through it needs it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct MachineAgent {
        /// Its identifier.
        pub id: Id,
        /// The harness it runs.
        pub harness_kind: String,
        /// The machine it belongs to.
        pub machine_id: Option<Id>,
        /// Its first owner.
        pub owner_id: Option<Id>,
        /// Whether it is switched off.
        pub disabled: bool,
    }

    /// The profile `id` unless it is deleted.
    pub async fn machine_agent_row(
        conn: &mut Conn,
        id: Id,
    ) -> Result<Option<MachineAgent>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, harness_kind, machine_id, owner_id, disabled FROM agents
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The models of a provider that are enabled, counted.
    pub async fn enabled_models(conn: &mut Conn, provider: Id) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM provider_models WHERE provider_id = $1 AND enabled",
        )
        .bind(provider)
        .fetch_one(conn)
        .await?)
    }

    /// Hands the key of a provider that was registered from a machine to the server: the machine stays
    /// where the provider runs, and no longer is where the key is kept. Returns whether there was
    /// a key on a machine.
    pub async fn release_key_machine(
        conn: &mut Conn,
        provider: Id,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let done = sqlx::query(
            "UPDATE providers SET key_machine_id = machine_id, revision = revision + 1, updated_at = $2
              WHERE id = $1 AND machine_id IS NOT NULL AND key_machine_id IS NOT NULL",
        )
        .bind(provider)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// A machine that holds a process of a harness, and how it stands.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct HarnessAt {
        /// The machine the process is on.
        pub machine_id: Id,
        /// `offline`, `starting`, `online` or `error`.
        pub status: String,
    }

    /// The machines on which the harness of the profile `agent` serves the provider `provider`, the one
    /// that was last heard of first.
    pub async fn harness_rows(
        conn: &mut Conn,
        provider: Id,
        agent: Id,
    ) -> Result<Vec<HarnessAt>, DbError> {
        Ok(sqlx::query_as(
            "SELECT machine_id, status FROM harnesses WHERE provider_id = $1 AND agent_id = $2
              ORDER BY updated_at DESC, machine_id",
        )
        .bind(provider)
        .bind(agent)
        .fetch_all(conn)
        .await?)
    }

    /// Whether the harness of `agent` serves `provider` at all: a thread was bound to the pair, or a
    /// process of it ran.
    pub async fn harness_is_known(
        conn: &mut Conn,
        provider: Id,
        agent: Id,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM harnesses WHERE provider_id = $1 AND agent_id = $2)
                 OR EXISTS (SELECT 1 FROM threads WHERE provider_id = $1 AND agent_id = $2)
                 OR EXISTS (SELECT 1 FROM providers WHERE id = $1 AND agent_id = $2)",
        )
        .bind(provider)
        .bind(agent)
        .fetch_one(conn)
        .await?)
    }
}

// ---- What making environments reads and writes of hosts, templates and enrollment -----------------
//
// (the environments part) One block, kept together at the end of the file: the hosts a unit is
// scheduled on and what they report, the template a unit is made from, and the enrollment of the
// connector inside a unit. Nothing here decides anything.

/// What the environments part reads and writes of hosts, templates and enrollment.
pub mod hosting {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// A host and its machine, as scheduling reads them.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct HostState {
        /// Its identifier.
        pub id: Id,
        /// The machine that runs its node.
        pub machine_id: Id,
        /// Who enrolled it; none for a host of the system.
        pub owner_id: Option<Id>,
        /// The root org of its home.
        pub org_id: Option<Id>,
        /// Its home.
        pub home_id: Option<Id>,
        /// Its name.
        pub name: String,
        /// Its labels.
        pub labels: Vec<String>,
        /// The drivers it reports, by name.
        pub drivers: Json,
        /// Setup schemas reported independently of the currently loaded drivers.
        pub driver_configuration_schemas: Json,
        /// What it reports that it has.
        pub capacity: Json,
        /// What it reports that it uses.
        pub load: Json,
        /// Its storages, as it reports them.
        pub storages: Json,
        /// Whether it takes new environments.
        pub schedulable: bool,
        /// A membership or home ending has switched it off, independently of taking new units.
        pub disabled: bool,
        /// Its own answer to being full; none follows the system.
        pub when_full: Option<String>,
        /// When this host was permanently declared lost.
        pub lost_at: Option<DateTime<Utc>>,
        /// Whether its node holds a session now.
        pub online: bool,
        /// When its machine was first observed offline; absent while online or never observed.
        pub offline_since: Option<DateTime<Utc>>,
        /// `active` or `revoked`.
        pub machine_status: String,
        /// Its operating system: `linux`, `macos` or `windows`.
        pub platform_os: String,
        /// Its processor architecture: `x86_64` or `aarch64`.
        pub platform_arch: String,
    }

    const HOST_COLUMNS: &str = "h.id, h.machine_id, h.owner_id, h.org_id, h.home_id, h.name, h.labels, h.drivers, h.driver_configuration_schemas,
        h.capacity, h.load, h.storages, h.schedulable, h.disabled, h.when_full, m.lost_at, m.online, m.offline_since, m.status AS machine_status,
        m.platform_os, m.platform_arch";

    /// The hosts `ids` whose machine is not deleted; every host when none are named.
    pub async fn hosts(conn: &mut Conn, ids: Option<&[Id]>) -> Result<Vec<HostState>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {HOST_COLUMNS} FROM hosts h JOIN machines m ON m.id = h.machine_id AND m.deleted_at IS NULL
              WHERE ($1::uuid[] IS NULL OR h.id = ANY($1)) ORDER BY h.created_at, h.id"
        )))
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// The host `id`.
    pub async fn host(conn: &mut Conn, id: Id) -> Result<Option<HostState>, DbError> {
        Ok(hosts(conn, Some(&[id])).await?.pop())
    }

    /// The host whose node is the machine `machine`.
    pub async fn host_of_machine(conn: &mut Conn, machine: Id) -> Result<Option<Id>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id = $1")
                .bind(machine)
                .fetch_optional(conn)
                .await?,
        )
    }

    /// Host facades of these physical machines, in one read for resource-page projection.
    pub async fn machine_hosts(
        conn: &mut Conn,
        machines: &[Id],
    ) -> Result<Vec<(Id, Id, String)>, DbError> {
        Ok(
            sqlx::query_as("SELECT machine_id,id,name FROM hosts WHERE machine_id=ANY($1)")
                .bind(machines)
                .fetch_all(conn)
                .await?,
        )
    }

    /// Records what a host's node reports it has and can make.
    pub async fn record_node_facts(
        conn: &mut Conn,
        host: Id,
        capacity: &Json,
        drivers: &Json,
        driver_configuration_schemas: &Json,
        storages: &Json,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE hosts SET capacity = $2, drivers = $3, driver_configuration_schemas = $4, storages = $5, updated_at = GREATEST(updated_at, $6)
              WHERE id = $1",
        )
        .bind(host)
        .bind(capacity)
        .bind(drivers)
        .bind(driver_configuration_schemas)
        .bind(storages)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Records what a host's node reports it uses.
    pub async fn record_node_load(
        conn: &mut Conn,
        host: Id,
        load: &Json,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE hosts SET load = $2, load_at = $3 WHERE id = $1")
            .bind(host)
            .bind(load)
            .bind(at)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// A template, as scheduling and provisioning read it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct TemplateState {
        /// Its identifier.
        pub id: Id,
        /// Its first owner; none for a system template.
        pub owner_id: Option<Id>,
        /// The root org of its home; none for a template of the system.
        pub org_id: Option<Id>,
        /// Its home.
        pub home_id: Option<Id>,
        /// Its name.
        pub name: String,
        /// `environment` or `provider`.
        pub kind: String,
        /// The driver that reads its spec.
        pub driver: String,
        /// Its spec, in the driver's schema.
        pub spec: Json,
        /// Whether it is switched on.
        pub enabled: bool,
        /// Its revision.
        pub revision: i64,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
    }

    /// The template `id`.
    pub async fn template(conn: &mut Conn, id: Id) -> Result<Option<TemplateState>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, owner_id, org_id, home_id, name, kind, driver, spec, enabled, revision, deleted_at
               FROM env_templates WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The templates a page of provider runtimes needs, including their owners, in one read.
    pub async fn templates(conn: &mut Conn, ids: &[Id]) -> Result<Vec<TemplateState>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, owner_id, org_id, home_id, name, kind, driver, spec, enabled, revision, deleted_at
               FROM env_templates WHERE id=ANY($1)",
        )
        .bind(ids)
        .fetch_all(conn)
        .await?)
    }

    /// One runtime on a host, with what it declares it needs.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct LoadRow {
        /// The host.
        pub host_id: Id,
        /// The runtime.
        pub runtime_id: Id,
        /// `environment` or `provider`.
        pub kind: String,
        /// Where it stands.
        pub state: String,
        /// The environment it is for.
        pub environment_id: Option<Id>,
        /// Only the frozen template fields used by capacity and runtime overhead.
        pub spec: Json,
        /// Only the own resource, need and runtime values used by capacity.
        pub config: Option<Json>,
        /// When something last happened in its environment.
        pub last_activity_at: Option<DateTime<Utc>>,
    }

    /// The runtimes that are somebody's current ones on the hosts `hosts`, with the template they
    /// were made from: what a host's declared sums are made of.
    pub async fn loads(conn: &mut Conn, hosts: &[Id]) -> Result<Vec<LoadRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT r.host_id, r.id AS runtime_id, r.kind, r.state, r.environment_id,
                    jsonb_strip_nulls(jsonb_build_object(
                      'needs',COALESCE(frozen.spec,t.spec)->'needs',
                      'resources',COALESCE(frozen.spec,t.spec)->'resources',
                      'resources_max',COALESCE(frozen.spec,t.spec)->'resources_max',
                      'runtime',COALESCE(frozen.spec,t.spec)->'runtime')) AS spec,
                    CASE WHEN r.kind='provider' THEN
                      jsonb_strip_nulls(jsonb_build_object(
                        'runtime.needs',p.runtime_config->'runtime.needs',
                        'runtime.resources',p.runtime_config->'runtime.resources',
                        'env.runtime',p.runtime_config->'env.runtime'))
                    ELSE jsonb_strip_nulls(jsonb_build_object(
                      'env.needs',e.config->'env.needs',
                      'env.resources',e.config->'env.resources',
                      'env.runtime',e.config->'env.runtime')) END AS config,
                    e.last_activity_at
               FROM runtimes r
               JOIN env_templates t ON t.id = r.template_id
               LEFT JOIN environments e ON e.id = r.environment_id
               LEFT JOIN providers p ON p.id = r.provider_id
               LEFT JOIN LATERAL (
                 SELECT plan->'created_spec'->'spec' AS spec FROM jobs
                  WHERE plan->>'runtime'=r.id::text AND plan->'created_spec'->'spec' IS NOT NULL
                  ORDER BY created_at DESC,id DESC LIMIT 1
               ) frozen ON true
              WHERE r.host_id = ANY($1) AND r.successor_of IS NULL
                AND (r.state IN ('creating', 'running', 'stopping', 'stopped', 'failed')
                     OR (r.state='removed' AND e.removal_phase IN ('kept','restoring','purging')
                         AND EXISTS(SELECT 1 FROM jsonb_array_elements(e.retained_volumes) v
                             WHERE v->>'runtime_id'=r.id::text AND v->>'host_id'=r.host_id::text)))
              ORDER BY r.host_id, r.created_at, r.id",
        )
        .bind(hosts)
        .fetch_all(conn)
        .await?)
    }

    /// Makes an enrollment token for the connector inside the unit of a runtime: single use, and the
    /// machine that comes of it is that runtime's. Returns whether it was stored.
    pub async fn insert_runtime_token(
        conn: &mut Conn,
        token_hash: &[u8],
        created_by: Id,
        name_hint: Option<&str>,
        runtime: Id,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let inserted = sqlx::query(
            "INSERT INTO enrollment_tokens (token_hash, created_by, purpose, runtime_id, name_hint, expires_at)
             VALUES ($1, $2, 'runtime', $3, $4, $5)
             ON CONFLICT (token_hash) DO NOTHING",
        )
        .bind(token_hash)
        .bind(created_by)
        .bind(runtime)
        .bind(name_hint)
        .bind(expires_at)
        .execute(conn)
        .await?
        .rows_affected();
        Ok(inserted > 0)
    }

    /// The runtime a token was made for.
    pub async fn runtime_of_token(
        conn: &mut Conn,
        token_hash: &[u8],
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar::<_, Option<Id>>(
            "SELECT runtime_id FROM enrollment_tokens WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(conn)
        .await?
        .flatten())
    }

    /// Makes the machine that just enrolled the runtime's: it is managed, belongs to the host the
    /// runtime is on, and the runtime knows it. Returns whether the runtime is there.
    pub async fn adopt_machine(
        conn: &mut Conn,
        machine: Id,
        runtime: Id,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let adopted =
            sqlx::query("UPDATE runtimes SET machine_id = $1, updated_at = $3 WHERE id = $2")
                .bind(machine)
                .bind(runtime)
                .bind(at)
                .execute(&mut *conn)
                .await?
                .rows_affected();
        if adopted == 0 {
            return Ok(false);
        }
        sqlx::query(
            "UPDATE machines SET host_id = (SELECT host_id FROM runtimes WHERE id = $2), runtime_id = $2,
                    updated_at = $3
              WHERE id = $1",
        )
        .bind(machine)
        .bind(runtime)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(true)
    }

    /// Revokes the machine of a runtime whose unit is gone: nothing connects with its certificate
    /// again.
    pub async fn revoke_machine_of(
        conn: &mut Conn,
        runtime: Id,
        at: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "UPDATE machines SET status = 'revoked', online = false, offline_since=COALESCE(offline_since,$2), updated_at = $2
              WHERE runtime_id = $1 AND status <> 'revoked'",
        )
        .bind(runtime)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected())
    }

    /// The machine of a runtime, if its connector enrolled, with whether it is online.
    pub async fn machine_of_runtime(
        conn: &mut Conn,
        runtime: Id,
    ) -> Result<Option<(Id, bool)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT m.id, m.online FROM runtimes r JOIN machines m ON m.id = r.machine_id
              WHERE r.id = $1 AND m.status = 'active' AND m.deleted_at IS NULL",
        )
        .bind(runtime)
        .fetch_optional(conn)
        .await?)
    }

    /// Whether each of the mounts `mounts` has a place on the machine `machine`.
    pub async fn mounts_present(
        conn: &mut Conn,
        mounts: &[Id],
        machine: Id,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT mount_id FROM mount_locations WHERE mount_id = ANY($1) AND machine_id = $2",
        )
        .bind(mounts)
        .bind(machine)
        .fetch_all(conn)
        .await?)
    }

    /// The mounts `ids` that are there, with their names.
    pub async fn mount_names(conn: &mut Conn, ids: &[Id]) -> Result<Vec<(Id, String)>, DbError> {
        Ok(
            sqlx::query_as("SELECT id, name FROM mounts WHERE id = ANY($1) AND deleted_at IS NULL")
                .bind(ids)
                .fetch_all(conn)
                .await?,
        )
    }

    /// Deletes the secret `name` of the runtime `runtime` (an enrollment token that was never used,
    /// or whose unit is gone).
    pub async fn delete_runtime_secret(
        conn: &mut Conn,
        runtime: Id,
        name: &str,
    ) -> Result<(), DbError> {
        sqlx::query(
            "DELETE FROM secrets WHERE owner_kind = 'environment' AND owner_id = $1 AND name = $2",
        )
        .bind(runtime)
        .bind(name)
        .execute(conn)
        .await?;
        Ok(())
    }
}

// ---- Management over HTTP: what the system's admins list and write of the system's own ----

/// The hosts, runtimes, capability definitions and harness builds as Management reads and writes
/// them, and the machines that report what Management shows of them.
pub mod management {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// The hosts whose machine is not removed, in the order they were enrolled.
    pub async fn host_ids(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT h.id FROM hosts h JOIN machines m ON m.id = h.machine_id AND m.deleted_at IS NULL
              ORDER BY h.created_at, h.id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// A runtime as stored, with the name of the template it was made from.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct RuntimeRow {
        /// Its identifier.
        pub id: Id,
        /// `provider` or `environment`.
        pub kind: String,
        /// The host it is on.
        pub host_id: Id,
        /// The machine it runs on, once its connector has enrolled.
        pub machine_id: Option<Id>,
        /// The driver's name for the unit.
        pub unit_name: String,
        /// The driver that made it.
        pub driver: String,
        /// The template it was made from.
        pub template_id: Id,
        /// That template's name.
        pub template_name: String,
        /// Where it stands.
        pub state: String,
        /// A sentence on that.
        pub state_detail: Option<String>,
        /// How many units it has had.
        pub generation: i32,
        /// The runtime it is being prepared to replace.
        pub successor_of: Option<Id>,
        /// For a retired one, when the reaper may remove it.
        pub remove_at: Option<DateTime<Utc>>,
        /// The environment, for an environment's runtime.
        pub environment_id: Option<Id>,
        /// The provider, for a provider's runtime.
        pub provider_id: Option<Id>,
        /// When something last happened in it.
        pub last_active_at: Option<DateTime<Utc>>,
    }

    /// The runtimes that still stand (those removed are a note for the node, not a unit), in the
    /// order they were made, those on the host `host` and of the kind `kind` where those are given.
    pub async fn runtimes(
        conn: &mut Conn,
        host: Option<Id>,
        kind: Option<&str>,
    ) -> Result<Vec<RuntimeRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT r.id, r.kind, r.host_id, r.machine_id, r.unit_name, r.driver, r.template_id,
                    t.name AS template_name, r.state, r.state_detail, r.generation, r.successor_of,
                    r.remove_at, r.environment_id, r.provider_id, r.last_active_at
               FROM runtimes r JOIN env_templates t ON t.id = r.template_id
              WHERE r.state <> 'removed' AND ($1::uuid IS NULL OR r.host_id = $1)
                AND ($2::text IS NULL OR r.kind = $2)
              ORDER BY r.created_at, r.id",
        )
        .bind(host)
        .bind(kind)
        .fetch_all(conn)
        .await?)
    }

    /// A machine that reports plugins of its own.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct PluginMachineRow {
        /// The machine.
        pub id: Id,
        /// Whether it is the machine of a host, whose node loads them.
        pub is_host: bool,
        /// What it reports: an array of `{ kind, name, version, state }`.
        pub plugins: Json,
    }

    /// The machines that stand and report at least one plugin, in the order they were enrolled.
    pub async fn machines_reporting_plugins(
        conn: &mut Conn,
    ) -> Result<Vec<PluginMachineRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT m.id, EXISTS (SELECT 1 FROM hosts h WHERE h.machine_id = m.id) AS is_host,
                    m.facts -> 'plugins' AS plugins
               FROM machines m
              WHERE m.deleted_at IS NULL AND m.status = 'active'
                AND jsonb_typeof(m.facts -> 'plugins') = 'array'
                AND jsonb_array_length(m.facts -> 'plugins') > 0
              ORDER BY m.created_at, m.id",
        )
        .fetch_all(conn)
        .await?)
    }

    /// How many active machines hold a certificate that ends at `before` or earlier, those that
    /// ended already among them: a machine whose certificate lapsed needs its owner more.
    pub async fn machines_with_certificates_ending_by(
        conn: &mut Conn,
        before: DateTime<Utc>,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM machines
              WHERE status = 'active' AND deleted_at IS NULL AND cert_not_after <= $1",
        )
        .bind(before)
        .fetch_one(conn)
        .await?)
    }

    /// A machine certificate that has lapsed or will end soon.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    pub struct MachineCertificateExpiryRow {
        /// The machine's identifier.
        pub id: Id,
        /// Until when its certificate is valid.
        pub not_after: DateTime<Utc>,
    }

    /// At most `limit` active, nondeleted machines whose certificates end by `before`, soonest
    /// first; lapsed certificates need attention too. Identifiers settle equal expiry times.
    pub async fn expiring_machine_certificates(
        conn: &mut Conn,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<MachineCertificateExpiryRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, cert_not_after AS not_after FROM machines
              WHERE status = 'active' AND deleted_at IS NULL AND cert_not_after <= $1
              ORDER BY cert_not_after, id LIMIT $2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    // ---- Capability definitions ------------------------------------------------------------

    /// The definition of a capability, as stored.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct CapabilityRow {
        /// Its key.
        pub key: String,
        /// What it is called.
        pub label: String,
        /// What it lets an environment do.
        pub description: String,
        /// `low`, `medium`, `high` or `critical`.
        pub risk: String,
        /// What it comes to for each driver.
        pub driver_patch: Json,
        /// `allow`, `ask` or `deny`.
        pub default_decision: String,
        /// `host`, `org` or `system`.
        pub default_approver_scope: String,
        /// Whether it ships with the system.
        pub builtin: bool,
        /// How many times it was changed, and one.
        pub revision: i64,
    }

    const CAPABILITY_COLUMNS: &str =
        "key, label, description, risk, driver_patch, default_decision,
        default_approver_scope, builtin, revision";

    /// Every capability definition, by key.
    pub async fn capabilities(conn: &mut Conn) -> Result<Vec<CapabilityRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {CAPABILITY_COLUMNS} FROM capability_defs ORDER BY key"
        )))
        .fetch_all(conn)
        .await?)
    }

    /// The definition of the capability `key`.
    pub async fn capability(conn: &mut Conn, key: &str) -> Result<Option<CapabilityRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {CAPABILITY_COLUMNS} FROM capability_defs WHERE key = $1"
        )))
        .bind(key)
        .fetch_optional(conn)
        .await?)
    }

    /// A definition to write.
    #[derive(Debug, Clone, Copy)]
    pub struct CapabilityWrite<'a> {
        /// Its key.
        pub key: &'a str,
        /// What it is called.
        pub label: &'a str,
        /// What it lets an environment do.
        pub description: &'a str,
        /// `low`, `medium`, `high` or `critical`.
        pub risk: &'a str,
        /// What it comes to for each driver.
        pub driver_patch: &'a Json,
        /// `allow`, `ask` or `deny`.
        pub default_decision: &'a str,
        /// `host`, `org` or `system`.
        pub default_approver_scope: &'a str,
    }

    /// What writing a definition came to.
    #[derive(Debug, Clone, PartialEq)]
    pub enum CapabilityPut {
        /// There was none; it was added.
        Added(CapabilityRow),
        /// There was one at the revision the writer saw; it was replaced.
        Replaced(CapabilityRow),
        /// There is one at another revision than the writer saw; nothing was written.
        Stale(CapabilityRow),
    }

    /// Writes a definition: adds it where there is none, and replaces it where there is one at
    /// the revision `seen`. The row is held until the transaction ends so that two writers go one
    /// after the other.
    pub async fn put_capability(
        conn: &mut Conn,
        write: &CapabilityWrite<'_>,
        seen: i64,
        at: DateTime<Utc>,
    ) -> Result<CapabilityPut, DbError> {
        let existing: Option<CapabilityRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {CAPABILITY_COLUMNS} FROM capability_defs WHERE key = $1 FOR UPDATE"
        )))
        .bind(write.key)
        .fetch_optional(&mut *conn)
        .await?;
        match existing {
            None => {
                let added = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "INSERT INTO capability_defs (key, label, description, risk, driver_patch,
                            default_decision, default_approver_scope, builtin, revision, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, false, 1, $8)
                     RETURNING {CAPABILITY_COLUMNS}"
                )))
                .bind(write.key)
                .bind(write.label)
                .bind(write.description)
                .bind(write.risk)
                .bind(write.driver_patch)
                .bind(write.default_decision)
                .bind(write.default_approver_scope)
                .bind(at)
                .fetch_one(conn)
                .await?;
                Ok(CapabilityPut::Added(added))
            }
            Some(row) if row.revision != seen => Ok(CapabilityPut::Stale(row)),
            Some(_) => {
                let replaced = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "UPDATE capability_defs SET label = $2, description = $3, risk = $4,
                            driver_patch = $5, default_decision = $6, default_approver_scope = $7,
                            revision = revision + 1, updated_at = $8
                      WHERE key = $1
                  RETURNING {CAPABILITY_COLUMNS}"
                )))
                .bind(write.key)
                .bind(write.label)
                .bind(write.description)
                .bind(write.risk)
                .bind(write.driver_patch)
                .bind(write.default_decision)
                .bind(write.default_approver_scope)
                .bind(at)
                .fetch_one(conn)
                .await?;
                Ok(CapabilityPut::Replaced(replaced))
            }
        }
    }

    /// Removes the definition of the capability `key` and returns it, or none when there was none.
    pub async fn delete_capability(
        conn: &mut Conn,
        key: &str,
    ) -> Result<Option<CapabilityRow>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "DELETE FROM capability_defs WHERE key = $1 RETURNING {CAPABILITY_COLUMNS}"
        )))
        .bind(key)
        .fetch_optional(conn)
        .await?)
    }

    /// How many environments that were not deleted hold the capability `key`, granted.
    pub async fn environments_holding(conn: &mut Conn, key: &str) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM environments WHERE deleted_at IS NULL AND $1 = ANY(granted_capabilities)",
        )
        .bind(key)
        .fetch_one(conn)
        .await?)
    }

    // ---- Harness builds a system admin added -----------------------------------------------

    /// What the key of a row that holds an added harness build begins with. The data model has no
    /// table of its own for them: they are rows of the system's settings under a name that is no
    /// key of the registry, which nothing else reads or writes.
    pub const HARNESS_BUILD_PREFIX: &str = "harness_build:";

    /// The harness builds that were added, as they were written, in the order of their keys.
    pub async fn added_harness_builds(conn: &mut Conn) -> Result<Vec<Json>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT value FROM settings WHERE node_id IS NULL AND starts_with(key, $1) ORDER BY key",
        )
        .bind(HARNESS_BUILD_PREFIX)
        .fetch_all(conn)
        .await?)
    }

    /// The prefix of the system's rows that hold the builds read from the vendors' release channels.
    pub const HARNESS_CHANNEL_PREFIX: &str = "harness_channel:";

    /// The builds read from the vendors' release channels, as their rows hold them.
    pub async fn channel_harness_builds(conn: &mut Conn) -> Result<Vec<Json>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT value FROM settings WHERE node_id IS NULL AND starts_with(key, $1) ORDER BY key",
        )
        .bind(HARNESS_CHANNEL_PREFIX)
        .fetch_all(conn)
        .await?)
    }

    /// Every build kept in the system's rows, read from a channel or added, with the key of its row.
    pub async fn stored_harness_builds(conn: &mut Conn) -> Result<Vec<(String, Json)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT key, value FROM settings
              WHERE node_id IS NULL AND (starts_with(key, $1) OR starts_with(key, $2)) ORDER BY key",
        )
        .bind(HARNESS_CHANNEL_PREFIX)
        .bind(HARNESS_BUILD_PREFIX)
        .fetch_all(conn)
        .await?)
    }

    /// Which of the orgs `orgs` are a person's own home (`org_kind = 'personal'`).
    pub async fn personal_orgs(conn: &mut Conn, orgs: &[Id]) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT node_id FROM orgs WHERE org_kind = 'personal' AND node_id = ANY($1)",
        )
        .bind(orgs)
        .fetch_all(conn)
        .await?)
    }

    /// Takes the build under `key` for a check: it is marked `checking`, unless a check holds it already and
    /// began after `stale_before` (one whose server went away holds it no longer). The answer says whether this
    /// check has it.
    pub async fn claim_harness_build_check(
        conn: &mut Conn,
        key: &str,
        at: DateTime<Utc>,
        stale_before: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE settings
                SET value = jsonb_set(jsonb_set(value, '{status}', '\"checking\"'), '{check,stage}', '\"interface\"'),
                    revision = revision + 1, updated_at = $2
              WHERE node_id IS NULL AND key = $1
                AND (value->>'status' IS DISTINCT FROM 'checking' OR updated_at < $3)",
        )
        .bind(key)
        .bind(at)
        .bind(stale_before)
        .execute(conn)
        .await?
        .rows_affected()
            == 1)
    }

    /// Keeps a build read from a release channel under `key` (which begins with [`HARNESS_CHANNEL_PREFIX`]).
    /// One that is kept already stays as it is, with its status and its check: a channel names a build once,
    /// and its checksum never changes. The answer says whether it is new.
    pub async fn put_channel_build(
        conn: &mut Conn,
        key: &str,
        build: &Json,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM settings WHERE node_id IS NULL AND key = $1)",
        )
        .bind(key)
        .fetch_one(&mut *conn)
        .await?;
        if known {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO settings (node_id, key, value, locked, revision, updated_by, updated_at)
             VALUES (NULL, $1, $2, false, 1, NULL, $3)",
        )
        .bind(key)
        .bind(build)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(true)
    }

    /// Records how far a kept build is known to work (`status`) and its last check, in its row under `key`,
    /// whichever of the two prefixes it has. False where no such row is kept.
    pub async fn set_harness_build_status(
        conn: &mut Conn,
        key: &str,
        status: &str,
        check: Option<&Json>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE settings
                SET value = CASE WHEN $3::jsonb IS NULL THEN jsonb_set(value, '{status}', to_jsonb($2::text))
                                 ELSE jsonb_set(jsonb_set(value, '{status}', to_jsonb($2::text)), '{check}', $3::jsonb) END,
                    revision = revision + 1, updated_at = $4
              WHERE node_id IS NULL AND key = $1",
        )
        .bind(key)
        .bind(status)
        .bind(check)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            == 1)
    }

    const IMPORT_BUILDS_SQL: &str =
        "INSERT INTO settings (node_id,key,value,locked,revision,updated_by,updated_at)
         SELECT NULL,key,value,false,1,$3,$4 FROM unnest($1::text[],$2::jsonb[]) AS b(key,value)
         ON CONFLICT (node_id,key) DO UPDATE SET value=EXCLUDED.value,
             revision=settings.revision+1,updated_by=EXCLUDED.updated_by,updated_at=EXCLUDED.updated_at";

    const IMPORT_DEFAULTS_SQL: &str = "WITH selected AS MATERIALIZED (
             SELECT key,jsonb_build_object('kind',value->'kind','os',value->'os',
                 'arch',value->'arch','is_default',true) AS filter
             FROM unnest($1::text[],$2::jsonb[]) AS b(key,value)
             WHERE value @> '{\"is_default\":true}'::jsonb
         ), previous AS MATERIALIZED (
             SELECT old.key FROM selected chosen JOIN LATERAL (
                 SELECT key FROM settings WHERE node_id IS NULL
                     AND value @> '{\"is_default\":true}'::jsonb AND value @> chosen.filter
                     AND starts_with(key,$5) AND key<>chosen.key
             ) old ON true
         )
         UPDATE settings s SET value=jsonb_set(value,'{is_default}','false'::jsonb),
             revision=revision+1,updated_by=$3,updated_at=$4
         FROM previous p WHERE s.node_id IS NULL AND s.key=p.key";

    /// Imports one validated, request-size-bounded checksum list in two statements, preserving
    /// the already resolved default for each harness, system and processor.
    pub async fn put_imported_harness_builds(
        conn: &mut Conn,
        builds: &[(String, Json)],
        by: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let keys: Vec<_> = builds.iter().map(|(key, _)| key.clone()).collect();
        let values: Vec<_> = builds.iter().map(|(_, value)| value.clone()).collect();
        sqlx::query(IMPORT_BUILDS_SQL)
            .bind(&keys)
            .bind(&values)
            .bind(by)
            .bind(at)
            .execute(&mut *conn)
            .await?;
        // Containment uses the existing JSON index; unrelated system settings and old
        // nondefault versions do not grow the amount of data brought into this process.
        sqlx::query(IMPORT_DEFAULTS_SQL)
            .bind(&keys)
            .bind(&values)
            .bind(by)
            .bind(at)
            .bind(HARNESS_BUILD_PREFIX)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Writes the added build under `key` (which begins with [`HARNESS_BUILD_PREFIX`]), replacing
    /// the one there is, and takes the default off the others added for the same harness, system
    /// and processor.
    pub async fn put_harness_build(
        conn: &mut Conn,
        key: &str,
        build: &Json,
        by: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        let replaced = sqlx::query(
            "UPDATE settings SET value = $2, revision = revision + 1, updated_by = $3, updated_at = $4
              WHERE node_id IS NULL AND key = $1",
        )
        .bind(key)
        .bind(build)
        .bind(by)
        .bind(at)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if replaced == 0 {
            sqlx::query(
                "INSERT INTO settings (node_id, key, value, locked, revision, updated_by, updated_at)
                 VALUES (NULL, $1, $2, false, 1, $3, $4)",
            )
            .bind(key)
            .bind(build)
            .bind(by)
            .bind(at)
            .execute(&mut *conn)
            .await?;
        }
        if build.get("is_default").and_then(Json::as_bool) == Some(true) {
            sqlx::query(
                "UPDATE settings SET value = jsonb_set(value, '{is_default}', 'false'::jsonb),
                        revision = revision + 1, updated_by = $6, updated_at = $7
                  WHERE node_id IS NULL AND starts_with(key, $1) AND key <> $2
                    AND value ->> 'kind' = $3 AND value ->> 'os' = $4 AND value ->> 'arch' = $5
                    AND value ->> 'is_default' = 'true'",
            )
            .bind(HARNESS_BUILD_PREFIX)
            .bind(key)
            .bind(build.get("kind").and_then(Json::as_str))
            .bind(build.get("os").and_then(Json::as_str))
            .bind(build.get("arch").and_then(Json::as_str))
            .bind(by)
            .bind(at)
            .execute(conn)
            .await?;
        }
        Ok(())
    }

    /// Makes a profile of the system, which has no home and no owner: system admins hold every
    /// permission on it, and its page names nobody.
    pub async fn insert_system_agent(
        conn: &mut Conn,
        id: Id,
        name: &str,
        harness_kind: &str,
        profile: &Json,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, machine_id, profile,
                                 created_at, updated_at)
             VALUES ($1, $2, NULL, NULL, NULL, $3, NULL, $4, $5, $5)",
        )
        .bind(id)
        .bind(name)
        .bind(harness_kind)
        .bind(profile)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }
}

// ---- A provider's runtime, its sign-in, and what keeps them fresh ----------------------------
//
// One block, kept together at the end of the file: the unit a managed provider's credential lives
// in, where the provider stands while that unit is made, started or stopped, the sign-in as its
// harness last told it, the sign-in that waits for a unit that is not up yet, and the providers
// whose unit is idle or whose sign-in is due to be looked at. Nothing here decides anything.

/// What a managed provider's runtime, its sign-in and their upkeep read and write.
pub mod provider_runtime {
    use crate::queries::environments::hosting::RuntimeFull;
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    const RUNTIME_COLUMNS: &str = "id, kind, host_id, machine_id, unit_name, successor_of, provider_id,
        environment_id, driver, template_id, spec_hash, generation, state, state_detail, remove_at, volumes,
        last_active_at, created_at, updated_at";

    /// The live runtime of a provider: the one that is neither retired nor removed and is nobody's
    /// successor in the making.
    pub async fn live_runtime(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE provider_id = $1 AND kind = 'provider' AND successor_of IS NULL
                AND state NOT IN ('retired', 'removed')"
        )))
        .bind(provider)
        .fetch_optional(conn)
        .await?)
    }

    /// The live runtimes of these providers, one each at most.
    pub async fn live_runtimes(
        conn: &mut Conn,
        providers: &[Id],
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE provider_id = ANY($1) AND kind = 'provider' AND successor_of IS NULL
                AND state NOT IN ('retired', 'removed')"
        )))
        .bind(providers)
        .fetch_all(conn)
        .await?)
    }

    /// A runtime to make for a provider.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewRuntime<'a> {
        /// Its identifier.
        pub id: Id,
        /// The host it is on.
        pub host_id: Id,
        /// The name of its unit.
        pub unit_name: &'a str,
        /// The provider it is for.
        pub provider_id: Id,
        /// The driver that runs it.
        pub driver: &'a str,
        /// The template it is made from.
        pub template_id: Id,
        /// The hash of what it is made with.
        pub spec_hash: &'a [u8],
        /// Which unit it is of its provider.
        pub generation: i32,
        /// Its volumes, by what they are for: the `state` volume alone.
        pub volumes: &'a Json,
    }

    /// Makes the runtime of a provider, `creating`. The one live runtime of a provider is a rule of
    /// the schema: a second is a [`DbError::Unique`].
    pub async fn insert_runtime(
        conn: &mut Conn,
        made: &NewRuntime<'_>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO runtimes (id, kind, host_id, unit_name, provider_id, driver, template_id, spec_hash,
                                   generation, state, volumes, created_at, updated_at)
             VALUES ($1, 'provider', $2, $3, $4, $5, $6, $7, $8, 'creating', $9, $10, $10)",
        )
        .bind(made.id)
        .bind(made.host_id)
        .bind(made.unit_name)
        .bind(made.provider_id)
        .bind(made.driver)
        .bind(made.template_id)
        .bind(made.spec_hash)
        .bind(made.generation)
        .bind(made.volumes)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// A provider, as the upkeep of its runtime and its sign-in read it.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct State {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: String,
        /// Its first owner.
        pub owner_id: Id,
        /// The root org of its home.
        pub org_id: Id,
        /// The org it belongs to.
        pub home_id: Id,
        /// `subscription` or `api`.
        pub kind: String,
        /// Forwarded access protocols, independent of how the credential is kept.
        pub protocols: Vec<String>,
        /// For a subscription: the harness whose login it is.
        pub harness_kind: Option<String>,
        /// For a subscription: the profile whose harness holds the login.
        pub agent_id: Option<Id>,
        /// `managed` or `connected`.
        pub placement: String,
        /// Where its runtime is; none while a managed one is not up.
        pub machine_id: Option<Id>,
        /// The machine that keeps the key.
        pub key_machine_id: Option<Id>,
        /// What it stands at.
        pub status: String,
        /// A sentence about that.
        pub status_detail: Option<String>,
        /// Its runtime's own values.
        pub runtime_config: Json,
        /// What its adapter's options hold.
        pub config: Json,
        /// What the harness last told of the sign-in.
        pub login_state: Option<Json>,
        /// The sign-in that waits for the runtime, as it was kept.
        pub login_pending: Option<Vec<u8>>,
        /// Since when it waits.
        pub login_pending_at: Option<DateTime<Utc>>,
        /// The environment its credential is with.
        pub held_by_environment_id: Option<Id>,
        /// The selected facts/models sources, learned descriptors and safe per-element errors.
        #[sqlx(default)]
        pub sources: Json,
        /// What its harness or endpoint last reported.
        pub facts: Json,
        /// When it was deleted.
        pub deleted_at: Option<DateTime<Utc>>,
        /// Its revision.
        pub revision: i64,
    }

    const STATE_COLUMNS: &str = "id, name, owner_id, org_id, home_id, kind, protocols, harness_kind, agent_id, provider_placement(machine_id) AS placement,
        machine_id, key_machine_id, status, status_detail, runtime_config, config, login_state, login_pending,
        login_pending_at, held_by_environment_id, sources, facts, deleted_at, revision";

    /// The provider `id`, deleted or not.
    pub async fn state(conn: &mut Conn, id: Id) -> Result<Option<State>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM providers WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// The provider `id`, deleted or not, locked until the transaction ends.
    pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<State>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {STATE_COLUMNS} FROM providers WHERE id = $1 FOR UPDATE"
        )))
        .bind(id)
        .fetch_optional(conn)
        .await?)
    }

    /// Says where a managed provider's runtime is: the machine of its connector while the unit is up,
    /// none while it is not. Counts as a change of the provider.
    pub async fn set_machine(
        conn: &mut Conn,
        provider: Id,
        machine: Option<Id>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET machine_id = $2, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND EXISTS (SELECT 1 FROM runtimes r WHERE r.provider_id=providers.id)
                AND machine_id IS DISTINCT FROM $2",
        )
        .bind(provider)
        .bind(machine)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    /// Keeps what the harness last told of the sign-in. Counts as a change of the provider.
    pub async fn set_login_state(
        conn: &mut Conn,
        provider: Id,
        state: &Json,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET login_state = $2, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND login_state IS DISTINCT FROM $2",
        )
        .bind(provider)
        .bind(state)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    /// Keeps the sign-in that waits for the runtime, replacing one that waited. Its secrets are in
    /// rows of their own, sealed; what is kept here is the method and the references to them.
    pub async fn keep_pending(
        conn: &mut Conn,
        provider: Id,
        pending: &[u8],
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE providers SET login_pending = $2, login_pending_at = $3 WHERE id = $1")
            .bind(provider)
            .bind(pending)
            .bind(at)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Forgets the sign-in that waited. Returns whether one did.
    pub async fn clear_pending(conn: &mut Conn, provider: Id) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET login_pending = NULL, login_pending_at = NULL
              WHERE id = $1 AND login_pending IS NOT NULL",
        )
        .bind(provider)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    /// The providers whose pending sign-in has waited since before `before`.
    pub async fn pending_before(
        conn: &mut Conn,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers
              WHERE login_pending IS NOT NULL AND login_pending_at <= $1
              ORDER BY login_pending_at, id LIMIT $2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The providers that have a pending sign-in, of any age.
    pub async fn with_pending(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers WHERE login_pending IS NOT NULL AND deleted_at IS NULL
              ORDER BY login_pending_at, id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Takes away the secrets a sign-in left for the runtime to take, by name prefix.
    pub async fn delete_login_secrets(
        conn: &mut Conn,
        provider: Id,
        prefix: &str,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "DELETE FROM secrets WHERE owner_kind = 'provider' AND owner_id = $1 AND starts_with(name, $2)",
        )
        .bind(provider)
        .bind(prefix)
        .execute(conn)
        .await?
        .rows_affected())
    }

    /// What holds a provider's runtime up: the turns running on it, the leases it has open and the
    /// sessions its harnesses hold.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
    pub struct Busy {
        /// Turns of threads bound to it that are not over.
        pub turns: i64,
        /// Leases of it that are open.
        pub leases: i64,
        /// Sessions its harnesses hold open.
        pub sessions: i64,
    }

    /// What holds the runtime of a provider up now.
    pub async fn busy(conn: &mut Conn, provider: Id) -> Result<Busy, DbError> {
        Ok(sqlx::query_as(
            "SELECT (SELECT count(*) FROM turns u JOIN threads t ON t.node_id = u.thread_id
                      WHERE t.provider_id = $1 AND u.state IN ('starting', 'running'))::bigint AS turns,
                    (SELECT count(*) FROM leases WHERE provider_id = $1 AND closed_at IS NULL)::bigint AS leases,
                    (SELECT COALESCE(sum(sessions_open), 0) FROM harnesses WHERE provider_id = $1)::bigint AS sessions",
        )
        .bind(provider)
        .fetch_one(conn)
        .await?)
    }

    /// The runtimes of providers that are up, those that were active longest ago first: the schedule
    /// applies each provider's own idle timeout.
    pub async fn running(conn: &mut Conn, limit: i64) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE kind = 'provider' AND state = 'running' AND successor_of IS NULL
              ORDER BY COALESCE(last_active_at, created_at), id LIMIT $1"
        )))
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The runtimes of providers whose stop was begun before `before` and not carried through, by a
    /// process that is gone.
    pub async fn stale_stops(
        conn: &mut Conn,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE kind = 'provider' AND state = 'stopping' AND updated_at <= $1
              ORDER BY updated_at, id LIMIT $2"
        )))
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The retired runtimes of providers whose time to be removed has come.
    pub async fn retired_due(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<RuntimeFull>, DbError> {
        Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {RUNTIME_COLUMNS} FROM runtimes
              WHERE kind = 'provider' AND state = 'retired' AND lost_at IS NULL AND remove_at IS NOT NULL AND remove_at <= $1
              ORDER BY remove_at, id LIMIT $2"
        )))
        .bind(now)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// A runtime that is stopped and has been for a while, with the provider it keeps the sign-in of.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct Stopped {
        /// The provider.
        pub provider_id: Id,
        /// Its runtime.
        pub runtime_id: Id,
        /// Since when the unit has been stopped.
        pub since: DateTime<Utc>,
    }

    /// The managed providers, not deleted and not switched off, whose runtime is stopped and has been
    /// since before `before`: the candidates for the renewal of a sign-in. The schedule applies each
    /// provider's own number of days, and leaves out those that hold no sign-in a harness renews.
    pub async fn stopped_before(
        conn: &mut Conn,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Stopped>, DbError> {
        Ok(sqlx::query_as(
            "SELECT r.provider_id, r.id AS runtime_id, r.updated_at AS since
               FROM runtimes r JOIN providers p ON p.id = r.provider_id
              WHERE r.kind = 'provider' AND r.state = 'stopped' AND r.successor_of IS NULL
                AND p.deleted_at IS NULL AND p.status <> 'disabled' AND p.kind = 'subscription'
                AND r.updated_at <= $1
              ORDER BY r.updated_at, r.id LIMIT $2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// A running or stopped native login whose renewal policy the scheduled job must inspect.
    #[derive(Debug, Clone, sqlx::FromRow)]
    pub struct RenewalCandidate {
        /// Its provider.
        pub provider_id: Id,
        /// The actual credential expiry, when the runtime reports one.
        pub expires_at: Option<DateTime<Utc>>,
        /// Last successful renewal, or creation while none has succeeded yet.
        pub since: DateTime<Utc>,
    }

    /// Every native login in stable pages, on a unit or a declared directory runtime. Tokens do not renew.
    pub async fn renewal_candidates(
        conn: &mut Conn,
        after: Option<Id>,
        limit: i64,
    ) -> Result<Vec<RenewalCandidate>, DbError> {
        Ok(sqlx::query_as("SELECT p.id AS provider_id,
                (p.facts #>> '{credential,expires_at}')::timestamptz AS expires_at,
                COALESCE(p.credential_renewed_at, p.created_at) AS since
            FROM providers p WHERE p.kind = 'subscription'
              AND p.deleted_at IS NULL AND p.status <> 'disabled' AND p.held_by_environment_id IS NULL
              AND COALESCE(p.facts #>> '{credential,yields}', 'login') = 'login'
              AND ($1::uuid IS NULL OR p.id > $1)
              AND (EXISTS (SELECT 1 FROM runtimes r WHERE r.provider_id = p.id AND r.kind = 'provider'
                    AND r.successor_of IS NULL AND r.state IN ('running','stopped')
                    AND (p.key_machine_id IS NULL OR EXISTS (SELECT 1 FROM machines keeper WHERE keeper.id=p.key_machine_id AND keeper.runtime_id=r.id)))
                   OR EXISTS (SELECT 1 FROM machines m WHERE m.id=p.key_machine_id AND m.status='active'
                     AND m.deleted_at IS NULL AND m.online AND 'agent'=ANY(m.roles)
                     AND COALESCE(m.facts #>> '{provider_runtime_descriptor,lifecycle}',m.facts #>> '{environment_descriptor,lifecycle}')='attach'))
            ORDER BY p.id LIMIT $2")
            .bind(after).bind(limit).fetch_all(conn).await?)
    }

    /// Renewal starts a unit only when that unit actually keeps the sign-in.
    pub async fn keeper_has_unit(conn: &mut Conn, provider: Id) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM providers p JOIN runtimes r ON r.provider_id=p.id
            WHERE p.id=$1 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')
              AND (p.key_machine_id IS NULL OR EXISTS (SELECT 1 FROM machines keeper WHERE keeper.id=p.key_machine_id AND keeper.runtime_id=r.id)))")
            .bind(provider).fetch_one(conn).await?)
    }

    /// Records only an observed successful renewal, independently of page refreshes and idle stops.
    pub async fn mark_credential_renewed(
        conn: &mut Conn,
        provider: Id,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE providers SET credential_renewed_at = $2 WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(provider)
        .bind(at)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Successful native-renewal times for a page of providers; ordinary facts reads never move them.
    pub async fn renewal_stamps(
        conn: &mut Conn,
        ids: &[Id],
    ) -> Result<Vec<(Id, Option<DateTime<Utc>>)>, DbError> {
        Ok(
            sqlx::query_as("SELECT id, credential_renewed_at FROM providers WHERE id = ANY($1)")
                .bind(ids)
                .fetch_all(conn)
                .await?,
        )
    }

    /// A subscription to make, which a managed runtime holds the sign-in of.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewSubscription<'a> {
        /// Its identifier.
        pub id: Id,
        /// Its name.
        pub name: &'a str,
        /// Its first owner.
        pub owner_id: Id,
        /// Its home: an org.
        pub home_id: Id,
        /// The harness whose login it is.
        pub harness_kind: &'a str,
        /// The profile whose harness holds the login.
        pub agent_id: Id,
        /// What its adapter's options hold.
        pub config: &'a Json,
        /// The types its owner allows.
        pub arrangements: &'a [String],
        /// Its runtime's own values.
        pub runtime_config: &'a Json,
    }

    /// Makes a managed subscription at home in an org; its root org is read from the hierarchy. It
    /// needs a sign-in. Returns whether the home is there.
    pub async fn insert_subscription(
        conn: &mut Conn,
        provider: &NewSubscription<'_>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id, config,
                                    arrangements, runtime_config, status, created_at, updated_at)
             SELECT $1, $2, $3, n.org_id, n.id, 'subscription', $5, $6, $7, $8, $9, 'needs_setup',
                    $10, $10
               FROM nodes n WHERE n.id = $4 AND n.deleted_at IS NULL",
        )
        .bind(provider.id)
        .bind(provider.name)
        .bind(provider.owner_id)
        .bind(provider.home_id)
        .bind(provider.harness_kind)
        .bind(provider.agent_id)
        .bind(provider.config)
        .bind(provider.arrangements)
        .bind(provider.runtime_config)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    /// The names of the secrets a managed provider's runtime is given on the way to a command, so
    /// that they are found by prefix and taken away again.
    pub const LOGIN_SECRET_PREFIX: &str = "login:";

    /// The kinds of harness the system has a profile of that is not tied to a machine and not
    /// switched off: the builds a runtime of a provider on a key is given.
    pub async fn system_harness_kinds(conn: &mut Conn) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT harness_kind FROM agents
              WHERE machine_id IS NULL AND deleted_at IS NULL AND NOT disabled
              ORDER BY harness_kind",
        )
        .fetch_all(conn)
        .await?)
    }

    /// The people who are told of what happens to a provider: its owner, and every person its page
    /// names for `provider.manage` or as an owner.
    pub async fn managers_of(conn: &mut Conn, id: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT owner_id FROM providers WHERE id = $1
             UNION
             SELECT holder_id FROM permission_entries
              WHERE object_kind = 'provider' AND object_id = $1 AND page = 'live'
                AND permission IN ('provider.manage', 'access.own')
                AND holder_kind = 'user' AND holder_id IS NOT NULL",
        )
        .bind(id)
        .fetch_all(conn)
        .await?)
    }

    /// The secrets of a provider that a sign-in left for the runtime to take, with their names and
    /// identifiers: what the command that gives them to the harness refers to.
    pub async fn login_secrets(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Vec<(String, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT name, id FROM secrets
              WHERE owner_kind = 'provider' AND owner_id = $1 AND starts_with(name, $2) ORDER BY name",
        )
        .bind(provider)
        .bind(LOGIN_SECRET_PREFIX)
        .fetch_all(conn)
        .await?)
    }

    /// When anything last happened on a provider's runtime's account: a lease opened or closed, a turn
    /// of a thread bound to it began or ended, a harness of it did something. None when nothing ever did.
    pub async fn activity_at(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<DateTime<Utc>>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT GREATEST(
                 (SELECT max(GREATEST(opened_at, COALESCE(closed_at, opened_at))) FROM leases
                   WHERE provider_id = $1),
                 (SELECT max(GREATEST(u.created_at, COALESCE(u.started_at, u.created_at),
                                      COALESCE(u.completed_at, u.created_at)))
                    FROM turns u JOIN threads t ON t.node_id = u.thread_id WHERE t.provider_id = $1),
                 (SELECT max(last_active_at) FROM harnesses WHERE provider_id = $1))",
        )
        .bind(provider)
        .fetch_one(conn)
        .await?)
    }

    /// As [`activity_at`], for several providers in one statement.
    pub async fn activities_at(
        conn: &mut Conn,
        providers: &[Id],
    ) -> Result<Vec<(Id, Option<DateTime<Utc>>)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT p.id,
                    GREATEST(
                      (SELECT max(GREATEST(opened_at, COALESCE(closed_at, opened_at))) FROM leases l
                        WHERE l.provider_id = p.id),
                      (SELECT max(GREATEST(u.created_at, COALESCE(u.started_at, u.created_at),
                                           COALESCE(u.completed_at, u.created_at)))
                         FROM turns u JOIN threads t ON t.node_id = u.thread_id WHERE t.provider_id = p.id),
                      (SELECT max(last_active_at) FROM harnesses h WHERE h.provider_id = p.id))
               FROM providers p WHERE p.id = ANY($1)",
        )
        .bind(providers)
        .fetch_all(conn)
        .await?)
    }

    /// The threads of a provider whose turn is not over.
    pub async fn threads_with_turns(conn: &mut Conn, provider: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT t.node_id FROM threads t JOIN turns u ON u.thread_id = t.node_id
              WHERE t.provider_id = $1 AND u.state IN ('starting', 'running')",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }

    /// The providers whose sign-in completed and was not looked at since: the state it has now is not
    /// yet the facts the provider shows.
    pub async fn completed_not_looked_at(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers
              WHERE deleted_at IS NULL AND login_state ->> 'state' = 'completed'
                AND NOT (login_state ? 'looked_at')
              ORDER BY updated_at, id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Says that a completed sign-in was looked at: the facts were asked for after it.
    pub async fn mark_looked_at(
        conn: &mut Conn,
        provider: Id,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET login_state = jsonb_set(login_state, '{looked_at}', to_jsonb($2::timestamptz)),
                    updated_at = $2
              WHERE id = $1 AND login_state ->> 'state' = 'completed' AND NOT (login_state ? 'looked_at')",
        )
        .bind(provider)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    /// The providers whose sign-in waits for the person and whose end has passed.
    pub async fn overdue_logins(
        conn: &mut Conn,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM providers
              WHERE deleted_at IS NULL AND login_state ->> 'state' = 'awaiting_user'
                AND (login_state ->> 'expires_at')::timestamptz <= $1
              ORDER BY updated_at, id LIMIT $2",
        )
        .bind(now)
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The subscriptions that hold a token whose end is known, with the end: what the notices of a
    /// lapse are made from.
    #[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
    pub struct TokenEnd {
        /// The provider.
        pub provider_id: Id,
        /// When the token lapses.
        pub expires_at: DateTime<Utc>,
        /// When the credential was issued, where its own claims or stated life supplied it.
        pub issued_at: Option<DateTime<Utc>>,
        /// Where it stands.
        pub status: String,
        /// The root org of its home.
        pub org_id: Id,
        /// Its name.
        pub name: String,
    }

    /// The providers not deleted whose facts say when a token lapses.
    pub async fn token_ends(conn: &mut Conn, limit: i64) -> Result<Vec<TokenEnd>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id AS provider_id,
                    (facts #>> '{credential,expires_at}')
                        ::timestamptz AS expires_at,
                    (facts #>> '{credential,issued_at}')
                        ::timestamptz AS issued_at,
                    status, org_id, name
               FROM providers
              WHERE deleted_at IS NULL AND facts #>> '{credential,yields}' IN ('token','key')
                AND (facts #>> '{credential,expires_at}')
                    IS NOT NULL
              ORDER BY id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// Whose a template is, when it is somebody's rather than the system's.
    pub async fn template_owner(conn: &mut Conn, template: Id) -> Result<Option<Id>, DbError> {
        Ok(
            sqlx::query_scalar::<_, Option<Id>>("SELECT owner_id FROM env_templates WHERE id = $1")
                .bind(template)
                .fetch_optional(conn)
                .await?
                .flatten(),
        )
    }

    /// Sets what a provider that is not ready says, without moving its state, and counts it as a
    /// change of the provider.
    pub async fn set_detail(
        conn: &mut Conn,
        provider: Id,
        detail: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET status_detail = $2, revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND status_detail IS DISTINCT FROM $2",
        )
        .bind(provider)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            > 0)
    }

    // ------------------------------------------------------------ a credential that leaves the provider

    /// Takes a provider's credential for an environment, in one compare-and-set on the credential being
    /// at home: the holder and the time are recorded together. Whether this call took it.
    pub async fn take_credential(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET held_by_environment_id = $2, held_since = $3,
                    revision = revision + 1, updated_at = $3
              WHERE id = $1 AND deleted_at IS NULL AND held_by_environment_id IS NULL",
        )
        .bind(provider)
        .bind(environment)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            == 1)
    }

    /// Brings a credential home: the holder is cleared, where the environment is the one that held it.
    /// Whether this call did.
    pub async fn release_credential(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET held_by_environment_id = NULL, held_since = NULL,
                    revision = revision + 1, updated_at = $3
              WHERE id = $1 AND held_by_environment_id = $2",
        )
        .bind(provider)
        .bind(environment)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            == 1)
    }

    /// Gives a credential up: the holder is cleared and the provider has to be signed in again, with the
    /// sign-in its harness last reported forgotten. Whether the environment held it.
    pub async fn declare_credential_lost(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
        detail: &str,
        at: DateTime<Utc>,
    ) -> Result<bool, DbError> {
        Ok(sqlx::query(
            "UPDATE providers SET held_by_environment_id = NULL, held_since = NULL,
                    status = 'needs_setup', status_detail = $3, login_state = NULL,
                    revision = revision + 1, updated_at = $4
              WHERE id = $1 AND held_by_environment_id = $2",
        )
        .bind(provider)
        .bind(environment)
        .bind(detail)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected()
            == 1)
    }

    /// The turns that are under way on a provider in threads that are not in this environment.
    pub async fn turns_outside(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM turns u JOIN threads t ON t.node_id = u.thread_id
              WHERE t.provider_id = $1 AND t.environment_id IS DISTINCT FROM $2
                AND u.state IN ('starting', 'running')",
        )
        .bind(provider)
        .bind(environment)
        .fetch_one(conn)
        .await?)
    }

    /// The threads in the environment whose turn is under way on the provider.
    pub async fn threads_running_in(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
    ) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT t.node_id FROM turns u JOIN threads t ON t.node_id = u.thread_id
              WHERE t.provider_id = $1 AND t.environment_id = $2 AND u.state IN ('starting', 'running')
              ORDER BY t.node_id",
        )
        .bind(provider)
        .bind(environment)
        .fetch_all(conn)
        .await?)
    }

    /// How many threads of the environment are bound to the provider in an arrangement that hands the
    /// credential over, not archived and not deleted.
    pub async fn handed_over_threads_in(
        conn: &mut Conn,
        provider: Id,
        environment: Id,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.provider_id = $1 AND t.environment_id = $2 AND t.arrangement = 'handed_over'
                AND n.deleted_at IS NULL AND n.archived_at IS NULL",
        )
        .bind(provider)
        .bind(environment)
        .fetch_one(conn)
        .await?)
    }

    /// The providers whose credential is handed over to an environment, with the environment, when no
    /// thread there is bound to them in the arrangement any longer or they are switched off.
    pub async fn handovers_no_longer_needed(
        conn: &mut Conn,
        limit: i64,
    ) -> Result<Vec<(Id, Id)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT p.id, p.held_by_environment_id FROM providers p
              WHERE p.held_by_environment_id IS NOT NULL AND p.deleted_at IS NULL
                AND (p.status = 'disabled'
                     OR NOT EXISTS (SELECT 1 FROM threads t JOIN nodes n ON n.id = t.node_id
                                     WHERE t.provider_id = p.id AND t.environment_id = p.held_by_environment_id
                                       AND t.arrangement = 'handed_over'
                                       AND n.deleted_at IS NULL AND n.archived_at IS NULL))
              ORDER BY p.held_since, p.id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// A lease to open for a turn whose harness is given the credential: the form it has and the machine
    /// it may be issued to.
    #[derive(Debug, Clone, PartialEq)]
    pub struct NewCredentialLease<'a> {
        /// Its identifier.
        pub id: Id,
        /// The provider.
        pub provider_id: Id,
        /// The thread.
        pub thread_id: Id,
        /// `shared` or `handover`.
        pub mode: &'a str,
        /// The form of the credential.
        pub form: &'a str,
        /// The machine where the harness runs: the only one a copy is issued to. A credential that is
        /// handed over records it once it has been delivered, and not before.
        pub recipient_machine_id: Option<Id>,
        /// The models it allows.
        pub allowed_models: &'a [String],
        /// When.
        pub at: DateTime<Utc>,
    }

    /// Opens the lease of a turn that is given a credential. A thread has one open lease at most; the
    /// answer is none where it has one.
    pub async fn open_credential_lease(
        conn: &mut Conn,
        new: &NewCredentialLease<'_>,
    ) -> Result<Option<crate::queries::threads::LeaseRow>, DbError> {
        Ok(sqlx::query_as(
            "INSERT INTO leases (id, provider_id, thread_id, mode, form, recipient_machine_id,
                                 allowed_models, opened_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (thread_id) WHERE closed_at IS NULL DO NOTHING
             RETURNING id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models,
                       allowance, opened_at, closed_at, close_reason",
        )
        .bind(new.id)
        .bind(new.provider_id)
        .bind(new.thread_id)
        .bind(new.mode)
        .bind(new.form)
        .bind(new.recipient_machine_id)
        .bind(new.allowed_models)
        .bind(new.at)
        .fetch_optional(conn)
        .await?)
    }

    /// Records that the credential a lease took was delivered to the machine: from then on the
    /// credential is with its harness, and the leases that follow need not deliver it again.
    pub async fn set_lease_recipient(
        conn: &mut Conn,
        lease: Id,
        machine: Id,
    ) -> Result<bool, DbError> {
        Ok(
            sqlx::query("UPDATE leases SET recipient_machine_id = $2 WHERE id = $1")
                .bind(lease)
                .bind(machine)
                .execute(conn)
                .await?
                .rows_affected()
                == 1,
        )
    }

    /// Whether the credential has been delivered to the environment that holds it: a lease of the mode
    /// `handover`, opened since it was taken, has recorded its recipient.
    pub async fn credential_delivered(conn: &mut Conn, provider: Id) -> Result<bool, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM leases l JOIN providers p ON p.id = l.provider_id
                             WHERE p.id = $1 AND p.held_since IS NOT NULL AND l.mode = 'handover'
                               AND l.recipient_machine_id IS NOT NULL AND l.opened_at >= p.held_since)",
        )
        .bind(provider)
        .fetch_one(conn)
        .await?)
    }

    /// Names the lease a turn runs under, for a turn that was made before its lease could be.
    pub async fn set_turn_lease(conn: &mut Conn, turn: Id, lease: Id) -> Result<bool, DbError> {
        Ok(
            sqlx::query("UPDATE turns SET lease_id = $2 WHERE id = $1 AND lease_id IS NULL")
                .bind(turn)
                .bind(lease)
                .execute(conn)
                .await?
                .rows_affected()
                == 1,
        )
    }

    /// The open leases of the mode `shared` whose turn is under way.
    pub async fn open_shared_leases(
        conn: &mut Conn,
        limit: i64,
    ) -> Result<Vec<crate::queries::threads::LeaseRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT l.id, l.provider_id, l.thread_id, l.turn_id, l.entry_id, l.mode, l.form,
                    l.allowed_models, l.allowance, l.opened_at, l.closed_at, l.close_reason
               FROM leases l JOIN turns t ON t.id = l.turn_id
              WHERE l.mode = 'shared' AND l.closed_at IS NULL AND t.state IN ('starting', 'running')
              ORDER BY l.opened_at, l.id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(conn)
        .await?)
    }

    /// The non-secret state needed to resume renewing a lease's credential after a process restart.
    #[derive(Debug, Clone, sqlx::FromRow)]
    pub struct SharedCopyState {
        /// The last delivered copy's expiry, where it expires.
        pub expires_at: Option<DateTime<Utc>>,
        /// When the last successful issue was recorded.
        pub issued_at: Option<DateTime<Utc>>,
        /// The successful issues already delivered, used to continue their command identities.
        pub issues: i64,
    }

    /// A shared lease's expiry, minimum-interval anchor and next issue command identity.
    pub async fn shared_copy_state(
        conn: &mut Conn,
        lease: Id,
    ) -> Result<Option<SharedCopyState>, DbError> {
        Ok(sqlx::query_as(
            "SELECT l.copy_expires_at AS expires_at, l.copy_issued_at AS issued_at, l.copy_issued_count AS issues
               FROM leases l
              WHERE l.id = $1 AND l.mode = 'shared' AND l.closed_at IS NULL",
        ).bind(lease).fetch_optional(conn).await?)
    }

    /// Saves the expiry of a delivered shared copy in the same transaction as its issue record.
    pub async fn record_shared_copy_expiry(
        conn: &mut Conn,
        lease: Id,
        expires: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE leases SET copy_expires_at = $2, copy_issued_at = $3, copy_issued_count = copy_issued_count + 1
            WHERE id = $1 AND mode = 'shared' AND closed_at IS NULL")
            .bind(lease).bind(expires).bind(at).execute(conn).await?;
        Ok(())
    }

    /// The endpoints of an API provider's upstream: `[{ protocol, base_url, options }]`.
    pub async fn upstreams_of(conn: &mut Conn, provider: Id) -> Result<Json, DbError> {
        Ok(
            sqlx::query_scalar::<_, Json>("SELECT upstreams FROM providers WHERE id = $1")
                .bind(provider)
                .fetch_optional(conn)
                .await?
                .unwrap_or_else(|| Json::Array(Vec::new())),
        )
    }

    /// The profile whose harness runs on the machine for the provider: the one that holds a credential
    /// that was given to it there.
    pub async fn harness_agent_on(
        conn: &mut Conn,
        provider: Id,
        machine: Id,
    ) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT agent_id FROM harnesses WHERE provider_id = $1 AND machine_id = $2
              ORDER BY (status = 'online') DESC, updated_at DESC LIMIT 1",
        )
        .bind(provider)
        .bind(machine)
        .fetch_optional(conn)
        .await?)
    }

    /// Since when a provider's credential is with the environment that holds it.
    pub async fn held_since(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<DateTime<Utc>>, DbError> {
        Ok(sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT held_since FROM providers WHERE id = $1",
        )
        .bind(provider)
        .fetch_optional(conn)
        .await?
        .flatten())
    }

    /// The threads bound to a provider that are blocked: what a credential that came home, or that was
    /// given up, may let go on.
    pub async fn blocked_threads_of(conn: &mut Conn, provider: Id) -> Result<Vec<Id>, DbError> {
        Ok(sqlx::query_scalar(
            "SELECT node_id FROM threads WHERE provider_id = $1 AND state = 'blocked' ORDER BY node_id",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }

    /// Closes the leases that gave a credential to the harness of an environment, for the provider: the
    /// credential is no longer there.
    pub async fn close_handover_leases(
        conn: &mut Conn,
        provider: Id,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "UPDATE leases SET closed_at = $3, close_reason = $2
              WHERE provider_id = $1 AND mode = 'handover' AND closed_at IS NULL",
        )
        .bind(provider)
        .bind(reason)
        .bind(at)
        .execute(conn)
        .await?
        .rows_affected())
    }

    /// The machine a lease may issue a copy of the credential to.
    pub async fn lease_recipient(conn: &mut Conn, lease: Id) -> Result<Option<Id>, DbError> {
        Ok(sqlx::query_scalar::<_, Option<Id>>(
            "SELECT recipient_machine_id FROM leases WHERE id = $1",
        )
        .bind(lease)
        .fetch_optional(conn)
        .await?
        .flatten())
    }

    /// The open leases under which a credential was given to a harness, with the environment of their
    /// thread, for the provider.
    pub async fn open_credential_leases(
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Vec<crate::queries::threads::LeaseRow>, DbError> {
        Ok(sqlx::query_as(
            "SELECT id, provider_id, thread_id, turn_id, entry_id, mode, form, allowed_models, allowance,
                    opened_at, closed_at, close_reason
               FROM leases
              WHERE provider_id = $1 AND closed_at IS NULL AND mode IN ('shared', 'handover')
              ORDER BY opened_at, id",
        )
        .bind(provider)
        .fetch_all(conn)
        .await?)
    }
}

// Re-enrollment preserves an existing machine and its registrations.
pub mod reenrollment;

/// Mount resources, their machine locations and the settings that enable them.
pub mod mounts;

/// MCP preset and environment-owned resources and their enablement.
pub mod mcp;

/// Skills, their immutable versions, their drafts, and the versions each turn was given.
pub mod skills;

/// Online hosts whose reported drivers can propose a template without allocating a unit.
pub mod template_imports;
