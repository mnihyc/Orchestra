//! Users and their state, each with an org of their own, what the interface keeps for them, sessions, tokens, identity providers, invitations, the browsers a person signed in from, codes sent by mail, and the counts of wrong tries by name.
//!
//! A token is never stored: what is kept and looked up is its SHA-256, and
//! so is the code that was mailed to a person (together with their
//! identifier). Every moment written here is given by the caller, who gets it
//! from the application's clock: a default of the database would be the
//! system's own time, which a test cannot move.
//!
//! A statement that decides something is one statement. The counts of wrong
//! tries are reserved before the proof is looked at, and given back when it
//! was right, so that a thousand tries at once cannot all pass before the
//! first of them is counted; a code that is spent, and a time step of an
//! authenticator that is accepted, are changes that happen only where the row
//! still says they may.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::{Id, Json};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

/// An active human for bootstrapping a read-only authorization frame when all ownership seeds have gone.
pub async fn first_active_human(conn: &mut Conn) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM users WHERE kind='human' AND status='active' ORDER BY id LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// How many colours an avatar is drawn in.
const AVATAR_COLORS: u64 = 12;

/// How many bytes of the digest of an identifier decide the colour of an
/// avatar: eight make one number of 64 bits.
const AVATAR_DIGEST_BYTES: usize = 8;

/// The key under which first setups are made one at a time: two requests that
/// both find no user must not both make the first administrator. Any constant
/// will do; this one spells the purpose.
const SETUP_LOCK: i64 = 0x6f72_6368_5f73_7475;

/// The key under which changes that could leave the system without a system
/// admin are made one at a time: two requests that each unmake or suspend a
/// different one of two admins must not both go through. Any constant will
/// do; this one spells the purpose.
const ADMINS_LOCK: i64 = 0x6f72_6368_5f61_646d;

/// A user, as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct UserRow {
    /// Its identifier.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The org a service identity belongs to.
    pub service_org_id: Option<Id>,
    /// The name it signs in by.
    pub username: String,
    /// The name people see.
    pub display_name: String,
    /// Its address.
    pub email: Option<String>,
    /// When that address was confirmed.
    pub email_verified_at: Option<DateTime<Utc>>,
    /// The Argon2id hash of its password; none until one is set.
    pub password_hash: Option<String>,
    /// The sealed secret of its authenticator; none when never enrolled.
    pub totp_secret: Option<Vec<u8>>,
    /// When the authenticator was switched on.
    pub totp_enabled_at: Option<DateTime<Utc>>,
    /// The last time step accepted, so that a code is used once.
    pub totp_last_step: Option<i64>,
    /// When a mailed code became its second step.
    pub email_code_at: Option<DateTime<Utc>>,
    /// Whether it is an admin of the system.
    pub is_system_admin: bool,
    /// When a system admin gave it an invitation to make one team org that it has not used.
    #[sqlx(default)]
    pub org_invited_at: Option<DateTime<Utc>>,
    /// `active` or `suspended`.
    pub status: String,
    /// The colour of its avatar, 0 to 11.
    pub avatar_color: i16,
    /// Its language.
    pub locale: String,
    /// Its time zone, an IANA name.
    pub timezone: String,
    /// `device` (its browser reports the zone) or `fixed` (the zone was chosen).
    pub timezone_mode: String,
    /// When a device last reported the zone.
    pub timezone_reported_at: Option<DateTime<Utc>>,
    /// Its theme.
    pub theme: String,
    /// Its density.
    pub density: String,
    /// The org that is its own.
    pub personal_org_id: Option<Id>,
    /// The sequence of its own feed.
    pub inbox_seq: i64,
    /// Who created it, where somebody did.
    pub created_by: Option<Id>,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
    /// When it was last seen.
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl UserRow {
    /// Whether the user is suspended everywhere.
    #[must_use]
    pub fn suspended(&self) -> bool {
        self.status == "suspended"
    }

    /// Whether the address they have is confirmed.
    #[must_use]
    pub fn email_confirmed(&self) -> bool {
        self.email.is_some() && self.email_verified_at.is_some()
    }

    /// Whether an authenticator is their second step.
    #[must_use]
    pub fn totp_on(&self) -> bool {
        self.totp_enabled_at.is_some()
    }

    /// Whether a mailed code is their second step.
    #[must_use]
    pub fn email_code_on(&self) -> bool {
        self.email_code_at.is_some()
    }
}

/// The user with the identifier `id`.
pub async fn user_by_id(conn: &mut Conn, id: Id) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// The person a name stands for: a username or an address, either way
/// without regard to case. A service identity signs in nowhere, so it is
/// nobody here.
pub async fn user_by_login(conn: &mut Conn, name: &str) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM users
          WHERE kind = 'human' AND (username = lower($1) OR lower(email) = lower($1))
          LIMIT 1",
    )
    .bind(name)
    .fetch_optional(conn)
    .await?)
}

/// The person whose username is `name`, without regard to case: only the
/// name they sign in by, and not an address that might be another person's.
/// A service identity signs in nowhere, so it is nobody here.
pub async fn user_by_username(conn: &mut Conn, name: &str) -> Result<Option<UserRow>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM users WHERE kind = 'human' AND username = lower($1)")
            .bind(name)
            .fetch_optional(conn)
            .await?,
    )
}

/// The system admin who can sign in and was made first: whom a thing that
/// the host sets up is attributed to, where the schema asks for a person.
pub async fn first_system_admin(conn: &mut Conn) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM users
          WHERE kind = 'human' AND is_system_admin AND status = 'active'
          ORDER BY created_at, id
          LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// The person whose confirmed address is `email`, without regard to case.
pub async fn user_by_confirmed_email(
    conn: &mut Conn,
    email: &str,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM users
          WHERE kind = 'human' AND lower(email) = lower($1) AND email_verified_at IS NOT NULL",
    )
    .bind(email)
    .fetch_optional(conn)
    .await?)
}

/// Whether the system has no user at all: the first administrator is still
/// to be made.
pub async fn no_users_yet(conn: &mut Conn) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM users)")
            .fetch_one(conn)
            .await?,
    )
}

/// Holds the setup lock until the transaction ends: whoever makes the first
/// administrator holds it from before they look for users to after they
/// made one.
pub async fn lock_setup(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SETUP_LOCK)
        .execute(conn)
        .await?;
    Ok(())
}

/// The colour of the avatar of the user `id`: the first eight bytes of the
/// SHA-256 of the identifier as a number, modulo the colours there are. It is
/// worked out once, when the user is made.
#[must_use]
pub fn avatar_color(id: Id) -> i16 {
    let digest = Sha256::digest(id.as_bytes());
    let number = digest
        .iter()
        .take(AVATAR_DIGEST_BYTES)
        .fold(0_u64, |number, byte| (number << 8) | u64::from(*byte));
    i16::try_from(number % AVATAR_COLORS).unwrap_or_default()
}

/// The optional personal organization created with an account.
#[derive(Debug, Clone, Copy)]
pub struct PersonalOrg {
    /// Whether it takes no member beyond its person.
    pub closed: bool,
    /// Whether it holds no organization inside it.
    pub flat: bool,
}

/// A person to make.
#[derive(Debug, Clone, Copy)]
pub struct NewUser<'a> {
    /// The identifier, made by the caller.
    pub id: Id,
    /// The username, which is also the short name of their own org.
    pub username: &'a str,
    /// The name people see; the name their own org starts with.
    pub display_name: &'a str,
    /// Their address.
    pub email: Option<&'a str>,
    /// Whether that address counts as confirmed from the start.
    pub email_confirmed: bool,
    /// The Argon2id hash of their password.
    pub password_hash: Option<&'a str>,
    /// Their language.
    pub locale: &'a str,
    /// Whether they administer the system.
    pub is_system_admin: bool,
    /// Who made them, where somebody did.
    pub created_by: Option<Id>,
    /// Their personal organization, only when account policy enables creation.
    pub personal_org: Option<PersonalOrg>,
}

/// A person who was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Created {
    /// The user as stored.
    pub user: UserRow,
    /// Their own org, when one was requested.
    pub personal_org_id: Option<Id>,
}

/// Makes a person and, when requested, their personal org in the caller's transaction:
/// the user, the org's node and its side row (a root org whose short name is
/// the username and whose name is theirs), and their membership of it. The
/// org starts empty. Naming them its owner is the access engine's, and the
/// caller does it in the same transaction.
/// Every new person follows their device, retaining UTC until their first validated zone report.
///
/// A username that an org already has as its short name, or that a person
/// already has, is refused as a unique value taken.
pub async fn create_user(
    conn: &mut Conn,
    new: &NewUser<'_>,
    at: DateTime<Utc>,
) -> Result<Created, DbError> {
    sqlx::query(
        "INSERT INTO users (id, kind, username, display_name, email, email_verified_at, password_hash,
                            is_system_admin, avatar_color, locale, created_by, created_at, updated_at, timezone_mode)
         VALUES ($1, 'human', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11, 'device')",
    )
    .bind(new.id)
    .bind(new.username)
    .bind(new.display_name)
    .bind(new.email)
    .bind(
        new.email
            .filter(|_| new.email_confirmed)
            .map(|_| at),
    )
    .bind(new.password_hash)
    .bind(new.is_system_admin)
    .bind(avatar_color(new.id))
    .bind(new.locale)
    .bind(new.created_by)
    .bind(at)
    .execute(&mut *conn)
    .await?;

    let org = if let Some(options) = new.personal_org {
        let org = orch_types::new_id();
        sqlx::query(
            "INSERT INTO nodes (id, kind, parent_id, org_id, path, name, owner_id, created_by, created_at, updated_at)
             VALUES ($1, 'org', NULL, $1, ARRAY[$1]::uuid[], $2, $3, $3, $4, $4)",
        )
        .bind(org)
        .bind(new.display_name)
        .bind(new.id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE users SET personal_org_id = $2 WHERE id = $1")
            .bind(new.id)
            .bind(org)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO orgs (node_id, parent_id, slug, org_kind, closed, flat)
             VALUES ($1, NULL, $2, 'personal', $3, $4)",
        )
        .bind(org)
        .bind(new.username)
        .bind(options.closed)
        .bind(options.flat)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO org_members (org_id, user_id, domain_id, joined_at) VALUES ($1, $2, $1, $3)",
        )
        .bind(org)
        .bind(new.id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
        // Every org has its chat, made with it.
        sqlx::query(
            "INSERT INTO chats (id, kind, node_id, org_id, created_by, created_at)
             VALUES ($1, 'node', $2, $2, $3, $4)",
        )
        .bind(orch_types::new_id())
        .bind(org)
        .bind(new.id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
        Some(org)
    } else {
        None
    };

    let user = user_by_id(conn, new.id)
        .await?
        .ok_or_else(|| DbError::Other(sqlx::Error::RowNotFound))?;
    Ok(Created {
        user,
        personal_org_id: org,
    })
}

/// What a person changes in how they appear and how they like the interface.
/// A member left out stays as it is.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProfileChange<'a> {
    /// The name to show.
    pub display_name: Option<&'a str>,
    /// The language.
    pub locale: Option<&'a str>,
    /// The time zone.
    pub timezone: Option<&'a str>,
    /// How the time zone is chosen: `device` or `fixed`.
    pub timezone_mode: Option<&'a str>,
    /// The colour scheme.
    pub theme: Option<&'a str>,
    /// How closely the interface is set.
    pub density: Option<&'a str>,
}

/// Changes how `user` appears and what they like, and returns them as they
/// are now.
pub async fn update_profile(
    conn: &mut Conn,
    user: Id,
    change: &ProfileChange<'_>,
    at: DateTime<Utc>,
) -> Result<Option<UserRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE users SET display_name = COALESCE($2, display_name), locale = COALESCE($3, locale),
                          timezone = COALESCE($4, timezone), theme = COALESCE($5, theme),
                          density = COALESCE($6, density), updated_at = $7,
                          timezone_mode = COALESCE($8, timezone_mode)
          WHERE id = $1
      RETURNING *",
    )
    .bind(user)
    .bind(change.display_name)
    .bind(change.locale)
    .bind(change.timezone)
    .bind(change.theme)
    .bind(change.density)
    .bind(at)
    .bind(change.timezone_mode)
    .fetch_optional(conn)
    .await?)
}

/// Starts a new person's time zone from their device: the zone their browser gave, or UTC until a
/// browser of theirs reports one. Only a person (never a service identity) follows a device.
pub async fn start_device_timezone(
    conn: &mut Conn,
    user: Id,
    zone: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET timezone = COALESCE($2::text, timezone), timezone_mode = 'device',
                          timezone_reported_at = CASE WHEN $2::text IS NULL THEN NULL ELSE $3 END,
                          updated_at = $3
          WHERE id = $1 AND kind = 'human'",
    )
    .bind(user)
    .bind(zone)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// A browser's report of its time zone, written only while the person's zone still follows their
/// device: a report that arrives after they chose a zone of their own changes nothing. Says whether
/// it was written.
pub async fn report_device_timezone(
    conn: &mut Conn,
    user: Id,
    zone: &str,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let written = sqlx::query(
        "UPDATE users SET timezone = $2, timezone_reported_at = $3, updated_at = $3
          WHERE id = $1 AND timezone_mode = 'device'",
    )
    .bind(user)
    .bind(zone)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(written.rows_affected() == 1)
}

/// Sets the password hash of `user`.
pub async fn set_password(
    conn: &mut Conn,
    user: Id,
    hash: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET password_hash = $2, updated_at = $3 WHERE id = $1")
        .bind(user)
        .bind(hash)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Gives `user` the address `email`, confirmed at `at` or not. An address
/// that another person has is refused as a unique value taken.
pub async fn set_email(
    conn: &mut Conn,
    user: Id,
    email: &str,
    confirmed: bool,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET email = $2, email_verified_at = CASE WHEN $3 THEN $4 ELSE NULL END, updated_at = $4
          WHERE id = $1",
    )
    .bind(user)
    .bind(email)
    .bind(confirmed)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether a person other than `except` has the address `email`, without
/// regard to case.
pub async fn email_in_use(conn: &mut Conn, email: &str, except: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE lower(email) = lower($1) AND id <> $2)",
    )
    .bind(email)
    .bind(except)
    .fetch_one(conn)
    .await?)
}

/// Marks the address `user` has as confirmed at `at`, if it was not.
pub async fn confirm_current_email(
    conn: &mut Conn,
    user: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET email_verified_at = $2, updated_at = $2
          WHERE id = $1 AND email IS NOT NULL AND email_verified_at IS NULL",
    )
    .bind(user)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Notes that `user` was seen at `at`.
pub async fn touch_last_seen(conn: &mut Conn, user: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET last_seen_at = $2 WHERE id = $1")
        .bind(user)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stores a secret for an authenticator that is not switched on yet, sealed
/// by the caller. Nothing is stored where the person has a secret already,
/// pending or on, so that two requests at once end with one secret: the
/// answer says whether this one is it.
pub async fn put_pending_totp(
    conn: &mut Conn,
    user: Id,
    sealed: &[u8],
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE users SET totp_secret = $2, totp_last_step = NULL, updated_at = $3
          WHERE id = $1 AND totp_enabled_at IS NULL AND totp_secret IS NULL",
    )
    .bind(user)
    .bind(sealed)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Switches the authenticator on, having accepted the time step `step`.
/// Nothing changes unless there is a secret and the authenticator is off.
pub async fn enable_totp(
    conn: &mut Conn,
    user: Id,
    step: i64,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE users SET totp_enabled_at = $3, totp_last_step = $2, updated_at = $3
          WHERE id = $1 AND totp_secret IS NOT NULL AND totp_enabled_at IS NULL",
    )
    .bind(user)
    .bind(step)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Accepts the time step `step` of the authenticator, once: it is refused
/// where a step as late or later was accepted already, so that a code works
/// one time however many requests carry it at once.
pub async fn accept_totp_step(conn: &mut Conn, user: Id, step: i64) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE users SET totp_last_step = $2
          WHERE id = $1 AND totp_enabled_at IS NOT NULL
            AND (totp_last_step IS NULL OR totp_last_step < $2)",
    )
    .bind(user)
    .bind(step)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Forgets the authenticator, its secret and the steps it accepted.
pub async fn disable_totp(conn: &mut Conn, user: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL, totp_last_step = NULL, updated_at = $2
          WHERE id = $1",
    )
    .bind(user)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    super::recovery_codes::clear(conn, user).await?;
    crate::sequences::notify_identity(conn, user).await?;
    Ok(())
}

/// Makes a mailed code the second step of `user` from `since`, or stops it
/// being one.
pub async fn set_email_code_second_step(
    conn: &mut Conn,
    user: Id,
    since: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET email_code_at = $2, updated_at = $3 WHERE id = $1")
        .bind(user)
        .bind(since)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    crate::sequences::notify_identity(conn, user).await?;
    Ok(())
}

/// What a change of a person's standing came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It was made.
    Changed,
    /// It was so already; nothing was written.
    AsItWas,
    /// It would have left the system without a system admin who can sign
    /// in; nothing was written.
    LastAdmin,
}

/// Holds the lock of the changes that could leave the system without a
/// system admin until the transaction ends.
async fn lock_admins(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMINS_LOCK)
        .execute(conn)
        .await?;
    Ok(())
}

/// Whether a system admin other than `user` can sign in.
async fn another_admin_can_sign_in(conn: &mut Conn, user: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users
                         WHERE id <> $1 AND kind = 'human' AND is_system_admin AND status = 'active')",
    )
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// Gives `user` the flag of a system admin, or takes it away. Taking it from
/// the last system admin who can sign in is refused, and so is anything that
/// two requests at once would make so: the changes are made one at a time.
/// Run it in the transaction of the change; a person that is not there is a
/// row that was not found.
pub async fn set_system_admin(
    conn: &mut Conn,
    user: Id,
    admin: bool,
    at: DateTime<Utc>,
) -> Result<Standing, DbError> {
    lock_admins(conn).await?;
    let (is_admin, status): (bool, String) =
        // KEY SHARE conflicts with UPDATE, so guards allow counters/FKs while
        // this identity change waits for every earlier action by this person.
        sqlx::query_as("SELECT is_system_admin, status FROM users WHERE id = $1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *conn)
            .await?;
    if is_admin == admin {
        return Ok(Standing::AsItWas);
    }
    if !admin && status == "active" && !another_admin_can_sign_in(conn, user).await? {
        return Ok(Standing::LastAdmin);
    }
    sqlx::query("UPDATE users SET is_system_admin = $2, updated_at = $3 WHERE id = $1")
        .bind(user)
        .bind(admin)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(Standing::Changed)
}

/// Suspends `user` altogether, or lifts that. Suspending the last system
/// admin who can sign in is refused, as taking their flag is. The sessions of
/// a person who is suspended are ended by the caller ([`revoke_sessions_of`]),
/// in the same transaction.
pub async fn set_suspended(
    conn: &mut Conn,
    user: Id,
    suspended: bool,
    at: DateTime<Utc>,
) -> Result<Standing, DbError> {
    lock_admins(conn).await?;
    let (is_admin, status): (bool, String) =
        // KEY SHARE conflicts with UPDATE; the status UPDATE alone would take
        // NO KEY UPDATE and would not protect a guarded action's user identity.
        sqlx::query_as("SELECT is_system_admin, status FROM users WHERE id = $1 FOR UPDATE")
            .bind(user)
            .fetch_one(&mut *conn)
            .await?;
    let wanted = if suspended { "suspended" } else { "active" };
    if status == wanted {
        return Ok(Standing::AsItWas);
    }
    if suspended && is_admin && !another_admin_can_sign_in(conn, user).await? {
        return Ok(Standing::LastAdmin);
    }
    sqlx::query("UPDATE users SET status = $2, updated_at = $3 WHERE id = $1")
        .bind(user)
        .bind(wanted)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(Standing::Changed)
}

/// A browser session.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRow {
    /// This browser session's explicit administrator mode, off when it starts.
    pub as_admin: bool,
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The browser that opened it.
    pub user_agent: String,
    /// The address it was opened from.
    pub ip: Option<String>,
    /// When it was opened.
    pub created_at: DateTime<Utc>,
    /// When it was last used.
    pub last_used_at: DateTime<Utc>,
    /// When it ends if it is not used before.
    pub expires_at: DateTime<Utc>,
    /// When it was ended.
    pub revoked_at: Option<DateTime<Utc>>,
}

const SESSION_COLUMNS: &str = "id, user_id, as_admin, user_agent, host(ip) AS ip, created_at, last_used_at, expires_at, revoked_at";

/// Reads only the selected live browser session's administrator switch.
pub async fn session_admin_mode(
    conn: &mut Conn,
    user: Id,
    session: Id,
    at: DateTime<Utc>,
) -> Result<Option<bool>, DbError> {
    Ok(sqlx::query_scalar("SELECT as_admin FROM sessions WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND expires_at > $3")
        .bind(session).bind(user).bind(at).fetch_optional(conn).await?)
}

/// Changes the switch under the person's lock and announces it after commit.
pub async fn set_session_admin_mode(
    conn: &mut Conn,
    user: Id,
    session: Id,
    enabled: bool,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query("UPDATE sessions SET as_admin = $3 WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL AND expires_at > $4")
        .bind(session).bind(user).bind(enabled).bind(at).execute(&mut *conn).await?;
    if changed.rows_affected() == 0 {
        return Ok(false);
    }
    crate::sequences::notify_identity(conn, user).await?;
    Ok(true)
}

/// Whether this active person has administrative reach beyond their personal/service space.
pub async fn admin_mode_available(
    conn: &mut Conn,
    user: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT CASE WHEN u.is_system_admin THEN true
         ELSE EXISTS (SELECT 1 FROM access_holders(u.id, $2, false) h JOIN nodes n ON n.id=h.id
           WHERE h.kind = 'role' AND h.role = 'admin'
             AND NOT COALESCE(u.personal_org_id=ANY(n.path),false)
             AND NOT COALESCE(u.service_org_id=ANY(n.path),false)) END
         FROM users u WHERE u.id = $1 AND u.status = 'active'",
    )
    .bind(user)
    .bind(at)
    .fetch_optional(conn)
    .await?
    .unwrap_or(false))
}

/// A session to open.
#[derive(Debug, Clone, Copy)]
pub struct NewSession<'a> {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The SHA-256 of the value of the cookie that holds it.
    pub refresh_hash: &'a [u8],
    /// The browser, cut to what a session keeps.
    pub user_agent: &'a str,
    /// Where it was opened from.
    pub ip: Option<IpAddr>,
    /// When it is opened.
    pub at: DateTime<Utc>,
    /// When it ends if it is not used.
    pub expires_at: DateTime<Utc>,
}

/// Opens a session.
pub async fn create_session(conn: &mut Conn, new: &NewSession<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sessions (id, user_id, refresh_hash, user_agent, ip, created_at, last_used_at, expires_at)
         VALUES ($1, $2, $3, $4, $5::inet, $6, $6, $7)",
    )
    .bind(new.id)
    .bind(new.user_id)
    .bind(new.refresh_hash)
    .bind(new.user_agent)
    .bind(new.ip.map(|ip| ip.to_string()))
    .bind(new.at)
    .bind(new.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Stores an access token of a session, by its hash, good until `expires_at`.
pub async fn add_access_token(
    conn: &mut Conn,
    token_hash: &[u8],
    session: Id,
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, session_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(token_hash)
    .bind(session)
    .bind(expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The session whose cookie hashes to `refresh_hash`, where it is neither
/// ended nor past its end at `now`.
pub async fn session_by_refresh_hash(
    conn: &mut Conn,
    refresh_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<SessionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SESSION_COLUMNS} FROM sessions
          WHERE refresh_hash = $1 AND revoked_at IS NULL AND expires_at > $2"
    )))
    .bind(refresh_hash)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Uses a session at `now`: it now ends at `expires_at` if it is not used
/// again, and the access tokens of it that have run out are dropped.
pub async fn slide_session(
    conn: &mut Conn,
    session: Id,
    now: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE sessions SET last_used_at = $2, expires_at = $3 WHERE id = $1")
        .bind(session)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM access_tokens WHERE session_id = $1 AND expires_at <= $2")
        .bind(session)
        .bind(now)
        .execute(conn)
        .await?;
    Ok(())
}

/// The sessions of `user` that are neither ended nor past their end at
/// `now`, the most recently used first.
pub async fn live_sessions(
    conn: &mut Conn,
    user: Id,
    now: DateTime<Utc>,
) -> Result<Vec<SessionRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SESSION_COLUMNS} FROM sessions
          WHERE user_id = $1 AND revoked_at IS NULL AND expires_at > $2
          ORDER BY last_used_at DESC, id"
    )))
    .bind(user)
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Ends the session `session` of `user`, and drops its access tokens. The
/// answer says whether there was a live session to end.
pub async fn revoke_session(
    conn: &mut Conn,
    session: Id,
    user: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE sessions SET revoked_at = $3 WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(session)
    .bind(user)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM access_tokens WHERE session_id = $1")
        .bind(session)
        .execute(&mut *conn)
        .await?;
    if done.rows_affected() == 1 {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// Ends every live session of `user` but `keep`, and drops their access
/// tokens. The answer says how many were ended.
pub async fn revoke_sessions_of(
    conn: &mut Conn,
    user: Id,
    keep: Option<Id>,
    now: DateTime<Utc>,
) -> Result<u64, DbError> {
    let ended: Vec<Id> = sqlx::query_scalar(
        "UPDATE sessions SET revoked_at = $3
          WHERE user_id = $1 AND revoked_at IS NULL AND ($2::uuid IS NULL OR id <> $2)
      RETURNING id",
    )
    .bind(user)
    .bind(keep)
    .bind(now)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM access_tokens WHERE session_id = ANY($1)")
        .bind(&ended)
        .execute(&mut *conn)
        .await?;
    if !ended.is_empty() {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(u64::try_from(ended.len()).unwrap_or(u64::MAX))
}

/// A browser session that an access token belongs to.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AccessRow {
    /// The exact browser session's explicit administrator mode.
    pub as_admin: bool,
    /// The session.
    pub session_id: Id,
    /// Its user.
    pub user_id: Id,
}

/// The session behind the access token whose hash is `token_hash`, where
/// the token has not expired and the session is neither expired nor
/// revoked at `now`.
pub async fn session_by_access_token(
    conn: &mut Conn,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<AccessRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.id AS session_id, s.user_id, s.as_admin
           FROM access_tokens a JOIN sessions s ON s.id = a.session_id
          WHERE a.token_hash = $1 AND a.expires_at > $2 AND s.expires_at > $2 AND s.revoked_at IS NULL",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// An API token, as far as a request needs it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ApiTokenRow {
    /// The token's identifier.
    pub id: Id,
    /// Its user.
    pub user_id: Id,
    /// The permissions it may exercise; `*` is all its user holds.
    pub scopes: Vec<String>,
    /// The node it is confined to, with everything below.
    pub node_id: Option<Id>,
}

/// The API token whose hash is `token_hash`, where it is neither revoked
/// nor expired at `now`. Its last use is noted when another action is not using it.
pub async fn api_token_by_hash(
    conn: &mut Conn,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<ApiTokenRow>, DbError> {
    // Last-use bookkeeping is not authority. NO KEY UPDATE SKIP LOCKED avoids
    // queuing authentication behind an action's SHARE lock; its guard still
    // rechecks revocation/scopes before writing. A busy token is read normally.
    Ok(sqlx::query_as(
        "WITH touched AS (
            UPDATE api_tokens SET last_used_at = $2
            WHERE id IN (
                SELECT id FROM api_tokens
                WHERE token_hash = $1 AND revoked_at IS NULL
                  AND (expires_at IS NULL OR expires_at > $2)
                FOR NO KEY UPDATE SKIP LOCKED
            ) RETURNING id, user_id, scopes, node_id
         )
         SELECT * FROM touched
         UNION ALL
         SELECT id, user_id, scopes, node_id FROM api_tokens
         WHERE token_hash = $1 AND revoked_at IS NULL
           AND (expires_at IS NULL OR expires_at > $2)
           AND NOT EXISTS (SELECT 1 FROM touched)",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// An API token as its person lists it, with the node it is kept to.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ApiTokenListed {
    /// Its identifier.
    pub id: Id,
    /// The name its maker gave it.
    pub name: String,
    /// What it may do.
    pub scopes: Vec<String>,
    /// The node it is kept to.
    pub node_id: Option<Id>,
    /// The kind of that node.
    pub node_kind: Option<String>,
    /// The name of that node.
    pub node_name: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was last used.
    pub last_used_at: Option<DateTime<Utc>>,
    /// When it stops working.
    pub expires_at: Option<DateTime<Utc>>,
}

const API_TOKEN_COLUMNS: &str =
    "t.id, t.name, t.scopes, t.node_id, n.kind AS node_kind, n.name AS node_name,
                                  t.created_at, t.last_used_at, t.expires_at";

/// An API token to make.
#[derive(Debug, Clone, Copy)]
pub struct NewApiToken<'a> {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// The name to tell it by.
    pub name: &'a str,
    /// The SHA-256 of its secret.
    pub token_hash: &'a [u8],
    /// What it may do.
    pub scopes: &'a [String],
    /// The node it is kept to.
    pub node_id: Option<Id>,
    /// When it is made.
    pub at: DateTime<Utc>,
    /// When it stops working.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Stores an API token, by the hash of its secret.
pub async fn create_api_token(conn: &mut Conn, new: &NewApiToken<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, node_id, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(new.id)
    .bind(new.user_id)
    .bind(new.name)
    .bind(new.token_hash)
    .bind(new.scopes)
    .bind(new.node_id)
    .bind(new.at)
    .bind(new.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The API token `id`, whoever's it is.
pub async fn api_token_listed(
    conn: &mut Conn,
    id: Id,
    user: Id,
) -> Result<Option<ApiTokenListed>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {API_TOKEN_COLUMNS} FROM api_tokens t LEFT JOIN nodes n ON n.id = t.node_id
          WHERE t.id = $1 AND t.user_id = $2 AND t.revoked_at IS NULL"
    )))
    .bind(id)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// The API tokens of `user` that were not revoked, the newest first.
pub async fn api_tokens_of(conn: &mut Conn, user: Id) -> Result<Vec<ApiTokenListed>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {API_TOKEN_COLUMNS} FROM api_tokens t LEFT JOIN nodes n ON n.id = t.node_id
          WHERE t.user_id = $1 AND t.revoked_at IS NULL
          ORDER BY t.created_at DESC, t.id"
    )))
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Revokes the API token `id` of `user`. The answer says whether there was a
/// token to revoke.
pub async fn revoke_api_token(
    conn: &mut Conn,
    id: Id,
    user: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE api_tokens SET revoked_at = $3 WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(done.rows_affected() == 1)
}

/// A link that was mailed, as stored: what it is for and where it went.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct EmailTokenRow {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// `verify_email` or `reset_password`.
    pub purpose: String,
    /// The address it was sent to.
    pub email: String,
    /// Whether using it also switches the second step off.
    pub clear_second_step: bool,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// When it was used.
    pub used_at: Option<DateTime<Utc>>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
}

/// A link to mail.
#[derive(Debug, Clone, Copy)]
pub struct NewEmailToken<'a> {
    /// Its identifier.
    pub id: Id,
    /// Whose it is.
    pub user_id: Id,
    /// `verify_email` or `reset_password`.
    pub purpose: &'a str,
    /// The SHA-256 of its secret.
    pub token_hash: &'a [u8],
    /// The address it goes to.
    pub email: &'a str,
    /// Whether using it also switches the second step off.
    pub clear_second_step: bool,
    /// When it is made.
    pub at: DateTime<Utc>,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
}

/// Stores a link that is to be mailed, by the hash of its secret.
pub async fn create_email_token(conn: &mut Conn, new: &NewEmailToken<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO email_tokens (id, user_id, purpose, token_hash, email, clear_second_step, expires_at, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(new.id)
    .bind(new.user_id)
    .bind(new.purpose)
    .bind(new.token_hash)
    .bind(new.email)
    .bind(new.clear_second_step)
    .bind(new.expires_at)
    .bind(new.at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether a link is unused, of this purpose, before its end and still belongs to an account.
pub async fn email_token_live(
    conn: &mut Conn,
    token_hash: &[u8],
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM email_tokens t JOIN users u ON u.id = t.user_id
          WHERE t.token_hash = $1 AND t.purpose = $2 AND t.used_at IS NULL AND t.expires_at > $3)",
    )
    .bind(token_hash)
    .bind(purpose)
    .bind(now)
    .fetch_one(conn)
    .await?)
}

/// Finds its person before taking that person's lock; consuming the link then
/// checks its purpose, use and end again after the lock wait.
pub async fn email_token_owner(
    conn: &mut Conn,
    token_hash: &[u8],
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM email_tokens
          WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > $3",
    )
    .bind(token_hash)
    .bind(purpose)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Consumes a still-live link once, after its person's row has been locked.
pub async fn use_email_token(
    conn: &mut Conn,
    token_hash: &[u8],
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<Option<EmailTokenRow>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE email_tokens SET used_at = $3
          WHERE token_hash = $1 AND purpose = $2 AND used_at IS NULL AND expires_at > $3
      RETURNING id, user_id, purpose, email, clear_second_step, expires_at, used_at, created_at",
    )
    .bind(token_hash)
    .bind(purpose)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Spends every link of `user` of that `purpose` that was not used, so that
/// none of them works any more. The answer says how many there were.
pub async fn void_email_tokens(
    conn: &mut Conn,
    user: Id,
    purpose: &str,
    now: DateTime<Utc>,
) -> Result<u64, DbError> {
    let done = sqlx::query(
        "UPDATE email_tokens SET used_at = $3 WHERE user_id = $1 AND purpose = $2 AND used_at IS NULL",
    )
    .bind(user)
    .bind(purpose)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// The address `user` asked to change to and has not confirmed: the one of
/// the newest link of confirmation that is neither used nor past its end and
/// goes to another address than the one they have.
pub async fn pending_email(
    conn: &mut Conn,
    user: Id,
    now: DateTime<Utc>,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.email FROM email_tokens t JOIN users u ON u.id = t.user_id
          WHERE t.user_id = $1 AND t.purpose = 'verify_email' AND t.used_at IS NULL AND t.expires_at > $2
            AND (u.email IS NULL OR lower(u.email) <> lower(t.email))
          ORDER BY t.created_at DESC, t.id DESC
          LIMIT 1",
    )
    .bind(user)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// A code that was mailed to a person to sign in with.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SignInCodeRow {
    /// Whose it is.
    pub user_id: Id,
    /// The SHA-256 of their identifier and the code.
    pub code_hash: Vec<u8>,
    /// The confirmed address it went to.
    pub email: String,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
}

/// Stores the code that was mailed to `user`, by its hash. A new code ends
/// the one before: a person has one good code at a time.
pub async fn put_sign_in_code(
    conn: &mut Conn,
    user: Id,
    code_hash: &[u8],
    email: &str,
    at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sign_in_codes (user_id, code_hash, email, expires_at, created_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id) DO UPDATE
            SET code_hash = EXCLUDED.code_hash, email = EXCLUDED.email,
                expires_at = EXCLUDED.expires_at, created_at = EXCLUDED.created_at",
    )
    .bind(user)
    .bind(code_hash)
    .bind(email)
    .bind(expires_at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The code `user` has, where it is not past its end at `now`.
pub async fn live_sign_in_code(
    conn: &mut Conn,
    user: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Option<SignInCodeRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT user_id, code_hash, email, expires_at, created_at FROM sign_in_codes
          WHERE user_id = $1 AND expires_at > $2",
    )
    .bind(user)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Spends the code of `user` whose hash is `code_hash`: it goes, and works
/// no more. The answer says whether this call spent it.
pub async fn spend_sign_in_code(
    conn: &mut Conn,
    user: Id,
    code_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "DELETE FROM sign_in_codes WHERE user_id = $1 AND code_hash = $2 AND expires_at > $3",
    )
    .bind(user)
    .bind(code_hash)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A browser that a person got in from.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KnownDevice {
    /// Its identifier, which the counts of wrong tries name it by.
    pub id: Id,
    /// When this person first got in from it.
    pub first_at: DateTime<Utc>,
    /// The last time they did.
    pub last_at: DateTime<Utc>,
}

/// The browser whose mark hashes to `mark_hash`, if `user` got in from it
/// after `not_before`. Without a mark or without a person it is nobody's, and
/// the statement is the same.
pub async fn known_device(
    conn: &mut Conn,
    mark_hash: Option<&[u8]>,
    user: Option<Id>,
    not_before: DateTime<Utc>,
) -> Result<Option<KnownDevice>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, first_at, last_at FROM known_devices
          WHERE mark_hash = $1 AND user_id = $2 AND last_at > $3",
    )
    .bind(mark_hash)
    .bind(user)
    .bind(not_before)
    .fetch_optional(conn)
    .await?)
}

/// Notes that `user` got in at `at` from the browser whose mark hashes to
/// `mark_hash`, and returns its identifier. A browser that was known stays
/// so, and its time is renewed; one that was not, or was known long ago,
/// becomes known. A person has at most `keep` of them: the one unused the
/// longest goes for the next, and so do those unused since `not_before`.
pub async fn remember_device(
    conn: &mut Conn,
    mark_hash: &[u8],
    user: Id,
    at: DateTime<Utc>,
    not_before: DateTime<Utc>,
    keep: i64,
) -> Result<Id, DbError> {
    let id: Id = sqlx::query_scalar(
        "INSERT INTO known_devices (id, mark_hash, user_id, first_at, last_at)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (mark_hash, user_id) DO UPDATE SET last_at = EXCLUDED.last_at
         RETURNING id",
    )
    .bind(orch_types::new_id())
    .bind(mark_hash)
    .bind(user)
    .bind(at)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM known_devices
          WHERE user_id = $1
            AND (last_at <= $2
                 OR id IN (SELECT id FROM known_devices WHERE user_id = $1
                            ORDER BY last_at DESC, id DESC OFFSET $3))",
    )
    .bind(user)
    .bind(not_before)
    .bind(keep)
    .execute(conn)
    .await?;
    Ok(id)
}

/// How a count of wrong tries stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reservation {
    /// The try is counted. It is `filled` when it was the one that made the
    /// count as large as its limit.
    Counted {
        /// Whether the count is now as large as its limit.
        filled: bool,
    },
    /// The count is full, or the last of its tries is too recent: the try is
    /// refused unseen, and nothing was counted. It may be made again after
    /// this long.
    Refused {
        /// Seconds until it may be.
        retry_after: u64,
    },
}

/// What bounds a count of tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    /// How many tries it holds at most.
    pub limit: u32,
    /// For how long a try counts.
    pub window: Duration,
    /// How long after one try the next may be made, where tries are paced.
    pub gap: Option<Duration>,
}

fn seconds(duration: Duration) -> f64 {
    duration.num_milliseconds() as f64 / 1000.0
}

/// How long until a count with the tries `times` lets one more in, at `now`.
/// It is a whole number of seconds, rounded up, and never less than one.
#[must_use]
pub fn retry_after(times: &[DateTime<Utc>], now: DateTime<Utc>, bound: &Bound) -> u64 {
    let mut kept: Vec<DateTime<Utc>> = times
        .iter()
        .copied()
        .filter(|time| *time > now - bound.window)
        .collect();
    kept.sort_unstable();
    let limit = usize::try_from(bound.limit).unwrap_or(usize::MAX).max(1);
    let mut until = now;
    if kept.len() >= limit {
        // The tries must thin out until there are fewer than the limit.
        if let Some(oldest_to_go) = kept.get(kept.len() - limit) {
            until = until.max(*oldest_to_go + bound.window);
        }
    }
    if let (Some(gap), Some(last)) = (bound.gap, kept.last())
        && *last + gap > now
    {
        until = until.max(*last + gap);
    }
    let wait = (until - now).num_milliseconds().max(0);
    let whole = u64::try_from((wait + 999) / 1000).unwrap_or(u64::MAX);
    whole.max(1)
}

/// Counts a try in the bucket `bucket` at `now` if the bucket has room, in
/// one statement, so that tries made at the same moment cannot pass the
/// limit together. The tries of the bucket are those within `bound.window`,
/// the oldest first, never more than `bound.limit`.
pub async fn reserve_try(
    conn: &mut Conn,
    bucket: &str,
    now: DateTime<Utc>,
    bound: &Bound,
) -> Result<Reservation, DbError> {
    if bound.limit == 0 {
        // A count that holds no try at all lets none in.
        return Ok(Reservation::Refused {
            retry_after: u64::try_from(bound.window.num_seconds())
                .unwrap_or(1)
                .max(1),
        });
    }
    let window = seconds(bound.window);
    let counted: Option<i32> = sqlx::query_scalar(
        "INSERT INTO login_throttle AS t (bucket, times) VALUES ($1, ARRAY[$2::timestamptz])
         ON CONFLICT (bucket) DO UPDATE
            SET times = ARRAY(SELECT x FROM unnest(
                           array_append(ARRAY(SELECT y FROM unnest(t.times) AS y
                                               WHERE y > $2::timestamptz - make_interval(secs => $4::float8)),
                                        $2::timestamptz)) AS x
                         ORDER BY x)
          WHERE cardinality(ARRAY(SELECT y FROM unnest(t.times) AS y
                                   WHERE y > $2::timestamptz - make_interval(secs => $4::float8))) < $3::int8
            AND ($5::float8 IS NULL
                 OR NOT EXISTS (SELECT 1 FROM unnest(t.times) AS y
                                 WHERE y > $2::timestamptz - make_interval(secs => $5::float8)))
      RETURNING cardinality(t.times)",
    )
    .bind(bucket)
    .bind(now)
    .bind(i64::from(bound.limit))
    .bind(window)
    .bind(bound.gap.map(seconds))
    .fetch_optional(&mut *conn)
    .await?;
    match counted {
        Some(count) => Ok(Reservation::Counted {
            filled: i64::from(count) >= i64::from(bound.limit),
        }),
        None => {
            let times = throttle_times(conn, bucket).await?;
            Ok(Reservation::Refused {
                retry_after: retry_after(&times, now, bound),
            })
        }
    }
}

/// Counts a try of something that is paced, such as the mailing of a code, at
/// `now`, in one statement, when `enabled` and the bucket has room: it is
/// counted only for what is done, so that a request that did nothing does not
/// hold up the next that does. The statement is the same whether it counts or
/// not, which is what lets a request answer every name alike. The answer says
/// whether the try was counted.
pub async fn pace(
    conn: &mut Conn,
    bucket: &str,
    now: DateTime<Utc>,
    bound: &Bound,
    enabled: bool,
) -> Result<bool, DbError> {
    let room: Option<i32> = sqlx::query_scalar(
        "INSERT INTO login_throttle AS t (bucket, times)
         SELECT $1, ARRAY[$2::timestamptz] WHERE $6::bool AND $3::int8 > 0
         ON CONFLICT (bucket) DO UPDATE
            SET times = ARRAY(SELECT x FROM unnest(
                           array_append(ARRAY(SELECT y FROM unnest(t.times) AS y
                                               WHERE y > $2::timestamptz - make_interval(secs => $4::float8)),
                                        $2::timestamptz)) AS x
                         ORDER BY x)
          WHERE cardinality(ARRAY(SELECT y FROM unnest(t.times) AS y
                                   WHERE y > $2::timestamptz - make_interval(secs => $4::float8))) < $3::int8
            AND ($5::float8 IS NULL
                 OR NOT EXISTS (SELECT 1 FROM unnest(t.times) AS y
                                 WHERE y > $2::timestamptz - make_interval(secs => $5::float8)))
      RETURNING cardinality(t.times)",
    )
    .bind(bucket)
    .bind(now)
    .bind(i64::from(bound.limit))
    .bind(seconds(bound.window))
    .bind(bound.gap.map(seconds))
    .bind(enabled)
    .fetch_optional(conn)
    .await?;
    Ok(room.is_some())
}

/// Gives back a try that `reserve_try` counted at `at`, because it was right
/// after all. Nothing else clears a count: it thins out as the tries age.
pub async fn refund_try(conn: &mut Conn, bucket: &str, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE login_throttle SET times = ARRAY(
              SELECT u.x FROM unnest(times) WITH ORDINALITY AS u(x, i)
               WHERE u.i <> (SELECT min(v.i) FROM unnest(times) WITH ORDINALITY AS v(x, i) WHERE v.x = $2::timestamptz)
               ORDER BY u.i)
          WHERE bucket = $1",
    )
    .bind(bucket)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// The tries a bucket holds, the oldest first, as they were counted.
pub async fn throttle_times(conn: &mut Conn, bucket: &str) -> Result<Vec<DateTime<Utc>>, DbError> {
    let times: Option<Vec<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT times FROM login_throttle WHERE bucket = $1")
            .bind(bucket)
            .fetch_optional(conn)
            .await?;
    Ok(times.unwrap_or_default())
}

/// Notes that the person was told that the bucket filled, if they were not
/// told in the last `interval`. The answer says whether this call is the one
/// to tell them.
pub async fn mark_noticed(
    conn: &mut Conn,
    bucket: &str,
    now: DateTime<Utc>,
    interval: Duration,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE login_throttle SET noticed_at = $2
          WHERE bucket = $1 AND (noticed_at IS NULL OR noticed_at <= $2 - make_interval(secs => $3))",
    )
    .bind(bucket)
    .bind(now)
    .bind(seconds(interval))
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Drops the buckets whose tries have all aged out and whose person was told
/// longer ago than `before`: they hold nothing that matters. The answer says
/// how many went.
pub async fn prune_throttle(
    conn: &mut Conn,
    now: DateTime<Utc>,
    window: Duration,
    before: Duration,
) -> Result<u64, DbError> {
    let done = sqlx::query(
        "DELETE FROM login_throttle
          WHERE (noticed_at IS NULL OR noticed_at <= $1 - make_interval(secs => $3))
            AND NOT EXISTS (SELECT 1 FROM unnest(times) AS y WHERE y > $1 - make_interval(secs => $2))",
    )
    .bind(now)
    .bind(seconds(window))
    .bind(seconds(before))
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// The id of the mail server the system has, where it has one that is
/// switched on.
pub async fn system_mail_channel(conn: &mut Conn) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM notification_channels
          WHERE org_id IS NULL AND user_id IS NULL AND kind = 'email' AND enabled
          LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// Puts a message that is no notification into the outbox, for the worker to
/// deliver by the channel `channel_id` of the kind `channel`, due at `at`.
pub async fn queue_mail(
    conn: &mut Conn,
    channel: &str,
    channel_id: Id,
    payload: &Json,
    at: DateTime<Utc>,
) -> Result<Id, DbError> {
    let id = orch_types::new_id();
    sqlx::query(
        "INSERT INTO outbox (id, channel, channel_id, payload, state, attempts, next_attempt_at, created_at)
         VALUES ($1, $2, $3, $4, 'pending', 0, $5, $5)",
    )
    .bind(id)
    .bind(channel)
    .bind(channel_id)
    .bind(payload)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(id)
}

/// A code to mail to a person.
#[derive(Debug, Clone, Copy)]
pub struct IssueCode<'a> {
    /// Whose it is.
    pub user_id: Id,
    /// The SHA-256 of their identifier and the code.
    pub code_hash: &'a [u8],
    /// The confirmed address it goes to.
    pub email: &'a str,
    /// When it is made.
    pub at: DateTime<Utc>,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// The mail server it goes by.
    pub channel: Id,
    /// The message, as the worker takes it.
    pub payload: &'a Json,
}

/// Stores a code and queues the mail that carries it, in one statement, and
/// only if `enabled`: a statement that does nothing costs what one that does
/// costs, which is how a request for a code answers every name alike. The
/// answer says whether anything was written.
pub async fn issue_code(
    conn: &mut Conn,
    issue: &IssueCode<'_>,
    enabled: bool,
) -> Result<bool, DbError> {
    let written: i64 = sqlx::query_scalar(
        "WITH code AS (
            INSERT INTO sign_in_codes (user_id, code_hash, email, expires_at, created_at)
            SELECT $1::uuid, $2::bytea, $3::text, $4::timestamptz, $5::timestamptz WHERE $6::bool
            ON CONFLICT (user_id) DO UPDATE
               SET code_hash = EXCLUDED.code_hash, email = EXCLUDED.email,
                   expires_at = EXCLUDED.expires_at, created_at = EXCLUDED.created_at
            RETURNING 1),
         mail AS (
            INSERT INTO outbox (id, channel, channel_id, payload, state, attempts, next_attempt_at, created_at)
            SELECT $7::uuid, 'email', $8::uuid, $9::jsonb, 'pending', 0, $5::timestamptz, $5::timestamptz
             WHERE $6::bool
            RETURNING 1)
         SELECT ((SELECT count(*) FROM code) + (SELECT count(*) FROM mail))::int8",
    )
    .bind(issue.user_id)
    .bind(issue.code_hash)
    .bind(issue.email)
    .bind(issue.expires_at)
    .bind(issue.at)
    .bind(enabled)
    .bind(orch_types::new_id())
    .bind(issue.channel)
    .bind(issue.payload)
    .fetch_one(conn)
    .await?;
    Ok(written > 0)
}

/// A link to mail to a person.
#[derive(Debug, Clone, Copy)]
pub struct IssueLink<'a> {
    /// The link, by the hash of its secret.
    pub token: NewEmailToken<'a>,
    /// The mail server it goes by.
    pub channel: Id,
    /// The message, as the worker takes it.
    pub payload: &'a Json,
}

/// Stores a link and queues the mail that carries it, in one statement, and
/// only if `enabled`, as [`issue_code`] does. The answer says whether
/// anything was written.
pub async fn issue_link(
    conn: &mut Conn,
    issue: &IssueLink<'_>,
    enabled: bool,
) -> Result<bool, DbError> {
    let token = &issue.token;
    let written: i64 = sqlx::query_scalar(
        "WITH link AS (
            INSERT INTO email_tokens (id, user_id, purpose, token_hash, email, clear_second_step, expires_at, created_at)
            SELECT $1::uuid, $2::uuid, $3::text, $4::bytea, $5::text, $6::bool, $7::timestamptz, $8::timestamptz
             WHERE $9::bool
            RETURNING 1),
         mail AS (
            INSERT INTO outbox (id, channel, channel_id, payload, state, attempts, next_attempt_at, created_at)
            SELECT $10::uuid, 'email', $11::uuid, $12::jsonb, 'pending', 0, $8::timestamptz, $8::timestamptz
             WHERE $9::bool
            RETURNING 1)
         SELECT ((SELECT count(*) FROM link) + (SELECT count(*) FROM mail))::int8",
    )
    .bind(token.id)
    .bind(token.user_id)
    .bind(token.purpose)
    .bind(token.token_hash)
    .bind(token.email)
    .bind(token.clear_second_step)
    .bind(token.expires_at)
    .bind(token.at)
    .bind(enabled)
    .bind(orch_types::new_id())
    .bind(issue.channel)
    .bind(issue.payload)
    .fetch_one(conn)
    .await?;
    Ok(written > 0)
}

/// A row for a person's Inbox.
#[derive(Debug, Clone, Copy)]
pub struct NewInboxRow<'a> {
    /// Whose Inbox.
    pub user_id: Id,
    /// The kind of notification.
    pub kind: &'a str,
    /// Its title.
    pub title: &'a str,
    /// Its first line.
    pub body: &'a str,
    /// Whatever else its kind carries.
    pub data: &'a Json,
    /// `<kind>:<source>`: the same key twice is one row.
    pub dedupe_key: &'a str,
    /// When it happened.
    pub at: DateTime<Utc>,
}

/// Writes a row to the Inbox of a person and moves their feed on, unless
/// they have a row with that key already. The answer is the number the row
/// was given in their feed.
pub async fn add_inbox_row(conn: &mut Conn, row: &NewInboxRow<'_>) -> Result<Option<i64>, DbError> {
    let seq: Option<i64> = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1
          WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM notifications WHERE user_id = $1 AND dedupe_key = $2)
      RETURNING inbox_seq",
    )
    .bind(row.user_id)
    .bind(row.dedupe_key)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(seq) = seq else {
        return Ok(None);
    };
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, title, body, data, dedupe_key, inbox_seq, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(orch_types::new_id())
    .bind(row.user_id)
    .bind(row.kind)
    .bind(row.title)
    .bind(row.body)
    .bind(row.data)
    .bind(row.dedupe_key)
    .bind(seq)
    .bind(row.at)
    .execute(&mut *conn)
    .await?;
    sqlx::query("SELECT pg_notify('orch_user', $1)")
        .bind(format!("{}:{seq}", row.user_id))
        .execute(conn)
        .await?;
    Ok(Some(seq))
}

/// An invitation, as far as registering needs it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InvitationCheck {
    /// Its identifier.
    pub id: Id,
    /// The one address a personal link is for.
    pub email: Option<String>,
}

/// The invitation whose secret hashes to `token_hash`, where it is still
/// good at `now`: not taken back, not past its end, not used by as many as it
/// serves.
pub async fn good_invitation(
    conn: &mut Conn,
    token_hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<InvitationCheck>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, email FROM invitations
          WHERE token_hash = $1 AND revoked_at IS NULL AND expires_at > $2 AND uses < max_uses",
    )
    .bind(token_hash)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// An identity at a provider that a person linked.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LinkedIdentity {
    /// The key of the identity provider.
    pub idp_key: String,
    /// Its name.
    pub idp_name: String,
    /// The address the provider gave.
    pub email: Option<String>,
    /// When it was linked.
    pub linked_at: DateTime<Utc>,
}

/// The identities `user` linked, the oldest first.
pub async fn identities_of(conn: &mut Conn, user: Id) -> Result<Vec<LinkedIdentity>, DbError> {
    Ok(sqlx::query_as(
        "SELECT p.key AS idp_key, p.name AS idp_name, i.email, i.linked_at
           FROM user_identities i JOIN identity_providers p ON p.id = i.identity_provider_id
          WHERE i.user_id = $1
          ORDER BY i.linked_at, p.key",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// The identity providers that can be signed in through, as the sign-in page
/// offers them: `(key, name, auto_register)`, by name.
pub async fn enabled_identity_providers(
    conn: &mut Conn,
) -> Result<Vec<(String, String, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT key, name, auto_register FROM identity_providers WHERE enabled ORDER BY name, key",
    )
    .fetch_all(conn)
    .await?)
}

/// Whether the system, or a root org, lets a mailed code in as a way to sign
/// in by itself: some row of the setting `auth.email_code` at the system or
/// at a root org that is not deleted holds `sign_in`.
pub async fn email_code_sign_in_set_somewhere(conn: &mut Conn) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM settings s LEFT JOIN nodes n ON n.id = s.node_id
             WHERE s.key = 'auth.email_code' AND s.value @> '[\"sign_in\"]'::jsonb
               AND (s.node_id IS NULL OR (n.parent_id IS NULL AND n.deleted_at IS NULL)))",
    )
    .fetch_one(conn)
    .await?)
}

/// What the interface of `user` remembers for them on every device, by key.
pub async fn interface_state(conn: &mut Conn, user: Id) -> Result<Vec<(String, Json)>, DbError> {
    Ok(
        sqlx::query_as("SELECT key, value FROM user_state WHERE user_id = $1 ORDER BY key")
            .bind(user)
            .fetch_all(conn)
            .await?,
    )
}

/// A served page carries its last-place fields without overwriting concurrent sidebar choices.
pub async fn note_opened_place(
    conn: &mut Conn,
    user: Id,
    node: Id,
    route: &str,
    org: Option<Id>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO user_state (user_id,key,value,updated_at)
         VALUES ($1,'sidebar',jsonb_strip_nulls(jsonb_build_object('last_place',jsonb_build_object('node_id',$2::text,'route',$3::text),'last_org_id',$4::text)),$5)
         ON CONFLICT (user_id,key) DO UPDATE
         SET value = COALESCE(user_state.value,'{}'::jsonb) || EXCLUDED.value, updated_at = EXCLUDED.updated_at
         WHERE (user_state.value || EXCLUDED.value) IS DISTINCT FROM user_state.value",
    ).bind(user).bind(node).bind(route).bind(org).bind(at).execute(conn).await?;
    Ok(())
}

/// Replaces the whole value of one key of the interface state of `user`.
pub async fn put_interface_state(
    conn: &mut Conn,
    user: Id,
    key: &str,
    value: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO user_state (user_id, key, value, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
    )
    .bind(user)
    .bind(key)
    .bind(value)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps `view` as the entry of `project` in the `project_views` value of the interface state of
/// `user`, beside the entries of the other projects, under the row's lock: two writes for two
/// projects do not replace each other. A value that is no object is begun again. The answer is the
/// size in bytes of the value as it now is, for the caller to hold to the bound.
pub async fn put_project_view(
    conn: &mut Conn,
    user: Id,
    project: Id,
    view: &Json,
    at: DateTime<Utc>,
) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "INSERT INTO user_state (user_id, key, value, updated_at)
         VALUES ($1, 'project_views', jsonb_build_object($2::text, $3::jsonb), $4)
         ON CONFLICT (user_id, key) DO UPDATE
            SET value = (CASE WHEN jsonb_typeof(user_state.value) = 'object' THEN user_state.value ELSE '{}'::jsonb END)
                        || jsonb_build_object($2::text, $3::jsonb),
                updated_at = EXCLUDED.updated_at
         RETURNING octet_length(value::text)::bigint",
    )
    .bind(user)
    .bind(project.to_string())
    .bind(view)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

/// Takes the entry of `project` out of the `project_views` value of the interface state of `user`.
pub async fn delete_project_view(
    conn: &mut Conn,
    user: Id,
    project: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE user_state SET value = value - $2::text, updated_at = $3
          WHERE user_id = $1 AND key = 'project_views' AND jsonb_typeof(value) = 'object'",
    )
    .bind(user)
    .bind(project.to_string())
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Looks for people the way the search of people does: among those who are
/// members of a root org that `caller` is also a member of, by the start of
/// a username or a part of a display name; and anywhere, by an exact
/// username or an exact address. With `org` set, only members of that org
/// (its domain or one inside it). At most `limit`, the nearest names first.
pub async fn search_users(
    conn: &mut Conn,
    caller: Id,
    text: &str,
    org: Option<Id>,
    limit: i64,
) -> Result<Vec<UserBrief>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.kind, u.username, u.display_name, u.avatar_color
           FROM users u
          WHERE u.status = 'active'
            AND (($3::uuid IS NULL
                  AND (u.username = lower($2) OR lower(u.email) = lower($2)
                       OR (EXISTS (SELECT 1 FROM org_members a JOIN org_members b ON a.org_id = b.org_id
                                    WHERE a.user_id = u.id AND b.user_id = $1)
                           AND (starts_with(u.username, lower($2))
                                OR strpos(lower(u.display_name), lower($2)) > 0))))
                 OR ($3::uuid IS NOT NULL
                     AND EXISTS (SELECT 1 FROM org_members m JOIN nodes d ON d.id = m.domain_id
                                  WHERE m.user_id = u.id AND $3 = ANY(d.path))
                     AND (starts_with(u.username, lower($2))
                          OR strpos(lower(u.display_name), lower($2)) > 0
                          OR lower(u.email) = lower($2))))
          ORDER BY (u.username = lower($2)) DESC, u.username, u.id
          LIMIT $4",
    )
    .bind(caller)
    .bind(text)
    .bind(org)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// An invitation, as stored.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InvitationRow {
    /// Its identifier.
    pub id: Id,
    /// `system`, `org`, `node` or `chat`.
    pub kind: String,
    /// The org it invites into.
    pub org_id: Option<Id>,
    /// The node it invites to.
    pub node_id: Option<Id>,
    /// The chat it invites to.
    pub chat_id: Option<Id>,
    /// The role it gives.
    pub role: Option<String>,
    /// The rows of a node's page it names the person in.
    pub permissions: Vec<String>,
    /// The one address a personal link is for.
    pub email: Option<String>,
    /// How many people may use it.
    pub max_uses: i32,
    /// How many have.
    pub uses: i32,
    /// A note.
    pub note: String,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// When it was taken back.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Who made it.
    pub created_by: Id,
    /// The issuer's explicit authority mode at creation; None on a legacy offer.
    pub created_as_admin: Option<bool>,
    /// When.
    pub created_at: DateTime<Utc>,
}

macro_rules! invitation_columns {
    () => {
        "id, kind, org_id, node_id, chat_id, role, permissions, email, max_uses, uses, note,
         expires_at, revoked_at, created_by, created_at, created_as_admin"
    };
}

/// An invitation to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInvitation<'a> {
    /// Its identifier.
    pub id: Id,
    /// The hash of the secret of its link.
    pub token_hash: &'a [u8],
    /// `system`, `org` or `node`.
    pub kind: &'a str,
    /// The org it invites into.
    pub org: Option<Id>,
    /// The node it invites to.
    pub node: Option<Id>,
    /// The group chat it invites to, for the same canonical writer.
    pub chat: Option<Id>,
    /// The role it gives.
    pub role: Option<&'a str>,
    /// The rows it names the person in.
    pub permissions: &'a [String],
    /// The address of a personal link.
    pub email: Option<&'a str>,
    /// How many may use it.
    pub max_uses: i32,
    /// A note.
    pub note: &'a str,
    /// When it stops being good.
    pub expires_at: DateTime<Utc>,
    /// Who makes it.
    pub by: Id,
    /// Whether the issuer explicitly acted in administrator mode for this offer.
    pub as_admin: bool,
    /// When.
    pub at: DateTime<Utc>,
}

/// Makes an invitation.
pub async fn create_invitation(conn: &mut Conn, new: &NewInvitation<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO invitations (id, token_hash, kind, org_id, node_id, role, permissions, email,
                                  max_uses, note, expires_at, created_by, created_at, chat_id, created_as_admin)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(new.id)
    .bind(new.token_hash)
    .bind(new.kind)
    .bind(new.org)
    .bind(new.node)
    .bind(new.role)
    .bind(new.permissions)
    .bind(new.email)
    .bind(new.max_uses)
    .bind(new.note)
    .bind(new.expires_at)
    .bind(new.by)
    .bind(new.at)
    .bind(new.chat)
    .bind(new.as_admin)
    .execute(conn)
    .await?;
    Ok(())
}

/// The invitation with the secret hashed to `hash`, good or not.
pub async fn invitation_by_hash(
    conn: &mut Conn,
    hash: &[u8],
) -> Result<Option<InvitationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        invitation_columns!(),
        " FROM invitations WHERE token_hash = $1"
    ))
    .bind(hash)
    .fetch_optional(conn)
    .await?)
}

/// The invitation `id`.
pub async fn invitation_by_id(conn: &mut Conn, id: Id) -> Result<Option<InvitationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        invitation_columns!(),
        " FROM invitations WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// The invitations into the org `org` when it is given; else the system's
/// when `system` is set, and those made by `maker`.
pub async fn invitations_of(
    conn: &mut Conn,
    org: Option<Id>,
    system: bool,
    maker: Id,
) -> Result<Vec<InvitationRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        invitation_columns!(),
        " FROM invitations
          WHERE CASE WHEN $1::uuid IS NOT NULL THEN org_id = $1
                     ELSE (created_by = $3 OR ($2 AND kind = 'system')) END
          ORDER BY created_at DESC, id DESC"
    ))
    .bind(org)
    .bind(system)
    .bind(maker)
    .fetch_all(conn)
    .await?)
}

/// Takes an invitation back; the answer says whether it was good to take.
pub async fn revoke_invitation(
    conn: &mut Conn,
    id: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done =
        sqlx::query("UPDATE invitations SET revoked_at = $2 WHERE id = $1 AND revoked_at IS NULL")
            .bind(id)
            .bind(at)
            .execute(conn)
            .await?;
    Ok(done.rows_affected() == 1)
}

/// Counts one use of an invitation by `user`, if it is still good at `now`.
/// Two people at once cannot both take the last use.
pub async fn use_invitation(
    conn: &mut Conn,
    id: Id,
    user: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    // Capacity belongs to this invitation. Only a newly inserted per-person
    // receipt may increment its count; the last-place check and both writes
    // are one statement, including for callers in an explicit transaction.
    let counted = sqlx::query(
        "WITH available AS MATERIALIZED (
             SELECT id FROM invitations
              WHERE id=$1 AND revoked_at IS NULL AND expires_at>$3 AND uses<max_uses
              FOR UPDATE
         ), admitted AS (
             INSERT INTO invitation_uses (invitation_id,user_id,used_at)
             SELECT id,$2,$3 FROM available
             ON CONFLICT DO NOTHING RETURNING invitation_id
         )
         UPDATE invitations i SET uses=i.uses+1
          FROM admitted a WHERE i.id=a.invitation_id",
    )
    .bind(id)
    .bind(user)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(counted.rows_affected() == 1)
}

/// A service identity of an org.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ServiceRow {
    /// The user.
    pub id: Id,
    /// The org that keeps it.
    pub org_id: Id,
    /// Who made it.
    pub created_by: Option<Id>,
    /// When.
    pub created_at: DateTime<Utc>,
    /// How many tokens it has that were not revoked.
    pub token_count: i64,
}

/// The service identities that the org `org` keeps and that are not
/// disabled, the oldest first.
pub async fn service_accounts_of(conn: &mut Conn, org: Id) -> Result<Vec<ServiceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.service_org_id AS org_id, u.created_by, u.created_at,
                (SELECT count(*) FROM api_tokens t WHERE t.user_id = u.id AND t.revoked_at IS NULL) AS token_count
           FROM users u
          WHERE u.kind = 'service' AND u.service_org_id = $1 AND u.status = 'active'
          ORDER BY u.created_at, u.id",
    )
    .bind(org)
    .fetch_all(conn)
    .await?)
}

/// The service identity `id`, if it exists and is not disabled.
pub async fn service_account(conn: &mut Conn, id: Id) -> Result<Option<ServiceRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.service_org_id AS org_id, u.created_by, u.created_at,
                (SELECT count(*) FROM api_tokens t WHERE t.user_id = u.id AND t.revoked_at IS NULL) AS token_count
           FROM users u WHERE u.id = $1 AND u.kind = 'service' AND u.status = 'active'",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Makes a service identity of the org `org`. A username that is taken, by
/// a user or as the short name of a root org, is a taken unique value.
/// It has no person's device, so its clock remains fixed UTC.
pub async fn create_service_account(
    conn: &mut Conn,
    id: Id,
    org: Id,
    username: &str,
    display_name: &str,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let org_taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM orgs WHERE parent_id IS NULL AND slug = $1)",
    )
    .bind(username)
    .fetch_one(&mut *conn)
    .await?;
    if org_taken {
        return Err(DbError::Unique {
            constraint: "orgs_slug_key".to_owned(),
        });
    }
    sqlx::query(
        "INSERT INTO users (id, kind, service_org_id, username, display_name, avatar_color,
                            created_by, created_at, updated_at, timezone, timezone_mode)
         VALUES ($1, 'service', $2, $3, $4, $5, $6, $7, $7, 'UTC', 'fixed')",
    )
    .bind(id)
    .bind(org)
    .bind(username)
    .bind(display_name)
    .bind(avatar_color(id))
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Disables a service identity: it can sign in nowhere, its tokens are
/// revoked, and the roles it was given and the entries that name it are
/// taken away by `by`. The answer is how many roles and entries went.
pub async fn disable_service_account(
    conn: &mut Conn,
    id: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(i64, i64), DbError> {
    sqlx::query("UPDATE users SET status = 'suspended', updated_at = $2 WHERE id = $1")
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE api_tokens SET revoked_at = $2 WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(id)
        .bind(at)
        .execute(&mut *conn)
        .await?;
    let roles = sqlx::query(
        "UPDATE node_roles SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE subject_kind = 'user' AND subject_id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let entries = sqlx::query(
        "UPDATE permission_entries SET removed_at = $3, removed_by = $2, revision = revision + 1
          WHERE holder_kind = 'user' AND holder_id = $1 AND removed_at IS NULL",
    )
    .bind(id)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?
    .rows_affected();
    Ok((
        i64::try_from(roles).unwrap_or(i64::MAX),
        i64::try_from(entries).unwrap_or(i64::MAX),
    ))
}

/// A person as others see them: what a reference to one is made of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, sqlx::FromRow)]
pub struct UserBrief {
    /// Its identifier.
    pub id: Id,
    /// `human` or `service`.
    pub kind: String,
    /// The name it signs in by.
    pub username: String,
    /// The name people see.
    pub display_name: String,
    /// The colour of its avatar.
    pub avatar_color: i16,
}

/// The users `ids`, as others see them; one that is not there is not among
/// them.
pub async fn users_brief(conn: &mut Conn, ids: &[Id]) -> Result<Vec<UserBrief>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind, username, display_name, avatar_color FROM users WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Rows written directly, for tests of what stands on them. Not part of
/// any request: a real user is made by signing up.
pub mod testing {
    use crate::{Conn, DbError};
    use chrono::{DateTime, Utc};
    use orch_types::{Id, Json};

    /// A user named `username`, with nothing else set.
    pub async fn insert_user(conn: &mut Conn, id: Id, username: &str) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, $2, $2, 3)",
        )
        .bind(id)
        .bind(username)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// A session of `user` with one access token, both good until `until`.
    pub async fn insert_session(
        conn: &mut Conn,
        id: Id,
        user: Id,
        access_hash: &[u8],
        until: DateTime<Utc>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO sessions (id, user_id, refresh_hash, expires_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(user)
        .bind(id.as_bytes().as_slice())
        .bind(until)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "INSERT INTO access_tokens (token_hash, session_id, expires_at) VALUES ($1, $2, $3)",
        )
        .bind(access_hash)
        .bind(id)
        .bind(until)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// An API token of `user` that never expires and may do all its user may.
    pub async fn insert_api_token(
        conn: &mut Conn,
        user: Id,
        token_hash: &[u8],
    ) -> Result<Id, DbError> {
        let id = orch_types::new_id();
        sqlx::query(
            "INSERT INTO api_tokens (id, user_id, name, token_hash) VALUES ($1, $2, 'test', $3)",
        )
        .bind(id)
        .bind(user)
        .bind(token_hash)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// Suspends `user` everywhere.
    pub async fn suspend(conn: &mut Conn, user: Id) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Makes `user` an admin of the system.
    pub async fn make_admin(conn: &mut Conn, user: Id) -> Result<(), DbError> {
        sqlx::query("UPDATE users SET is_system_admin = true WHERE id = $1")
            .bind(user)
            .execute(conn)
            .await?;
        Ok(())
    }

    /// Switches on a mail server for the whole system, set up by `by`, and
    /// returns it.
    pub async fn insert_system_mail_channel(conn: &mut Conn, by: Id) -> Result<Id, DbError> {
        let id = orch_types::new_id();
        sqlx::query(
            "INSERT INTO notification_channels (id, kind, name, created_by) VALUES ($1, 'email', 'Mail', $2)",
        )
        .bind(id)
        .bind(by)
        .execute(conn)
        .await?;
        Ok(id)
    }

    /// What the outbox holds, the oldest first: `(channel, payload)`.
    pub async fn outbox(conn: &mut Conn) -> Result<Vec<(String, Json)>, DbError> {
        Ok(
            sqlx::query_as("SELECT channel, payload FROM outbox ORDER BY created_at, id")
                .fetch_all(conn)
                .await?,
        )
    }

    /// The rows of the audit log, the oldest first: `(action, object_id, details)`.
    pub async fn audit(conn: &mut Conn) -> Result<Vec<(String, Option<Id>, Json)>, DbError> {
        Ok(
            sqlx::query_as("SELECT action, object_id, details FROM audit_log ORDER BY at, id")
                .fetch_all(conn)
                .await?,
        )
    }

    /// The rows of the Inbox of `user`, the oldest first: `(kind, title, data)`.
    pub async fn inbox(conn: &mut Conn, user: Id) -> Result<Vec<(String, String, Json)>, DbError> {
        Ok(sqlx::query_as(
            "SELECT kind, title, data FROM notifications WHERE user_id = $1 ORDER BY inbox_seq",
        )
        .bind(user)
        .fetch_all(conn)
        .await?)
    }
}

// ---- Management over HTTP: the people a system admin lists, and the identity providers ----

/// An invitation to make one team org, as a system admin gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct OrgInvitation {
    /// The system admin who gave it.
    pub given_by: Id,
    /// When.
    pub given_at: DateTime<Utc>,
}

/// Gives the person `user` an invitation to make one team org, from the system admin `by`. A
/// person who holds one they have not used keeps it as it was: `None` then, and nothing changed;
/// a service identity makes no org and is given none.
pub async fn give_org_invitation(
    conn: &mut Conn,
    user: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<Option<OrgInvitation>, DbError> {
    Ok(sqlx::query_as(
        "UPDATE users SET org_invited_by = $2, org_invited_at = $3
          WHERE id = $1 AND kind = 'human' AND org_invited_at IS NULL
      RETURNING org_invited_by AS given_by, org_invited_at AS given_at",
    )
    .bind(user)
    .bind(by)
    .bind(at)
    .fetch_optional(conn)
    .await?)
}

/// Takes from the person `user` the invitation to make a team org they hold unused, and returns
/// it as it was; `None` when they held none. Taking it back and using it are the same write, so
/// of two at once one finds it and the other does not.
pub async fn take_org_invitation(
    conn: &mut Conn,
    user: Id,
) -> Result<Option<OrgInvitation>, DbError> {
    Ok(sqlx::query_as(
        "WITH held AS (
             SELECT id, org_invited_by, org_invited_at FROM users
              WHERE id = $1 AND org_invited_at IS NOT NULL
                FOR NO KEY UPDATE
         )
         UPDATE users u SET org_invited_by = NULL, org_invited_at = NULL
           FROM held
          WHERE u.id = held.id
      RETURNING held.org_invited_by AS given_by, held.org_invited_at AS given_at",
    )
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// What a list of people is narrowed to.
#[derive(Debug, Clone, Copy, Default)]
pub struct PeopleFilter<'a> {
    /// Only those whose name, username or address holds this, without regard to case.
    pub q: Option<&'a str>,
    /// Only those in this state: `active` or `suspended`.
    pub status: Option<&'a str>,
    /// Only system admins (`true`), or only those who are none (`false`).
    pub system_admin: Option<bool>,
}

/// The people (service identities are nobody here) in the order of their usernames, after the
/// username `after`, at most `limit`. A username is what names a place in the list because it is
/// unique and never changes: a person made or renamed meanwhile moves no one past the cursor.
pub async fn people(
    conn: &mut Conn,
    filter: PeopleFilter<'_>,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<UserRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT * FROM users
          WHERE kind = 'human'
            AND ($1::text IS NULL OR strpos(lower(username), lower($1)) > 0
                 OR strpos(lower(display_name), lower($1)) > 0
                 OR strpos(lower(COALESCE(email, '')), lower($1)) > 0)
            AND ($2::text IS NULL OR status = $2)
            AND ($3::bool IS NULL OR is_system_admin = $3)
            AND ($4::text IS NULL OR username COLLATE \"C\" > $4)
          ORDER BY username COLLATE \"C\"
          LIMIT $5",
    )
    .bind(filter.q)
    .bind(filter.status)
    .bind(filter.system_admin)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// The tries that the buckets named in `buckets` hold, each with its name; a bucket nobody has
/// tried at is not among them. One statement however many buckets are asked for.
pub async fn throttle_rows(
    conn: &mut Conn,
    buckets: &[String],
) -> Result<Vec<(String, Vec<DateTime<Utc>>)>, DbError> {
    Ok(
        sqlx::query_as("SELECT bucket, times FROM login_throttle WHERE bucket = ANY($1)")
            .bind(buckets)
            .fetch_all(conn)
            .await?,
    )
}

/// The owner kind under which the client secret of an identity provider is stored.
pub const IDENTITY_PROVIDER_SECRET_OWNER: &str = "identity_provider";
/// The name under which it is stored there.
pub const IDENTITY_PROVIDER_SECRET_NAME: &str = "client_secret";

/// An identity provider, as stored, and whether a client secret is kept for it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct IdentityProviderRow {
    /// Its identifier.
    pub id: Id,
    /// Its key, which appears in callback addresses.
    pub key: String,
    /// The label of its button.
    pub name: String,
    /// The issuer.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The scopes asked for.
    pub scopes: Vec<String>,
    /// The claim that names the person.
    pub claim_subject: String,
    /// The claim that holds their address.
    pub claim_email: String,
    /// The claim that holds their name.
    pub claim_name: String,
    /// The claim that holds their username.
    pub claim_username: String,
    /// The domains of addresses let in; none for any.
    pub allowed_email_domains: Vec<String>,
    /// Whether a user is made at the first sign-in.
    pub auto_register: bool,
    /// The org joined at auto-registration.
    pub default_org_id: Option<Id>,
    /// Their role in that org.
    pub default_role: Option<String>,
    /// Whether it is offered.
    pub enabled: bool,
    /// How many times it was changed, and one.
    pub revision: i64,
    /// Whether a client secret is kept.
    pub client_secret_set: bool,
}

macro_rules! identity_provider_columns {
    () => {
        "p.id, p.key, p.name, p.issuer, p.client_id, p.scopes, p.claim_subject, p.claim_email,
         p.claim_name, p.claim_username, p.allowed_email_domains, p.auto_register, p.default_org_id,
         p.default_role, p.enabled, p.revision,
         EXISTS (SELECT 1 FROM secrets s WHERE s.owner_kind = 'identity_provider' AND s.owner_id = p.id
                    AND s.name = 'client_secret') AS client_secret_set"
    };
}

/// Every identity provider, by the label of its button.
pub async fn identity_providers(conn: &mut Conn) -> Result<Vec<IdentityProviderRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        identity_provider_columns!(),
        " FROM identity_providers p ORDER BY lower(p.name), p.key"
    ))
    .fetch_all(conn)
    .await?)
}

/// The identity provider `id`.
pub async fn identity_provider(
    conn: &mut Conn,
    id: Id,
) -> Result<Option<IdentityProviderRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        identity_provider_columns!(),
        " FROM identity_providers p WHERE p.id = $1"
    ))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// What an identity provider is made from.
#[derive(Debug, Clone, Copy)]
pub struct NewIdentityProvider<'a> {
    /// Its identifier.
    pub id: Id,
    /// Its key.
    pub key: &'a str,
    /// The label of its button.
    pub name: &'a str,
    /// The issuer.
    pub issuer: &'a str,
    /// The client identifier.
    pub client_id: &'a str,
    /// The scopes asked for.
    pub scopes: &'a [String],
    /// The claim that names the person.
    pub claim_subject: &'a str,
    /// The claim that holds their address.
    pub claim_email: &'a str,
    /// The claim that holds their name.
    pub claim_name: &'a str,
    /// The claim that holds their username.
    pub claim_username: &'a str,
    /// The domains of addresses let in.
    pub allowed_email_domains: &'a [String],
    /// Whether a user is made at the first sign-in.
    pub auto_register: bool,
    /// The org joined at auto-registration.
    pub default_org_id: Option<Id>,
    /// Their role in that org.
    pub default_role: Option<&'a str>,
    /// Whether it is offered.
    pub enabled: bool,
}

/// Adds an identity provider at revision 1. A key that is taken is `DbError::Unique`.
pub async fn insert_identity_provider(
    conn: &mut Conn,
    new: &NewIdentityProvider<'_>,
    at: DateTime<Utc>,
) -> Result<IdentityProviderRow, DbError> {
    sqlx::query(
        "INSERT INTO identity_providers (id, key, name, issuer, client_id, scopes, claim_subject,
                claim_email, claim_name, claim_username, allowed_email_domains, auto_register,
                default_org_id, default_role, enabled, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $16)",
    )
    .bind(new.id)
    .bind(new.key)
    .bind(new.name)
    .bind(new.issuer)
    .bind(new.client_id)
    .bind(new.scopes)
    .bind(new.claim_subject)
    .bind(new.claim_email)
    .bind(new.claim_name)
    .bind(new.claim_username)
    .bind(new.allowed_email_domains)
    .bind(new.auto_register)
    .bind(new.default_org_id)
    .bind(new.default_role)
    .bind(new.enabled)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        identity_provider_columns!(),
        " FROM identity_providers p WHERE p.id = $1"
    ))
    .bind(new.id)
    .fetch_one(conn)
    .await?)
}

/// What a change of an identity provider sets; what is left out stays.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityProviderChange<'a> {
    /// A new key.
    pub key: Option<&'a str>,
    /// A new label.
    pub name: Option<&'a str>,
    /// A new issuer.
    pub issuer: Option<&'a str>,
    /// A new client identifier.
    pub client_id: Option<&'a str>,
    /// New scopes.
    pub scopes: Option<&'a [String]>,
    /// A new claim for the person's identity.
    pub claim_subject: Option<&'a str>,
    /// A new claim for their address.
    pub claim_email: Option<&'a str>,
    /// A new claim for their name.
    pub claim_name: Option<&'a str>,
    /// A new claim for their username.
    pub claim_username: Option<&'a str>,
    /// New domains of addresses let in.
    pub allowed_email_domains: Option<&'a [String]>,
    /// Whether a user is made at the first sign-in.
    pub auto_register: Option<bool>,
    /// A new org joined at auto-registration, or none.
    pub default_org_id: Option<Option<Id>>,
    /// A new role in that org, or none.
    pub default_role: Option<Option<&'a str>>,
    /// Whether it is offered.
    pub enabled: Option<bool>,
}

/// Changes the identity provider `id` if it is at `expected_revision`, and moves the revision on.
/// None when there is no such provider or it was changed meanwhile. A key that is taken is
/// `DbError::Unique`.
pub async fn change_identity_provider(
    conn: &mut Conn,
    id: Id,
    expected_revision: i64,
    change: &IdentityProviderChange<'_>,
    at: DateTime<Utc>,
) -> Result<Option<IdentityProviderRow>, DbError> {
    let changed = sqlx::query(
        "UPDATE identity_providers SET
                key = COALESCE($3, key), name = COALESCE($4, name), issuer = COALESCE($5, issuer),
                client_id = COALESCE($6, client_id), scopes = COALESCE($7, scopes),
                claim_subject = COALESCE($8, claim_subject), claim_email = COALESCE($9, claim_email),
                claim_name = COALESCE($10, claim_name), claim_username = COALESCE($11, claim_username),
                allowed_email_domains = COALESCE($12, allowed_email_domains),
                auto_register = COALESCE($13, auto_register),
                default_org_id = CASE WHEN $14 THEN $15 ELSE default_org_id END,
                default_role = CASE WHEN $16 THEN $17 ELSE default_role END,
                enabled = COALESCE($18, enabled), revision = revision + 1, updated_at = $19
          WHERE id = $1 AND revision = $2",
    )
    .bind(id)
    .bind(expected_revision)
    .bind(change.key)
    .bind(change.name)
    .bind(change.issuer)
    .bind(change.client_id)
    .bind(change.scopes)
    .bind(change.claim_subject)
    .bind(change.claim_email)
    .bind(change.claim_name)
    .bind(change.claim_username)
    .bind(change.allowed_email_domains)
    .bind(change.auto_register)
    .bind(change.default_org_id.is_some())
    .bind(change.default_org_id.flatten())
    .bind(change.default_role.is_some())
    .bind(change.default_role.flatten())
    .bind(change.enabled)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(None);
    }
    identity_provider(conn, id).await
}

/// Removes the identity provider `id` with the client secret kept for it and the sign-ins that were
/// linked through it. Says whether there was one.
pub async fn delete_identity_provider(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    sqlx::query("DELETE FROM secrets WHERE owner_kind = 'identity_provider' AND owner_id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(sqlx::query("DELETE FROM identity_providers WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?
        .rows_affected()
        > 0)
}

/// The active people for whom the identity provider `provider` is the only way to sign in: they
/// have no password and no sign-in through any other provider. Removing the provider would shut
/// them out.
pub async fn only_signed_in_through(conn: &mut Conn, provider: Id) -> Result<Vec<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.username FROM users u
          WHERE u.kind = 'human' AND u.status = 'active' AND u.password_hash IS NULL
            AND EXISTS (SELECT 1 FROM user_identities i
                         WHERE i.user_id = u.id AND i.identity_provider_id = $1)
            AND NOT EXISTS (SELECT 1 FROM user_identities i
                             WHERE i.user_id = u.id AND i.identity_provider_id <> $1)
          ORDER BY u.username",
    )
    .bind(provider)
    .fetch_all(conn)
    .await?)
}

/// The identity provider whose key is `key`.
pub async fn identity_provider_by_key(
    conn: &mut Conn,
    key: &str,
) -> Result<Option<IdentityProviderRow>, DbError> {
    Ok(sqlx::query_as(concat!(
        "SELECT ",
        identity_provider_columns!(),
        " FROM identity_providers p WHERE p.key = $1"
    ))
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

/// A sign-in at an identity provider that was begun and has not ended: a row of `oidc_states`. It is
/// a sign-in, a link to the signed-in user `link_user_id`, or a fresh proof for one act of the session
/// `session_id` (`sensitive_purpose`).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SignInState {
    /// Names the sign-in; the browser brings it back.
    pub state: String,
    /// The provider it was begun with.
    pub identity_provider_id: Id,
    /// What ties the provider's answer to it.
    pub nonce: String,
    /// The secret half of its proof key.
    pub pkce_verifier: String,
    /// Where the browser goes back to.
    pub redirect_path: String,
    /// The signed-in user an identity is linked to; none otherwise.
    pub link_user_id: Option<Id>,
    /// The invitation a sign-in came with, as it was given.
    pub invitation: Option<String>,
    /// Whether the person explicitly composed acceptance with this sign-in.
    pub accept_invitation: bool,
    /// The browser session a fresh proof is for; none otherwise.
    pub session_id: Option<Id>,
    /// The act a fresh proof is for; none otherwise.
    pub sensitive_purpose: Option<String>,
    /// The service identity a token proof is for.
    pub service_account_id: Option<Id>,
    /// The SHA-256 of the mark of the browser that began it, which alone may end it.
    pub browser_hash: Vec<u8>,
    /// When it was begun: a fresh proof's sign-in at the provider must be at or after it.
    pub started_at: DateTime<Utc>,
    /// When it ends.
    pub expires_at: DateTime<Utc>,
}

/// Keeps a sign-in that was begun.
pub async fn begin_sign_in(conn: &mut Conn, row: &SignInState) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO oidc_states (state, identity_provider_id, nonce, pkce_verifier, redirect_path,
                                  link_user_id, invitation, session_id, sensitive_purpose,
                                  service_account_id, browser_hash, started_at, expires_at, accept_invitation)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(&row.state)
    .bind(row.identity_provider_id)
    .bind(&row.nonce)
    .bind(&row.pkce_verifier)
    .bind(&row.redirect_path)
    .bind(row.link_user_id)
    .bind(&row.invitation)
    .bind(row.session_id)
    .bind(&row.sensitive_purpose)
    .bind(row.service_account_id)
    .bind(&row.browser_hash)
    .bind(row.started_at)
    .bind(row.expires_at)
    .bind(row.accept_invitation)
    .execute(conn)
    .await?;
    Ok(())
}

/// Takes the sign-in named `state`, so that it serves once: it is gone after this, whatever comes of
/// it. One statement, so that of two callbacks with the same state one has it and the other nothing.
pub async fn take_sign_in(conn: &mut Conn, state: &str) -> Result<Option<SignInState>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM oidc_states WHERE state = $1
         RETURNING state, identity_provider_id, nonce, pkce_verifier, redirect_path, link_user_id,
                   invitation, session_id, sensitive_purpose, service_account_id, browser_hash,
                   started_at, expires_at, accept_invitation",
    )
    .bind(state)
    .fetch_optional(conn)
    .await?)
}

/// The user the identity `subject` of the provider `provider` is linked to, and whether a system admin
/// suspended them.
pub async fn identity_account(
    conn: &mut Conn,
    provider: Id,
    subject: &str,
) -> Result<Option<(Id, bool)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.status = 'suspended'
           FROM user_identities i JOIN users u ON u.id = i.user_id
          WHERE i.identity_provider_id = $1 AND i.subject = $2",
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(conn)
    .await?)
}

/// The subject `user` is linked as at the provider `provider`, if they are linked there.
pub async fn identity_at(
    conn: &mut Conn,
    provider: Id,
    user: Id,
) -> Result<Option<String>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT subject FROM user_identities WHERE identity_provider_id = $1 AND user_id = $2
          ORDER BY linked_at LIMIT 1",
    )
    .bind(provider)
    .bind(user)
    .fetch_optional(conn)
    .await?)
}

/// An identity to link to a user.
#[derive(Debug, Clone, Copy)]
pub struct IdentityLink<'a> {
    /// The provider.
    pub provider: Id,
    /// Who the person is there.
    pub subject: &'a str,
    /// The user.
    pub user: Id,
    /// The address the provider gave.
    pub email: Option<&'a str>,
    /// When it is linked, which is also a sign-in through it.
    pub at: DateTime<Utc>,
}

/// Links an identity to a user; an identity that is linked already, to anybody, is left as it is. The
/// answer says whether this linked it.
pub async fn link_identity(conn: &mut Conn, link: &IdentityLink<'_>) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO user_identities (identity_provider_id, subject, user_id, email, linked_at, last_login_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (identity_provider_id, subject) DO NOTHING",
    )
    .bind(link.provider)
    .bind(link.subject)
    .bind(link.user)
    .bind(link.email)
    .bind(link.at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Notes a sign-in through an identity at `at`, with the address the provider gave this time.
pub async fn note_identity_sign_in(
    conn: &mut Conn,
    provider: Id,
    subject: &str,
    email: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE user_identities SET last_login_at = $3, email = $4
          WHERE identity_provider_id = $1 AND subject = $2",
    )
    .bind(provider)
    .bind(subject)
    .bind(at)
    .bind(email)
    .execute(conn)
    .await?;
    Ok(())
}

/// The ways a person signs in: whether they have a password, and the providers their identities are
/// at. The person's row is locked until the transaction ends, so that two requests that each take one
/// way away cannot both pass a count of what is left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaysIn {
    /// They have a password.
    pub password: bool,
    /// The providers they are linked at, one entry for each identity.
    pub providers: Vec<Id>,
}

/// The ways `user` signs in, with their row locked (see [`WaysIn`]).
pub async fn lock_ways_in(conn: &mut Conn, user: Id) -> Result<WaysIn, DbError> {
    let password: Option<bool> =
        sqlx::query_scalar("SELECT password_hash IS NOT NULL FROM users WHERE id = $1 FOR UPDATE")
            .bind(user)
            .fetch_optional(&mut *conn)
            .await?;
    let providers: Vec<Id> = sqlx::query_scalar(
        "SELECT identity_provider_id FROM user_identities WHERE user_id = $1 ORDER BY linked_at",
    )
    .bind(user)
    .fetch_all(conn)
    .await?;
    Ok(WaysIn {
        password: password.unwrap_or(false),
        providers,
    })
}

/// Unlinks the identities `user` has at the provider `provider`; the answer says how many there were.
pub async fn unlink_identity(conn: &mut Conn, provider: Id, user: Id) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM user_identities WHERE identity_provider_id = $1 AND user_id = $2")
            .bind(provider)
            .bind(user)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}

/// The usernames and root-org slugs that begin with `stem`: what a name made from it may not be. The
/// two share one namespace, and a username is never given again, so every user counts, suspended or
/// not.
pub async fn names_taken_from(conn: &mut Conn, stem: &str) -> Result<Vec<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT name FROM public_names WHERE starts_with(name, $1)")
            .bind(stem)
            .fetch_all(conn)
            .await?,
    )
}

/// Holder definitions for a mention page, without expanding a group's roster in application memory.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MentionSources {
    /// Direct and grouped holders, with parallel identifier, minimum-role and owner-root arrays.
    pub kinds: Vec<String>,
    /// One identifier per holder.
    pub ids: Vec<Option<Id>>,
    /// One minimum role per holder.
    pub minimum_roles: Vec<String>,
    /// Owners must remain members of this root; other entries have no extra gate.
    pub owner_roots: Vec<Option<Id>>,
    /// Implicit role places exclude page-only guests.
    pub implicit_places: Vec<Id>,
    /// Candidate role levels, authoritatively rechecked on the document before returning names.
    pub implicit_roles: Vec<String>,
    /// Explicit role places can include page-only viewers.
    pub explicit_places: Vec<Id>,
    /// Candidate explicit role levels.
    pub explicit_roles: Vec<String>,
    /// Everyone is the active membership of these roots.
    pub member_roots: Vec<Id>,
    /// Whether the role sources include system administrators.
    pub system_admins: bool,
}

/// SQL expands holder sources once and returns only one bounded keyset page of matching identities.
pub const MENTION_READER_SQL: &str = "WITH matching AS MATERIALIZED (
    SELECT id,kind,username,display_name,avatar_color,is_system_admin FROM users WHERE id=ANY($12::uuid[])
  ), potential AS (
    (SELECT h.user_id FROM access_holder_people_many($1::text[],$2::uuid[],$3::text[],$4,true,$12::uuid[]) h
      JOIN unnest($5::uuid[]) WITH ORDINALITY AS gate(root,idx) ON gate.idx=h.wanted
      JOIN matching u ON u.id=h.user_id
      WHERE gate.root IS NULL OR EXISTS (SELECT 1 FROM org_members m WHERE m.org_id=gate.root AND m.user_id=h.user_id)
      ORDER BY h.user_id LIMIT $13)
    UNION (SELECT h.user_id FROM access_role_people($6::uuid[],$7::text[],false,$4,true,$12::uuid[]) h
      JOIN matching u ON u.id=h.user_id WHERE cardinality($6::uuid[])>0 ORDER BY h.user_id LIMIT $13)
    UNION (SELECT h.user_id FROM access_role_people($8::uuid[],$9::text[],true,$4,true,$12::uuid[]) h
      JOIN matching u ON u.id=h.user_id WHERE cardinality($8::uuid[])>0 ORDER BY h.user_id LIMIT $13)
    UNION (SELECT m.user_id FROM org_members m JOIN matching u ON u.id=m.user_id
      WHERE m.org_id=ANY($10::uuid[]) ORDER BY m.user_id LIMIT $13)
    UNION (SELECT id FROM matching WHERE $11 AND is_system_admin ORDER BY id LIMIT $13)
  ) SELECT u.id,u.kind,u.username,u.display_name,u.avatar_color FROM potential p JOIN matching u ON u.id=p.user_id
    ORDER BY u.id LIMIT $13";

/// The people whose names match, a batch at a time in identifier order: what [`MENTION_READER_SQL`] checks.
const MENTION_CANDIDATES_SQL: &str = "SELECT id FROM users
    WHERE kind='human' AND status='active' AND ($1::uuid IS NULL OR id>$1)
      AND (lower(username)||' '||lower(display_name)) LIKE $2 ESCAPE '\\' AND created_at<=$3
    ORDER BY id LIMIT $4";

/// How many people whose names match are checked at once, for each one a page shows. A batch walks the grants of the
/// document once and looks each of its people up in the groups and orgs they name, so a larger batch spends fewer
/// walks on a page whose readers are rare among the matches and a smaller one reads less when they are common.
const MENTION_CANDIDATES_PER_READER: i64 = 4;

/// A directory page preserves the existing visibility rule and uses an identifier continuation, never OFFSET.
pub const MENTION_DIRECTORY_SQL: &str = "WITH visible AS MATERIALIZED (
    (SELECT a.user_id FROM org_members a JOIN org_members b ON a.org_id=b.org_id
      JOIN users u ON u.id=a.user_id
      WHERE b.user_id=$1 AND u.kind='human' AND u.status='active'
        AND ($2::uuid IS NULL OR u.id>$2) AND u.created_at<=$6
        AND (lower(u.username)||' '||lower(u.display_name)) LIKE $4 ESCAPE '\\'
      GROUP BY a.user_id ORDER BY a.user_id LIMIT $5)
    UNION SELECT id FROM users WHERE kind='human' AND status='active'
      AND (username=$3 OR lower(email)=$3) AND ($2::uuid IS NULL OR id>$2) AND created_at<=$6
  ) SELECT u.id,u.kind,u.username,u.display_name,u.avatar_color FROM visible v JOIN users u ON u.id=v.user_id
    ORDER BY u.id LIMIT $5";

fn mention_pattern(text: &str) -> String {
    format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// One matching potential-reader page; the producer verifies current document View for each bounded candidate. The
/// people whose names match are taken a batch at a time, in identifier order, and only they are looked up among those
/// the document's grants reach, so a group or an org of any size is never listed whole for a page.
pub async fn mention_readers_page(
    conn: &mut Conn,
    sources: &MentionSources,
    after: Option<Id>,
    text: &str,
    limit: i64,
    now: chrono::DateTime<chrono::Utc>,
    snapshot: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<UserBrief>, DbError> {
    let pattern = mention_pattern(text);
    let batch = limit.max(1).saturating_mul(MENTION_CANDIDATES_PER_READER);
    let mut found: Vec<UserBrief> = Vec::new();
    let mut cursor = after;
    while i64::try_from(found.len()).unwrap_or(i64::MAX) < limit {
        let candidates: Vec<Id> = sqlx::query_scalar(MENTION_CANDIDATES_SQL)
            .bind(cursor)
            .bind(&pattern)
            .bind(snapshot)
            .bind(batch)
            .fetch_all(&mut *conn)
            .await?;
        let Some(last) = candidates.last().copied() else {
            break;
        };
        let wanted = limit - i64::try_from(found.len()).unwrap_or(i64::MAX);
        let readers: Vec<UserBrief> = sqlx::query_as(MENTION_READER_SQL)
            .bind(&sources.kinds)
            .bind(&sources.ids)
            .bind(&sources.minimum_roles)
            .bind(now)
            .bind(&sources.owner_roots)
            .bind(&sources.implicit_places)
            .bind(&sources.implicit_roles)
            .bind(&sources.explicit_places)
            .bind(&sources.explicit_roles)
            .bind(&sources.member_roots)
            .bind(sources.system_admins)
            .bind(&candidates)
            .bind(wanted)
            .fetch_all(&mut *conn)
            .await?;
        found.extend(readers);
        if i64::try_from(candidates.len()).unwrap_or(i64::MAX) < batch {
            break;
        }
        cursor = Some(last);
    }
    Ok(found)
}

/// One matching shareable-directory page; exact outside names remain visible as in ordinary person search.
pub async fn mention_directory_page(
    conn: &mut Conn,
    reader: Id,
    after: Option<Id>,
    text: &str,
    limit: i64,
    snapshot: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<UserBrief>, DbError> {
    Ok(sqlx::query_as(MENTION_DIRECTORY_SQL)
        .bind(reader)
        .bind(after)
        .bind(text)
        .bind(mention_pattern(text))
        .bind(limit)
        .bind(snapshot)
        .fetch_all(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_testkit::{clock, database::TestDb};
    use orch_types::new_id;

    async fn user(conn: &mut Conn, name: &str) -> Id {
        let id = new_id();
        sqlx::query(
            "INSERT INTO users (id, username, display_name, avatar_color) VALUES ($1, $2, $2, 3)",
        )
        .bind(id)
        .bind(name)
        .execute(conn)
        .await
        .unwrap();
        id
    }

    fn person<'a>(id: Id, username: &'a str) -> NewUser<'a> {
        NewUser {
            id,
            username,
            display_name: "Ada Lovelace",
            email: Some("ada@example.org"),
            email_confirmed: false,
            password_hash: None,
            locale: "en",
            is_system_admin: false,
            created_by: None,
            personal_org: Some(PersonalOrg {
                closed: true,
                flat: true,
            }),
        }
    }

    #[tokio::test]
    async fn a_person_inserted_without_a_zone_choice_follows_the_device_by_schema_default() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let id = new_id();
        sqlx::query("INSERT INTO users (id, username, display_name, avatar_color, locale) VALUES ($1, 'ada', 'Ada', 3, 'en')")
            .bind(id).execute(&mut *conn).await.unwrap();
        let stored = user_by_id(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(stored.timezone_mode, "device");
        assert_eq!(stored.timezone, "UTC");
        assert!(stored.timezone_reported_at.is_none());
    }

    #[tokio::test]
    async fn a_headless_service_identity_keeps_a_fixed_clock_without_a_persons_device() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let at = clock::start();
        let owner = create_user(&mut conn, &person(new_id(), "ada"), at)
            .await
            .unwrap();
        let service = new_id();
        create_service_account(
            &mut conn,
            service,
            owner
                .personal_org_id
                .expect("the fixture requests a personal org"),
            "buildbot",
            "Build bot",
            owner.user.id,
            at,
        )
        .await
        .unwrap();
        let stored = user_by_id(&mut conn, service).await.unwrap().unwrap();
        assert_eq!(stored.kind, "service");
        assert!(stored.personal_org_id.is_none());
        assert_eq!(stored.timezone_mode, "fixed");
        assert_eq!(stored.timezone, "UTC");
    }

    #[tokio::test]
    async fn an_access_token_stands_for_its_session_until_either_ends() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let ada = user(&mut conn, "ada").await;
        let session = new_id();
        sqlx::query(
            "INSERT INTO sessions (id, user_id, refresh_hash, expires_at) VALUES ($1, $2, 'r', $3)",
        )
        .bind(session)
        .bind(ada)
        .bind(now + Duration::days(30))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO access_tokens (token_hash, session_id, expires_at) VALUES ('t', $1, $2)",
        )
        .bind(session)
        .bind(now + Duration::minutes(15))
        .execute(&mut *conn)
        .await
        .unwrap();

        let found = session_by_access_token(&mut conn, b"t", now).await.unwrap();
        assert_eq!(
            found,
            Some(AccessRow {
                as_admin: false,
                session_id: session,
                user_id: ada
            })
        );
        assert_eq!(
            session_by_access_token(&mut conn, b"other", now)
                .await
                .unwrap(),
            None
        );
        // The token ran out.
        assert_eq!(
            session_by_access_token(&mut conn, b"t", now + Duration::minutes(16))
                .await
                .unwrap(),
            None
        );
        // The session was revoked.
        sqlx::query("UPDATE sessions SET revoked_at = $1")
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(
            session_by_access_token(&mut conn, b"t", now).await.unwrap(),
            None
        );
        assert_eq!(
            user_by_id(&mut conn, ada).await.unwrap().unwrap().username,
            "ada"
        );
    }

    #[tokio::test]
    async fn an_api_token_is_good_until_it_is_revoked_or_expires() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let ada = user(&mut conn, "ada").await;
        let token = new_id();
        sqlx::query("INSERT INTO api_tokens (id, user_id, name, token_hash, scopes, expires_at) VALUES ($1, $2, 'ci', 'h', ARRAY['*'], $3)")
            .bind(token)
            .bind(ada)
            .bind(now + Duration::days(1))
            .execute(&mut *conn)
            .await
            .unwrap();
        let found = api_token_by_hash(&mut conn, b"h", now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (found.id, found.user_id, found.scopes),
            (token, ada, vec!["*".to_owned()])
        );
        assert_eq!(
            api_token_by_hash(&mut conn, b"h", now + Duration::days(2))
                .await
                .unwrap(),
            None
        );
        sqlx::query("UPDATE api_tokens SET revoked_at = $1")
            .bind(now)
            .execute(&mut *conn)
            .await
            .unwrap();
        assert_eq!(api_token_by_hash(&mut conn, b"h", now).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_person_is_made_with_an_org_of_their_own_that_they_alone_are_in() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        let id = new_id();
        let made = create_user(&mut conn, &person(id, "ada"), now)
            .await
            .unwrap();
        let personal = made
            .personal_org_id
            .expect("the fixture requests a personal org");
        assert_eq!(made.user.personal_org_id, Some(personal));
        assert_eq!(made.user.created_at, now);
        assert_eq!(made.user.avatar_color, avatar_color(id));
        assert_eq!(made.user.timezone_mode, "device");
        assert_eq!(made.user.timezone, "UTC");
        // Not confirmed, as it was not said to be.
        assert!(made.user.email.is_some() && !made.user.email_confirmed());

        let org: (String, String, bool, bool, String) = sqlx::query_as(
            "SELECT o.slug, o.org_kind, o.closed, o.flat, n.name
               FROM orgs o JOIN nodes n ON n.id = o.node_id WHERE o.node_id = $1",
        )
        .bind(made.personal_org_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            org,
            (
                "ada".into(),
                "personal".into(),
                true,
                true,
                "Ada Lovelace".into()
            )
        );
        let members: Vec<(Id, Id)> =
            sqlx::query_as("SELECT user_id, domain_id FROM org_members WHERE org_id = $1")
                .bind(made.personal_org_id)
                .fetch_all(&mut *conn)
                .await
                .unwrap();
        assert_eq!(members, vec![(id, personal)]);
        let claim: (Option<Id>, Option<Id>) =
            sqlx::query_as("SELECT user_id,org_id FROM public_names WHERE name='ada'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(claim, (Some(id), Some(personal)));
    }

    async fn root_named(conn: &mut Conn, owner: Id, slug: &str) -> Result<(), DbError> {
        use crate::queries::hierarchy::{self, NewNode, NewOrg};
        hierarchy::create(
            conn,
            &NewNode {
                id: new_id(),
                kind: "org",
                parent: None,
                name: slug,
                name_by_person: true,
                description: "",
                description_by_person: false,
                owner,
                org: Some(NewOrg {
                    slug,
                    org_kind: "team",
                }),
                at: clock::start(),
            },
        )
        .await
    }

    #[tokio::test]
    async fn a_human_without_a_personal_org_still_owns_its_public_name_without_home_rows() {
        let test = TestDb::fresh().await;
        let mut tx = test.pool.begin().await.unwrap();
        let id = new_id();
        let mut input = person(id, "solo");
        input.personal_org = None;
        let made = create_user(&mut tx, &input, clock::start()).await.unwrap();
        assert_eq!(made.personal_org_id, None);
        assert_eq!(made.user.personal_org_id, None);
        for query in [
            "SELECT count(*) FROM nodes WHERE owner_id=$1",
            "SELECT count(*) FROM org_members WHERE user_id=$1",
            "SELECT count(*) FROM chats WHERE created_by=$1",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(query)
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap(),
                0
            );
        }
        let claim: (Option<Id>, Option<Id>) =
            sqlx::query_as("SELECT user_id,org_id FROM public_names WHERE name='solo'")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(claim, (Some(id), None));
        tx.commit().await.unwrap();
        let mut attempt = test.pool.begin().await.unwrap();
        let refused = root_named(&mut attempt, id, "solo").await.unwrap_err();
        assert!(matches!(refused, DbError::Unique { .. }));
        attempt.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn a_root_name_also_blocks_an_account_without_a_home_and_rollback_releases_a_claim() {
        let test = TestDb::fresh().await;
        let mut tx = test.pool.begin().await.unwrap();
        let owner = new_id();
        let mut input = person(owner, "allocator");
        input.personal_org = None;
        create_user(&mut tx, &input, clock::start()).await.unwrap();
        root_named(&mut tx, owner, "reserved").await.unwrap();
        tx.commit().await.unwrap();
        let mut attempt = test.pool.begin().await.unwrap();
        let mut input = person(new_id(), "reserved");
        input.personal_org = None;
        input.email = None;
        assert!(matches!(
            create_user(&mut attempt, &input, clock::start()).await,
            Err(DbError::Unique { .. })
        ));
        attempt.rollback().await.unwrap();
        let mut attempt = test.pool.begin().await.unwrap();
        let mut input = person(new_id(), "rolled-back");
        input.personal_org = None;
        input.email = None;
        create_user(&mut attempt, &input, clock::start())
            .await
            .unwrap();
        attempt.rollback().await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM public_names WHERE name='rolled-back'")
                .fetch_one(&test.pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
        let mut attempt = test.pool.begin().await.unwrap();
        create_user(&mut attempt, &input, clock::start())
            .await
            .unwrap();
        attempt.commit().await.unwrap();
    }

    #[tokio::test]
    async fn an_account_and_a_root_claiming_one_public_name_concurrently_have_one_winner() {
        let test = TestDb::fresh().await;
        let owner = new_id();
        let mut input = person(owner, "allocator");
        input.personal_org = None;
        let mut setup = test.pool.begin().await.unwrap();
        create_user(&mut setup, &input, clock::start())
            .await
            .unwrap();
        setup.commit().await.unwrap();
        let gate = tokio::sync::Barrier::new(2);
        let account = async {
            let mut tx = test.pool.begin().await.unwrap();
            let mut input = person(new_id(), "racing");
            input.personal_org = None;
            input.email = None;
            gate.wait().await;
            let result = create_user(&mut tx, &input, clock::start()).await;
            if result.is_ok() {
                tx.commit().await.unwrap();
                true
            } else {
                tx.rollback().await.unwrap();
                assert!(matches!(result, Err(DbError::Unique { .. })));
                false
            }
        };
        let organization = async {
            let mut tx = test.pool.begin().await.unwrap();
            gate.wait().await;
            let result = root_named(&mut tx, owner, "racing").await;
            if result.is_ok() {
                tx.commit().await.unwrap();
                true
            } else {
                tx.rollback().await.unwrap();
                assert!(matches!(result, Err(DbError::Unique { .. })));
                false
            }
        };
        let (account, organization) = tokio::join!(account, organization);
        assert_ne!(
            account, organization,
            "one public handle has one independent owner"
        );
        let counts:(i64,i64)=sqlx::query_as(
            "SELECT (SELECT count(*) FROM users WHERE username='racing'),(SELECT count(*) FROM orgs WHERE parent_id IS NULL AND slug='racing')",
        ).fetch_one(&test.pool).await.unwrap();
        assert_eq!(counts.0 + counts.1, 1);
    }

    #[tokio::test]
    async fn a_name_that_an_org_has_as_its_short_name_is_taken_for_a_username() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let now = clock::start();
        create_user(&mut conn, &person(new_id(), "ada"), now)
            .await
            .unwrap();
        // The second person's own org would have the same short name as the first's.
        let mut other = person(new_id(), "ada");
        other.email = None;
        let refused = create_user(&mut conn, &other, now).await.unwrap_err();
        assert!(matches!(refused, DbError::Unique { .. }), "{refused:?}");
    }

    #[test]
    fn the_colour_of_an_avatar_is_one_of_twelve_and_the_same_for_the_same_user() {
        let id = new_id();
        assert_eq!(avatar_color(id), avatar_color(id));
        for _ in 0..200 {
            assert!((0..12).contains(&avatar_color(new_id())));
        }
        // The first eight bytes of the digest, as a big-endian number, modulo 12.
        let digest = Sha256::digest(Id::nil().as_bytes());
        let number = u64::from_be_bytes(digest[..8].try_into().unwrap());
        assert_eq!(i64::from(avatar_color(Id::nil())), (number % 12) as i64);
    }

    #[tokio::test]
    async fn a_name_finds_a_person_by_username_or_address_without_regard_to_case() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let id = new_id();
        create_user(&mut conn, &person(id, "ada"), clock::start())
            .await
            .unwrap();
        for name in ["ada", "ADA", "Ada@Example.org", "ada@example.org"] {
            let found = user_by_login(&mut conn, name).await.unwrap();
            assert_eq!(found.map(|user| user.id), Some(id), "{name}");
        }
        assert!(user_by_login(&mut conn, "adam").await.unwrap().is_none());
        // An address that was not confirmed finds nobody as one to send mail to.
        assert!(
            user_by_confirmed_email(&mut conn, "ada@example.org")
                .await
                .unwrap()
                .is_none()
        );
        confirm_current_email(&mut conn, id, clock::start())
            .await
            .unwrap();
        assert!(
            user_by_confirmed_email(&mut conn, "ADA@example.org")
                .await
                .unwrap()
                .is_some()
        );
        assert!(!no_users_yet(&mut conn).await.unwrap());
    }

    fn bound(limit: u32) -> Bound {
        Bound {
            limit,
            window: Duration::hours(3),
            gap: None,
        }
    }

    #[tokio::test]
    async fn a_count_refuses_the_try_after_its_limit_and_says_when_the_oldest_has_aged() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        for index in 0..3 {
            let at = start + Duration::minutes(index);
            let reserved = reserve_try(&mut conn, "password|u:x", at, &bound(3))
                .await
                .unwrap();
            assert_eq!(reserved, Reservation::Counted { filled: index == 2 });
        }
        let at = start + Duration::minutes(10);
        let refused = reserve_try(&mut conn, "password|u:x", at, &bound(3))
            .await
            .unwrap();
        // The oldest try was made at the start and counts for three hours.
        assert_eq!(
            refused,
            Reservation::Refused {
                retry_after: 3 * 3600 - 10 * 60
            }
        );
        // A refused try is not counted.
        assert_eq!(
            throttle_times(&mut conn, "password|u:x")
                .await
                .unwrap()
                .len(),
            3
        );
        // Another bucket is another count.
        assert_eq!(
            reserve_try(&mut conn, "password|u:y", at, &bound(3))
                .await
                .unwrap(),
            Reservation::Counted { filled: false }
        );
        // When the oldest is three hours old, one more is let in.
        let later = start + Duration::hours(3) + Duration::seconds(1);
        assert_eq!(
            reserve_try(&mut conn, "password|u:x", later, &bound(3))
                .await
                .unwrap(),
            Reservation::Counted { filled: true }
        );
    }

    #[tokio::test]
    async fn a_try_that_was_right_is_given_back_and_one_that_was_wrong_stays() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let first = start;
        let second = start + Duration::seconds(5);
        reserve_try(&mut conn, "b", first, &bound(8)).await.unwrap();
        reserve_try(&mut conn, "b", second, &bound(8))
            .await
            .unwrap();
        refund_try(&mut conn, "b", second).await.unwrap();
        assert_eq!(throttle_times(&mut conn, "b").await.unwrap(), vec![first]);
        // Two tries at the very same moment are two; giving one back leaves one.
        reserve_try(&mut conn, "same", first, &bound(8))
            .await
            .unwrap();
        reserve_try(&mut conn, "same", first, &bound(8))
            .await
            .unwrap();
        refund_try(&mut conn, "same", first).await.unwrap();
        assert_eq!(
            throttle_times(&mut conn, "same").await.unwrap(),
            vec![first]
        );
    }

    #[tokio::test]
    async fn tries_made_at_the_same_moment_cannot_pass_the_limit_together() {
        let test = TestDb::fresh().await;
        let at = clock::start();
        let mut handles = Vec::new();
        for _ in 0..12 {
            let pool = test.pool.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = pool.acquire().await.unwrap();
                reserve_try(&mut conn, "race", at, &bound(8)).await.unwrap()
            }));
        }
        let mut counted = 0;
        for handle in handles {
            if matches!(handle.await.unwrap(), Reservation::Counted { .. }) {
                counted += 1;
            }
        }
        assert_eq!(counted, 8);
    }

    #[tokio::test]
    async fn tries_that_are_paced_wait_for_the_gap_as_well() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let paced = Bound {
            limit: 5,
            window: Duration::hours(3),
            gap: Some(Duration::seconds(60)),
        };
        assert!(matches!(
            reserve_try(&mut conn, "send|u:x", start, &paced)
                .await
                .unwrap(),
            Reservation::Counted { .. }
        ));
        let soon = start + Duration::seconds(20);
        assert_eq!(
            reserve_try(&mut conn, "send|u:x", soon, &paced)
                .await
                .unwrap(),
            Reservation::Refused { retry_after: 40 }
        );
        let after = start + Duration::seconds(61);
        assert!(matches!(
            reserve_try(&mut conn, "send|u:x", after, &paced)
                .await
                .unwrap(),
            Reservation::Counted { .. }
        ));
    }

    #[tokio::test]
    async fn a_paced_try_is_counted_only_when_it_is_enabled_and_there_is_room() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        let paced = Bound {
            limit: 2,
            window: Duration::hours(3),
            gap: Some(Duration::seconds(60)),
        };
        // A try that does nothing is not counted, and leaves the bucket as it was.
        assert!(
            !pace(&mut conn, "send|u:x", start, &paced, false)
                .await
                .unwrap()
        );
        assert!(
            throttle_times(&mut conn, "send|u:x")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            pace(&mut conn, "send|u:x", start, &paced, true)
                .await
                .unwrap()
        );
        // Not within the minute, and a disabled try after it is still none.
        let soon = start + Duration::seconds(30);
        assert!(
            !pace(&mut conn, "send|u:x", soon, &paced, true)
                .await
                .unwrap()
        );
        let later = start + Duration::seconds(61);
        assert!(
            !pace(&mut conn, "send|u:x", later, &paced, false)
                .await
                .unwrap()
        );
        assert!(
            pace(&mut conn, "send|u:x", later, &paced, true)
                .await
                .unwrap()
        );
        // Two in the window is all it holds.
        let again = start + Duration::seconds(130);
        assert!(
            !pace(&mut conn, "send|u:x", again, &paced, true)
                .await
                .unwrap()
        );
        assert_eq!(
            throttle_times(&mut conn, "send|u:x").await.unwrap().len(),
            2
        );
        // A bucket that holds no try at all lets none in.
        let none = Bound { limit: 0, ..paced };
        assert!(
            !pace(&mut conn, "send|u:y", start, &none, true)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn the_person_is_told_once_a_day_that_a_count_filled() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        reserve_try(&mut conn, "password|u:x", start, &bound(8))
            .await
            .unwrap();
        assert!(
            mark_noticed(&mut conn, "password|u:x", start, Duration::days(1))
                .await
                .unwrap()
        );
        let later = start + Duration::hours(23);
        assert!(
            !mark_noticed(&mut conn, "password|u:x", later, Duration::days(1))
                .await
                .unwrap()
        );
        let next_day = start + Duration::hours(25);
        assert!(
            mark_noticed(&mut conn, "password|u:x", next_day, Duration::days(1))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_person_keeps_twenty_browsers_and_forgets_those_unused_for_long() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let start = clock::start();
        let mut first = None;
        for index in 0..21_i64 {
            let at = start + Duration::minutes(index);
            let mark = Sha256::digest(format!("mark-{index}"));
            let id = remember_device(&mut conn, &mark, ada, at, at - Duration::days(90), 20)
                .await
                .unwrap();
            first.get_or_insert(id);
        }
        let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM known_devices WHERE user_id = $1")
            .bind(ada)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(kept, 20);
        // The one unused the longest went.
        let oldest = Sha256::digest("mark-0");
        let newest = Sha256::digest("mark-20");
        let now = start + Duration::minutes(30);
        let horizon = now - Duration::days(90);
        assert!(
            known_device(&mut conn, Some(&oldest), Some(ada), horizon)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            known_device(&mut conn, Some(&newest), Some(ada), horizon)
                .await
                .unwrap()
                .is_some()
        );
        // Ninety days without getting in from it, and it is a stranger's.
        let long_after = now + Duration::days(91);
        assert!(
            known_device(
                &mut conn,
                Some(&newest),
                Some(ada),
                long_after - Duration::days(90)
            )
            .await
            .unwrap()
            .is_none()
        );
        // Nobody's browser, and a browser without a person, find nothing in the same statement.
        assert!(
            known_device(&mut conn, None, None, horizon)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_code_that_was_mailed_works_once_and_a_newer_one_ends_it() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let now = clock::start();
        let ends = now + Duration::minutes(10);
        put_sign_in_code(&mut conn, ada, b"one", "ada@example.org", now, ends)
            .await
            .unwrap();
        put_sign_in_code(&mut conn, ada, b"two", "ada@example.org", now, ends)
            .await
            .unwrap();
        assert!(
            !spend_sign_in_code(&mut conn, ada, b"one", now)
                .await
                .unwrap()
        );
        // Past its time it is spent by nobody.
        assert!(
            !spend_sign_in_code(&mut conn, ada, b"two", ends + Duration::seconds(1))
                .await
                .unwrap()
        );
        assert!(
            spend_sign_in_code(&mut conn, ada, b"two", now)
                .await
                .unwrap()
        );
        assert!(
            !spend_sign_in_code(&mut conn, ada, b"two", now)
                .await
                .unwrap()
        );
        assert!(
            live_sign_in_code(&mut conn, Some(ada), now)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_link_that_was_mailed_works_once_and_before_its_end() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let now = clock::start();
        let link = |hash: &'static [u8], purpose: &'static str| NewEmailToken {
            id: new_id(),
            user_id: ada,
            purpose,
            token_hash: hash,
            email: "new@example.org",
            clear_second_step: false,
            at: now,
            expires_at: now + Duration::minutes(30),
        };
        create_email_token(&mut conn, &link(b"t1", "reset_password"))
            .await
            .unwrap();
        create_email_token(&mut conn, &link(b"t2", "verify_email"))
            .await
            .unwrap();
        // Another purpose finds nothing.
        assert!(
            use_email_token(&mut conn, b"t1", "verify_email", now)
                .await
                .unwrap()
                .is_none()
        );
        // The address they asked for is pending until its link is used.
        assert_eq!(
            pending_email(&mut conn, ada, now).await.unwrap().as_deref(),
            Some("new@example.org")
        );
        let used = use_email_token(&mut conn, b"t1", "reset_password", now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(used.user_id, ada);
        assert!(
            use_email_token(&mut conn, b"t1", "reset_password", now)
                .await
                .unwrap()
                .is_none()
        );
        // Past its end, a link is not used.
        let late = now + Duration::minutes(31);
        assert!(
            use_email_token(&mut conn, b"t2", "verify_email", late)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(pending_email(&mut conn, ada, late).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_session_slides_with_use_and_ending_it_takes_its_access_tokens() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let now = clock::start();
        let session = NewSession {
            id: new_id(),
            user_id: ada,
            refresh_hash: b"refresh",
            user_agent: "tests",
            ip: Some("203.0.113.7".parse().unwrap()),
            at: now,
            expires_at: now + Duration::days(30),
        };
        create_session(&mut conn, &session).await.unwrap();
        add_access_token(&mut conn, b"old", session.id, now + Duration::minutes(15))
            .await
            .unwrap();
        let found = session_by_refresh_hash(&mut conn, b"refresh", now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.ip.as_deref(), Some("203.0.113.7"));

        let later = now + Duration::days(20);
        slide_session(&mut conn, session.id, later, later + Duration::days(30))
            .await
            .unwrap();
        // The access token had run out; the slide dropped it.
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(tokens, 0);
        // Thirty days after the start it would have ended, but it was used.
        let after = now + Duration::days(40);
        assert!(
            session_by_refresh_hash(&mut conn, b"refresh", after)
                .await
                .unwrap()
                .is_some()
        );

        add_access_token(&mut conn, b"new", session.id, after + Duration::minutes(15))
            .await
            .unwrap();
        assert_eq!(live_sessions(&mut conn, ada, after).await.unwrap().len(), 1);
        assert!(
            revoke_session(&mut conn, session.id, ada, after)
                .await
                .unwrap()
        );
        assert!(
            !revoke_session(&mut conn, session.id, ada, after)
                .await
                .unwrap()
        );
        assert!(
            live_sessions(&mut conn, ada, after)
                .await
                .unwrap()
                .is_empty()
        );
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM access_tokens")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn a_time_step_of_an_authenticator_is_accepted_once() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let now = clock::start();
        // Nothing to accept before it is switched on.
        assert!(!accept_totp_step(&mut conn, ada, 10).await.unwrap());
        assert!(
            put_pending_totp(&mut conn, ada, b"sealed", now)
                .await
                .unwrap()
        );
        assert!(!accept_totp_step(&mut conn, ada, 10).await.unwrap());
        assert!(enable_totp(&mut conn, ada, 10, now).await.unwrap());
        // The step it was switched on with is spent, and so is any before it.
        assert!(!accept_totp_step(&mut conn, ada, 10).await.unwrap());
        assert!(!accept_totp_step(&mut conn, ada, 9).await.unwrap());
        assert!(accept_totp_step(&mut conn, ada, 11).await.unwrap());
        assert!(!accept_totp_step(&mut conn, ada, 11).await.unwrap());
        // No new secret is stored over one that is on.
        assert!(
            !put_pending_totp(&mut conn, ada, b"other", now)
                .await
                .unwrap()
        );
        disable_totp(&mut conn, ada, now).await.unwrap();
        assert!(
            put_pending_totp(&mut conn, ada, b"other", now)
                .await
                .unwrap()
        );
        // Nor over one that waits to be switched on: two requests end with one secret.
        assert!(
            !put_pending_totp(&mut conn, ada, b"another", now)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_row_in_the_inbox_moves_the_feed_once_for_one_key() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = user(&mut conn, "ada").await;
        let data = serde_json::json!({ "way": "password" });
        let row = NewInboxRow {
            user_id: ada,
            kind: "sign_in.attention",
            title: "Somebody tried to sign in as you",
            body: "",
            data: &data,
            dedupe_key: "sign_in.attention:password:2026-10-08",
            at: clock::start(),
        };
        assert_eq!(add_inbox_row(&mut conn, &row).await.unwrap(), Some(1));
        assert_eq!(add_inbox_row(&mut conn, &row).await.unwrap(), None);
        let seq: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id = $1")
            .bind(ada)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(seq, 1);
    }

    fn admin<'a>(id: Id, username: &'a str) -> NewUser<'a> {
        NewUser {
            is_system_admin: true,
            email: None,
            ..person(id, username)
        }
    }

    #[tokio::test]
    async fn a_username_finds_a_person_by_that_name_alone_and_never_by_an_address() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let ada = new_id();
        create_user(&mut conn, &person(ada, "ada"), clock::start())
            .await
            .unwrap();

        let found = user_by_username(&mut conn, "ADA").await.unwrap().unwrap();

        assert_eq!(found.id, ada);
        assert!(
            user_by_username(&mut conn, "ada@example.org")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            user_by_username(&mut conn, "grace")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_first_system_admin_is_the_oldest_one_who_can_sign_in() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let start = clock::start();
        assert_eq!(first_system_admin(&mut conn).await.unwrap(), None);
        let (oldest, next, newest) = (new_id(), new_id(), new_id());
        create_user(&mut conn, &admin(oldest, "oldest"), start)
            .await
            .unwrap();
        create_user(
            &mut conn,
            &admin(next, "next"),
            start + Duration::minutes(1),
        )
        .await
        .unwrap();
        create_user(
            &mut conn,
            &admin(newest, "newest"),
            start + Duration::minutes(2),
        )
        .await
        .unwrap();
        set_suspended(&mut conn, oldest, true, start).await.unwrap();

        assert_eq!(first_system_admin(&mut conn).await.unwrap(), Some(next));
    }

    #[tokio::test]
    async fn two_admins_cannot_each_be_unmade_at_the_same_time() {
        let test = TestDb::fresh().await;
        let (x, y) = (new_id(), new_id());
        {
            let mut conn = test.pool.acquire().await.unwrap();
            create_user(&mut conn, &admin(x, "xavier"), clock::start())
                .await
                .unwrap();
            create_user(&mut conn, &admin(y, "yolanda"), clock::start())
                .await
                .unwrap();
        }
        let unmake = |user: Id| {
            let pool = test.pool.clone();
            async move {
                let mut tx = pool.begin().await.unwrap();
                let standing = set_system_admin(&mut tx, user, false, clock::start())
                    .await
                    .unwrap();
                // The other request arrives while this one has not yet ended.
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                tx.commit().await.unwrap();
                standing
            }
        };

        let (first, second) = tokio::join!(unmake(x), unmake(y));

        let mut standings = [first, second];
        standings.sort_by_key(|standing| format!("{standing:?}"));
        assert_eq!(standings, [Standing::Changed, Standing::LastAdmin]);
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE is_system_admin")
            .fetch_one(&test.pool)
            .await
            .unwrap();
        assert_eq!(left, 1, "one admin is always left");
    }

    #[test]
    fn the_wait_for_a_count_is_whole_seconds_and_at_least_one() {
        let now = clock::start();
        let bound = bound(2);
        let times = [now - Duration::hours(3) + Duration::milliseconds(500), now];
        assert_eq!(retry_after(&times, now, &bound), 1);
        assert_eq!(retry_after(&[], now, &bound), 1);
        let times = [now - Duration::minutes(5), now - Duration::minutes(1)];
        assert_eq!(retry_after(&times, now, &bound), 3 * 3600 - 5 * 60);
    }
}
