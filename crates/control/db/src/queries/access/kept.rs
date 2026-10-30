//! The identities a person holds (their groups and places of page-only viewing among them) with what they were worked
//! out under, so that they can be kept between requests and used again exactly while nothing they come from changed.
//!
//! `access_holders` reads every place and every page of the system, which is too much for each request. The
//! database records each transaction that writes a row it reads (`access_changes`, written by triggers, so that no
//! writer can leave one out). A set worked out under one snapshot still holds under a later one when no such
//! transaction is visible to the later snapshot that was not to the earlier, and when no role or entry of a page
//! begins or stops counting between the two moments it is asked at (the only time the function compares).

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// What a holder set was worked out under: the database, the snapshot, and the moments between which no role or
/// entry of a page begins or stops counting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    database: i64,
    snapshot: String,
    /// The latest moment, at or before the one asked about, at which a role or an entry stopped counting.
    from: Option<DateTime<Utc>>,
    /// The first moment after the one asked about at which a role or an entry stops counting.
    until: Option<DateTime<Utc>>,
    keepable: bool,
}

impl Stamp {
    /// Whether no role or entry begins or stops counting between the moment the set was worked out at and `at`.
    #[must_use]
    pub fn covers(&self, at: DateTime<Utc>) -> bool {
        self.from.is_none_or(|from| from <= at) && self.until.is_none_or(|until| at < until)
    }

    /// Whether the set may be used by other requests: it was worked out by a transaction that had written nothing.
    /// One that had written could see changes of its own that are not committed, and may never be.
    #[must_use]
    pub fn keepable(&self) -> bool {
        self.keepable
    }
}

/// The identities `access_holders` finds for a person now: their groups and places of page-only viewing (each sorted)
/// and all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderSets {
    /// The groups they are in, nested groups and groups of the roles and orgs they hold included.
    pub groups: Vec<Id>,
    /// The places whose pages let them view without giving them a role there.
    pub viewers: Vec<Id>,
    /// Every identity they hold, for a statement that needs them all.
    pub held: Held,
    /// What they were worked out under.
    pub stamp: Stamp,
}

/// Every identity `access_holders` finds for a person, ordered by kind and identifier, as arrays a statement binds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Held {
    /// `user`, `members`, `group`, `role`, `visible` or `reachable`.
    pub kinds: Vec<String>,
    /// The person, the org, the group or the place.
    pub ids: Vec<Id>,
    /// The highest role for a role, `viewer` for a place of page-only viewing, else empty.
    pub roles: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct Row {
    holds: bool,
    groups: Vec<Id>,
    viewers: Vec<Id>,
    kinds: Vec<String>,
    ids: Vec<Id>,
    roles: Vec<String>,
    database: i64,
    snapshot: String,
    keepable: bool,
    valid_from: Option<DateTime<Utc>>,
    valid_until: Option<DateTime<Utc>>,
}

/// The holder sets of `user` at `at` (`system`: whether their flag of system admin counts), in one statement:
/// none when the sets stamped `kept` still hold for it, else worked out now under its snapshot, which is the new
/// stamp's. Sets still hold when they were worked out in the same database; every transaction their snapshot saw is
/// seen now; and no transaction that wrote what they are worked out from (`access_changes`) is seen now that their
/// snapshot did not see, a running transaction's own writes included. A snapshot whose transactions' records may have
/// been removed (`access_changes_floor`) holds nothing. Whether nothing begins or stops counting between the two
/// moments is the caller's to ask first ([`Stamp::covers`]). One statement either way, as the one it replaces.
pub async fn holder_sets(
    conn: &mut Conn,
    user: Id,
    at: DateTime<Utc>,
    system: bool,
    kept: Option<&Stamp>,
) -> Result<Option<HolderSets>, DbError> {
    let row: Row = sqlx::query_as(
        "WITH kept AS MATERIALIZED (
             SELECT $4::text IS NOT NULL
                AND COALESCE((SELECT oid::int8 FROM pg_database WHERE datname = current_database()) = $5, false)
                AND pg_snapshot_xmin($4::pg_snapshot) > COALESCE((SELECT floor FROM access_changes_floor WHERE id), '0'::xid8)
                AND pg_snapshot_xmax($4::pg_snapshot) <= pg_snapshot_xmax(pg_current_snapshot())
                AND NOT EXISTS (SELECT 1 FROM pg_snapshot_xip(pg_current_snapshot()) AS running(xid)
                                 WHERE pg_visible_in_snapshot(running.xid, $4::pg_snapshot))
                AND NOT EXISTS (SELECT 1 FROM access_changes c WHERE c.xid >= pg_snapshot_xmax($4::pg_snapshot))
                AND NOT EXISTS (SELECT 1 FROM access_changes c
                                 WHERE c.xid = ANY (ARRAY(SELECT pg_snapshot_xip($4::pg_snapshot)))) AS holds
         ), held AS MATERIALIZED (
             -- Not worked out at all where the kept sets hold: a filter of the whole scan, decided once.
             SELECT kind, id, role FROM access_holders($1, $2, $3) WHERE NOT (SELECT holds FROM kept)
         )
         SELECT (SELECT holds FROM kept) AS holds,
                COALESCE((SELECT array_agg(id ORDER BY id) FROM held WHERE kind = 'group'), '{}') AS groups,
                COALESCE((SELECT array_agg(id ORDER BY id) FROM held WHERE kind = 'visible'), '{}') AS viewers,
                COALESCE((SELECT array_agg(kind ORDER BY kind, id) FROM held), '{}') AS kinds,
                COALESCE((SELECT array_agg(id ORDER BY kind, id) FROM held), '{}') AS ids,
                COALESCE((SELECT array_agg(role ORDER BY kind, id) FROM held), '{}') AS roles,
                (SELECT oid::int8 FROM pg_database WHERE datname = current_database()) AS database,
                pg_current_snapshot()::text AS snapshot,
                pg_current_xact_id_if_assigned() IS NULL AS keepable,
                greatest(
                  (SELECT max(expires_at) FROM node_roles
                    WHERE removed_at IS NULL AND expires_at IS NOT NULL AND expires_at <= $2),
                  (SELECT max(expires_at) FROM permission_entries
                    WHERE removed_at IS NULL AND expires_at IS NOT NULL AND object_kind IN ('system','node')
                      AND expires_at <= $2)) AS valid_from,
                least(
                  (SELECT min(expires_at) FROM node_roles
                    WHERE removed_at IS NULL AND expires_at IS NOT NULL AND expires_at > $2),
                  (SELECT min(expires_at) FROM permission_entries
                    WHERE removed_at IS NULL AND expires_at IS NOT NULL AND object_kind IN ('system','node')
                      AND expires_at > $2)) AS valid_until",
    )
    .bind(user)
    .bind(at)
    .bind(system)
    .bind(kept.map(|stamp| stamp.snapshot.as_str()))
    .bind(kept.map(|stamp| stamp.database))
    .fetch_one(conn)
    .await?;
    if row.holds {
        return Ok(None);
    }
    Ok(Some(HolderSets {
        groups: row.groups,
        viewers: row.viewers,
        held: Held {
            kinds: row.kinds,
            ids: row.ids,
            roles: row.roles,
        },
        stamp: Stamp {
            database: row.database,
            snapshot: row.snapshot,
            from: row.valid_from,
            until: row.valid_until,
            keepable: row.keepable,
        },
    }))
}

/// How long a record of a change is kept at least, by the database's clock that wrote it.
pub const RECORDS_KEPT_SECS: i64 = 86_400;

/// Removes the records of changes written over [`RECORDS_KEPT_SECS`] ago that every snapshot now sees, raising
/// the floor below which stamps hold nothing; returns how many were removed.
pub async fn prune(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT access_changes_prune(make_interval(secs => $1::float8))")
            .bind(RECORDS_KEPT_SECS)
            .fetch_one(conn)
            .await?,
    )
}
