//! Authorization consistency per root org, request lock classes, and the identity a write acts as.
//!
//! There is no barrier that every request shares, and no request waits for another request's work. A
//! write reads the authorization epochs it decides under before it reads any permission fact: the
//! system's, those of the root orgs the person is a member of and the root org of the request's place,
//! in the statement that locks its identity, and the root org of every node a permission decision
//! reads, in the statement that reads that node (`hierarchy::with_ancestors`). Its commit checks, as
//! its last statement, that none of them moved (`orch_commit_work`, a deferred trigger of the schema):
//! a change of authority that committed meanwhile refuses the commit with a retryable error
//! (`DbError::Retry`), and one that commits later is ordered after it. So a revoked person's write
//! never commits after the revoke, and every request reads permission facts after any change that
//! committed before its commit.
//!
//! A change of who may do what moves, at its commit, the epochs of the root orgs it changes: the root
//! org of its request's place where it is about one place ([`Place`]), the system's otherwise (and for
//! background work that names none), and checks its own decisions the same way. Roles, pages,
//! membership, standing and shared availability/policy changes move them through
//! `access::bump_epoch`, which also moves the cache epoch; creation moves only the cache epoch and
//! pins its existing parent through its foreign key. The epochs are locked only inside a commit, in
//! one order (by org id, the system's nil id first), so a lock on one lasts as long as a commit.
//!
//! Personal credential writes exclusively lock their own user and watch no epoch, so even a slow
//! recovery never makes another person wait; they change no shared permission page. A write still
//! locks its own user, session and token rows while it runs, as before: revoking a session or
//! suspending a person waits for that person's open writes only.

use super::identity::UserRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;
use sqlx::Connection;
use std::future::Future;

/// The system's authorization epoch: the row a change of the whole installation moves.
pub const SYSTEM: Id = Id::nil();
/// The epoch of the system's administrators as a set: what a change of a person's standing in the
/// whole system (a system admin given or taken, a person suspended) moves and checks, so that two
/// such changes are ordered (the last admin stays) without refusing anybody else's work. That
/// person's own writes are ordered by their user row, which such a change locks.
pub const STANDING: Id = Id::from_u128(1);

/// The lock class declared before a request starts a transaction.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    /// An ordinary content action.
    Action,
    /// Ordinary background/tool work without the HTTP guard's combined identity statement.
    Shared,
    /// A change to existing shared authority or availability.
    Authority,
    /// A new object and its initial page, under its parent's current authority.
    Creation,
    /// The actor's own credentials; anonymous flows find their person before writing.
    Personal {
        /// The authenticated actor, where known at the request's door.
        user: Option<Id>,
    },
}

tokio::task_local! {
    static REQUEST_SCOPE: Scope;
}

/// Runs a handler whose transactions are declared authority changes, or ordinary actions.
/// Spawned background tasks must declare their own authority transaction.
pub async fn scope<T>(authority_change: bool, work: impl Future<Output = T>) -> T {
    request_scope(
        if authority_change {
            Scope::Authority
        } else {
            Scope::Action
        },
        work,
    )
    .await
}

/// Runs a request with its declared authority, creation or personal-credential lock class.
pub async fn request_scope<T>(scope: Scope, work: impl Future<Output = T>) -> T {
    REQUEST_SCOPE.scope(scope, work).await
}

pub(crate) fn personal_user() -> Option<Id> {
    match REQUEST_SCOPE.try_with(|scope| *scope).ok() {
        Some(Scope::Personal { user }) => user,
        _ => None,
    }
}

/// Applies a request's lock class when its transaction begins. Nothing here waits for another
/// transaction except a personal credential's own user row.
pub(crate) async fn begin_scope(conn: &mut Conn) -> Result<(), DbError> {
    match REQUEST_SCOPE.try_with(|scope| *scope).ok() {
        Some(Scope::Authority) => lock_authority(conn).await?,
        Some(Scope::Shared) => lock_action(conn).await?,
        Some(Scope::Creation) => {
            lock_action(conn).await?;
            mark_scope(conn, "birth").await?;
        }
        Some(Scope::Personal { user }) => {
            mark_scope(conn, "personal").await?;
            if let Some(user) = user {
                let _ = lock_person(conn, user).await?;
            }
        }
        Some(Scope::Action) | None => {}
    }
    Ok(())
}

async fn mark_scope(conn: &mut Conn, mode: &'static str) -> Result<(), DbError> {
    let _: String = sqlx::query_scalar("SELECT set_config('orch.guard_epoch', $1, true)")
        .bind(mode)
        .fetch_one(conn)
        .await?;
    Ok(())
}

/// Pins a personal credential writer against every action authenticated as that person.
pub async fn lock_person(conn: &mut Conn, user: Id) -> Result<Option<UserRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM users WHERE id = $1 FOR UPDATE")
            .bind(user)
            .fetch_optional(conn)
            .await?,
    )
}

/// Pins an existing parent for a child/reference without serializing its other makers.
pub async fn lock_parent(conn: &mut Conn, node: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, Id>(
        "SELECT id FROM nodes WHERE id = $1 AND deleted_at IS NULL FOR KEY SHARE",
    )
    .bind(node)
    .fetch_optional(conn)
    .await?
    .is_some())
}

/// Whether the connection is inside an explicit transaction.
#[must_use]
pub fn in_transaction(conn: &mut Conn) -> bool {
    conn.is_in_transaction()
}

/// Makes an ordinary background action's commit check the system's authorization epoch, and the
/// epoch of the root org of every node its permission decisions read, keeping a stronger class
/// already declared in this transaction. Locks nothing.
pub async fn lock_action(conn: &mut Conn) -> Result<(), DbError> {
    let marked: Option<String> = sqlx::query_scalar(
        "SELECT set_config('orch.guard_epoch',
             CASE WHEN current_setting('orch.guard_epoch', true) IN ('write','birth','personal')
                  THEN current_setting('orch.guard_epoch', true) ELSE 'read' END, true)
          WHERE orch_authz_saw(orch_authz_epochs('{}'))",
    )
    .fetch_optional(&mut *conn)
    .await?;
    if marked.is_none() {
        return Err(DbError::Rule {
            constraint: "authorization_epoch_missing".into(),
        });
    }
    Ok(())
}

/// Declares that this transaction may change authority. Locks nothing: what it changes moves the
/// epochs at its commit (`access::bump_epoch`), and its commit checks the epochs it decided under,
/// as an action's does (the system's from here, and each place's its decisions read). An action
/// or a creation that turns out to change authority becomes one here; a personal credential
/// transaction cannot.
pub async fn lock_authority(conn: &mut Conn) -> Result<(), DbError> {
    let locked: Option<String> = sqlx::query_scalar(
        "SELECT set_config('orch.guard_epoch', 'write', true)
          WHERE COALESCE(current_setting('orch.guard_epoch', true), '') <> 'personal'
            AND orch_authz_saw(orch_authz_epochs('{}'))",
    )
    .fetch_optional(conn)
    .await?;
    if locked.is_none() {
        return Err(DbError::Rule {
            constraint: "authority_change_after_shared_guard".into(),
        });
    }
    Ok(())
}

/// Fences delegated authority read from another person's standing, without
/// serializing unrelated users. Personal-credential transactions cannot redeem
/// invitations; their caller must begin the appropriate creation/action scope.
pub async fn watch_standing(conn: &mut Conn) -> Result<(), DbError> {
    let watched: Option<bool> = sqlx::query_scalar(
        "SELECT orch_authz_saw(orch_authz_epochs(ARRAY[$1]::uuid[]))
          WHERE current_setting('orch.guard_epoch',true) IN ('read','write','birth')",
    )
    .bind(STANDING)
    .fetch_optional(conn)
    .await?;
    if watched != Some(true) {
        return Err(DbError::Rule {
            constraint: "authorization_epoch_missing".into(),
        });
    }
    Ok(())
}

/// Adds the root orgs of `nodes` to the epochs this transaction's commit checks, where it checks any
/// (a guarded write): for a decision about a node that no statement of `hierarchy` read.
pub async fn watch_nodes(conn: &mut Conn, nodes: &[Id]) -> Result<(), DbError> {
    let _: Option<bool> = sqlx::query_scalar(
        "SELECT orch_authz_saw(orch_authz_epochs(ARRAY(SELECT DISTINCT org_id FROM nodes WHERE id = ANY($1))))
          WHERE COALESCE(current_setting('orch.authz_seen', true), '') <> ''",
    )
    .bind(nodes)
    .fetch_optional(conn)
    .await?;
    Ok(())
}

/// Orders this change of authority after every other one that moves `epoch` (a change of a
/// person's standing, [`STANDING`]): the epoch's row is locked until commit, which no other kind of
/// write ever waits for, and its value is read after that wait for the commit's check.
pub async fn hold_epoch(conn: &mut Conn, epoch: Id) -> Result<(), DbError> {
    let _: Option<bool> = sqlx::query_scalar(
        "SELECT orch_authz_saw(jsonb_build_object(e.org_id::text, e.epoch))
           FROM (SELECT org_id, epoch FROM authz_epochs WHERE org_id = $1 FOR NO KEY UPDATE) e",
    )
    .bind(epoch)
    .fetch_optional(conn)
    .await?;
    Ok(())
}

/// The root org of a request's place, as one SQL expression over the parameters `$1` (node), `$2`
/// (document), `$3` (resource table) and `$4` (resource id).
const PLACE_ROOT: &str = "COALESCE((SELECT n.org_id FROM nodes n WHERE n.id = $1),
         (SELECT d.org_id FROM documents d WHERE d.id = $2),
         CASE $3::text
           WHEN 'chats' THEN (SELECT x.org_id FROM chats x WHERE x.id = $4)
           WHEN 'providers' THEN (SELECT x.org_id FROM providers x WHERE x.id = $4)
           WHEN 'agents' THEN (SELECT x.org_id FROM agents x WHERE x.id = $4)
           WHEN 'environments' THEN (SELECT x.org_id FROM environments x WHERE x.id = $4)
           WHEN 'hosts' THEN (SELECT x.org_id FROM hosts x WHERE x.id = $4)
           WHEN 'machines' THEN (SELECT x.org_id FROM machines x WHERE x.id = $4)
           WHEN 'env_templates' THEN (SELECT x.org_id FROM env_templates x WHERE x.id = $4)
           WHEN 'mcp_servers' THEN (SELECT x.org_id FROM mcp_servers x WHERE x.id = $4)
           WHEN 'mounts' THEN (SELECT x.org_id FROM mounts x WHERE x.id = $4)
           WHEN 'skills' THEN (SELECT x.org_id FROM skills x WHERE x.id = $4)
         END)";

/// Orders this change of authority after every other one of the same scope (the root org of its
/// place where it is about one place, else the system) before it reads anything, as the shared
/// barrier ordered them all before: two admins changing one org do not refuse each other. Only
/// changes of authority take it, at their start, so no write ever waits for it; it is held until
/// commit (`pg_advisory_xact_lock`).
pub async fn hold_change(conn: &mut Conn, place: &Place) -> Result<(), DbError> {
    let query = sqlx::AssertSqlSafe(format!(
        "SELECT pg_advisory_xact_lock(hashtextextended('authz.change:' || COALESCE(CASE WHEN $5 THEN {PLACE_ROOT} END,
                    '00000000-0000-0000-0000-000000000000'::uuid)::text, 0))"
    ));
    sqlx::query(query)
        .bind(place.node)
        .bind(place.document)
        .bind(place.resource.map(|(table, _)| table))
        .bind(place.resource.map(|(_, id)| id))
        .bind(place.alone)
        .execute(conn)
        .await?;
    Ok(())
}

/// Makes this transaction's commit refuse (retryably) when the cache epoch is no longer `epoch`:
/// for background work that decided under it before its transaction began. Locks nothing now.
pub async fn expect_cache_epoch(conn: &mut Conn, epoch: i64) -> Result<(), DbError> {
    let _: bool = sqlx::query_scalar(
        "SELECT set_config('orch.cache_expect', $1::text, true) IS NOT NULL AND orch_commit_arm()",
    )
    .bind(epoch)
    .fetch_one(conn)
    .await?;
    Ok(())
}

/// The credential authenticated when the request began, pinned to its user.
#[derive(Debug, Clone, Copy)]
pub enum Credential<'a> {
    /// A browser access token and its session.
    Session {
        /// The session originally authenticated.
        id: Id,
        /// The hash of the bearer access token.
        hash: &'a [u8],
    },
    /// An API token whose scopes must be read again.
    Token {
        /// The token originally authenticated.
        id: Id,
        /// The hash of the bearer API token.
        hash: &'a [u8],
    },
}

/// Where a request acts, as its path names it: what its commit checks beside the person's own
/// orgs, and, for a change of authority about one place, the one root org it moves.
#[derive(Debug, Clone, Copy, Default)]
pub struct Place {
    /// The node the path names.
    pub node: Option<Id>,
    /// The document the path names.
    pub document: Option<Id>,
    /// The resource the path names: its table and its id.
    pub resource: Option<(&'static str, Id)>,
    /// Whether a change of authority made by the request is about this place's root org alone
    /// (otherwise, or where the place has none, it moves the system's epoch).
    pub alone: bool,
    /// The one epoch a change of authority made by the request moves instead ([`STANDING`]).
    pub epoch: Option<Id>,
    /// Whether a change of authority made by the request only adds authority (an invitation made
    /// or taken, a member, a group or a token added): it moves no authorization epoch, since no
    /// decision made before it can be wrong after it; the cache epoch still moves.
    pub grants: bool,
}

/// The table of a resource kind as a path names it, for [`Place::resource`].
#[must_use]
pub fn resource_table(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "chat" | "chats" => "chats",
        "provider" | "providers" => "providers",
        "agent" | "agents" => "agents",
        "environment" | "environments" => "environments",
        "host" | "hosts" => "hosts",
        "machine" | "machines" => "machines",
        "template" | "templates" => "env_templates",
        "mcp_server" | "mcp-servers" | "mcp_servers" => "mcp_servers",
        "mount" | "mounts" => "mounts",
        "skill" | "skills" => "skills",
        _ => return None,
    })
}

/// Current identity facts, with the rows locked until the transaction ends.
#[derive(Debug, sqlx::FromRow)]
pub struct Identity {
    /// The exact session's mode; always false for an API token.
    pub as_admin: bool,
    /// The current user, including suspension and administrator state.
    #[sqlx(flatten)]
    pub user: UserRow,
    /// API token scopes; none for a browser session.
    pub scopes: Option<Vec<String>>,
    /// The API token's current subtree boundary.
    pub node_id: Option<Id>,
    /// The first end of the credential or its session; none for a lasting token.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Locks the user and credential for a read (a frame's projection), returning no revoked
/// credential; the caller checks expiration with its clock. Watches no epoch: a read commits nothing.
pub async fn lock_identity(
    conn: &mut Conn,
    user: Id,
    credential: Credential<'_>,
    authority_change: bool,
) -> Result<Option<Identity>, DbError> {
    identity_in(conn, user, credential, authority_change, None).await
}

/// Locks the user and credential of a write in one statement that also reads the authorization
/// epochs its commit checks (the system's, the person's root orgs' and `place`'s), before any
/// permission fact is read; returns no revoked credential.
pub async fn lock_identity_for_write(
    conn: &mut Conn,
    user: Id,
    credential: Credential<'_>,
    authority_change: bool,
    place: Place,
) -> Result<Option<Identity>, DbError> {
    identity_in(conn, user, credential, authority_change, Some(place)).await
}

async fn identity_in(
    conn: &mut Conn,
    user: Id,
    credential: Credential<'_>,
    authority_change: bool,
    place: Option<Place>,
) -> Result<Option<Identity>, DbError> {
    // PostgreSQL's row-lock conflict-table lines:
    // SHARE conflicts with NO KEY UPDATE and UPDATE; SHARE and KEY SHARE coexist.
    // KEY SHARE conflicts only with UPDATE.
    // Session end, token scopes/end and access-token expiry are non-key updates,
    // so they require SHARE. Suspension/admin writers explicitly lock users FOR
    // UPDATE; KEY SHARE also permits inbox counters and foreign-key references.
    // Row locking rechecks tuples changed during a wait; later permission statements
    // obtain new snapshots after that wait.
    let personal = matches!(
        REQUEST_SCOPE.try_with(|scope| *scope),
        Ok(Scope::Personal { .. })
    );
    // The epochs are read in this statement's snapshot, before any statement that reads a permission fact.
    let watched = match place {
        Some(_) if !personal => {
            "SELECT orch_authz_saw(orch_authz_epochs(
                        ARRAY(SELECT m.org_id FROM org_members m WHERE m.user_id = $1) || ARRAY[p.root, $9::uuid])),
                    CASE WHEN $10 THEN set_config('orch.authz_scope', '\"grants\"', true)
                         WHEN $9::uuid IS NOT NULL AND $6
                         THEN set_config('orch.authz_scope', jsonb_build_array($9::uuid)::text, true)
                         WHEN $6 AND p.root IS NOT NULL
                         THEN set_config('orch.authz_scope', jsonb_build_array(p.root)::text, true) END
               FROM (SELECT COALESCE((SELECT n.org_id FROM nodes n WHERE n.id = $4),
                                     (SELECT d.org_id FROM documents d WHERE d.id = $5),
                                     CASE $7::text
                                       WHEN 'providers' THEN (SELECT x.org_id FROM providers x WHERE x.id = $8)
                                       WHEN 'agents' THEN (SELECT x.org_id FROM agents x WHERE x.id = $8)
                                       WHEN 'environments' THEN (SELECT x.org_id FROM environments x WHERE x.id = $8)
                                       WHEN 'hosts' THEN (SELECT x.org_id FROM hosts x WHERE x.id = $8)
                                       WHEN 'machines' THEN (SELECT x.org_id FROM machines x WHERE x.id = $8)
                                       WHEN 'env_templates' THEN (SELECT x.org_id FROM env_templates x WHERE x.id = $8)
                                       WHEN 'mcp_servers' THEN (SELECT x.org_id FROM mcp_servers x WHERE x.id = $8)
                                       WHEN 'chats' THEN (SELECT x.org_id FROM chats x WHERE x.id = $8)
                                       WHEN 'mounts' THEN (SELECT x.org_id FROM mounts x WHERE x.id = $8)
                                       WHEN 'skills' THEN (SELECT x.org_id FROM skills x WHERE x.id = $8)
                                     END) AS root) p"
        }
        // A read, or a personal credential: no epoch (the parameters are named all the same).
        _ => "SELECT $4::uuid IS NULL OR $5::uuid IS NULL OR $6::boolean OR $7::text IS NULL
                     OR $8::uuid IS NULL OR $9::uuid IS NULL OR $10::boolean OR true",
    };
    let user_lock = if personal { "UPDATE" } else { "KEY SHARE" };
    let (identity, id, hash) = match credential {
        Credential::Session { id, hash } => (
            "SELECT u.*, s.as_admin, NULL::text[] AS scopes, NULL::uuid AS node_id,
                    LEAST(a.expires_at, s.expires_at) AS expires_at
               FROM marked CROSS JOIN users u
               JOIN sessions s ON s.user_id = u.id
               JOIN access_tokens a ON a.session_id = s.id
              WHERE u.id = $1 AND s.id = $2 AND a.token_hash = $3
                AND s.revoked_at IS NULL
              FOR {user_lock} OF u FOR SHARE OF s, a",
            id,
            hash,
        ),
        Credential::Token { id, hash } => (
            "SELECT u.*, false AS as_admin, t.scopes, t.node_id, t.expires_at
               FROM marked CROSS JOIN users u
               JOIN api_tokens t ON t.user_id = u.id
              WHERE u.id = $1 AND t.id = $2 AND t.token_hash = $3
                AND t.revoked_at IS NULL
              FOR {user_lock} OF u FOR SHARE OF t",
            id,
            hash,
        ),
    };
    let mode = if authority_change {
        "write"
    } else {
        match REQUEST_SCOPE.try_with(|scope| *scope).ok() {
            Some(Scope::Creation) => "birth",
            Some(Scope::Personal { .. }) => "personal",
            _ => "read",
        }
    };
    let identity = identity.replace("{user_lock}", user_lock);
    // Only the fixed statements above are composed, never request text.
    let query = sqlx::AssertSqlSafe(format!(
        "WITH watched AS MATERIALIZED ({watched}), marked AS MATERIALIZED (
             SELECT set_config('orch.guard_epoch',
                 CASE WHEN current_setting('orch.guard_epoch', true) IN ('write','birth','personal')
                      THEN current_setting('orch.guard_epoch', true) ELSE '{mode}' END, true) FROM watched
         ) {identity}"
    ));
    let place = place.unwrap_or_default();
    let identity: Option<Identity> = sqlx::query_as(query)
        .bind(user)
        .bind(id)
        .bind(hash)
        .bind(place.node)
        .bind(place.document)
        .bind((place.alone || place.epoch.is_some()) && authority_change)
        .bind(place.resource.map(|(table, _)| table))
        .bind(place.resource.map(|(_, id)| id))
        .bind(place.epoch)
        .bind(place.grants && authority_change)
        .fetch_optional(conn)
        .await?;
    if let Some(identity) = &identity {
        crate::audit::set_admin_mode(user, identity.as_admin);
    }
    Ok(identity)
}
