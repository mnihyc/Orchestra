//! Signing in: the proofs a sign-in takes and what it still lacks; wrong tries counted by the name that was tried, each proof apart and a browser its person signed in from by itself; a code sent by mail; and the answers that tell nothing about a name.
//!
//! A sign-in is made of proofs: a password, a code mailed to the person's
//! confirmed address, and a code of their authenticator app. It takes one
//! proof, or two different ones for a person who has a second step; what is
//! missing is asked for by name. Each request carries all the proofs it has,
//! so nothing is kept between the two requests of a sign-in; a mailed code is
//! spent, and a step of the authenticator accepted, only when the sign-in is
//! complete.
//!
//! What is common to the requests that get a person in lives here: the limits
//! from the configuration, the system's policies that decide who may sign in
//! and how, the cookies, and the opening of a session. The parts are:
//!
//! - [`throttle`] counts wrong tries by name, per proof, per known browser and
//!   per address.
//! - [`proofs`] checks a password, a mailed code and an authenticator's code,
//!   and says what a person's set of proofs lacks.
//! - [`login`] is the sign-in itself and the request for a code by mail.
//! - [`accounts`] makes a person with the org that is theirs.
//! - [`reset_links`] makes a link that sets a new password by hand, for a person
//!   whom no mail reaches.
//! - [`fresh`] asks a sensitive act for a fresh proof of who the caller is.
//! - [`second_step`] is the second step an org can require, and where a
//!   request waits for it.
//! - [`passwords`], [`codes`], [`mail`] and [`people`] are what these stand on.

pub mod accounts;
pub mod codes;
pub mod fresh;
pub mod login;
pub mod mail;
pub mod passwords;
pub mod people;
pub mod proofs;
pub mod recovery_codes;
pub mod reset_links;
pub mod second_step;
pub mod throttle;

use crate::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, SESSION_PREFIX, new_token, token_hash,
};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx};
use chrono::Duration;
use http::header::{CACHE_CONTROL, SET_COOKIE};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::auth::{AuthResult, OrgCreation, RegistrationMode};
use orch_api_contract::common::{DEVICE_COOKIE, ErrorCode, SESSION_COOKIE};
use orch_config::server::Auth;
use orch_db::Conn;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::identity::{self, Bound, NewSession, UserRow};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Json, Time};
use serde::de::DeserializeOwned;

/// What a cache is told of an answer that holds a secret, which is shown once
/// and must not be kept.
pub const NOT_KEPT: &str = "no-store";

/// Refuses simple browser forms at public requests that can create a session.
pub(crate) fn require_json(ctx: &Ctx) -> ApiResult<()> {
    let headers = ctx.headers.get_all(http::header::CONTENT_TYPE);
    let mut values = headers.iter();
    let json = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
    if !json || values.next().is_some() {
        return Err(ApiError::invalid("Content-Type", "send application/json"));
    }
    Ok(())
}

/// How soon after one code was mailed to a name another may be: a minute, so
/// that nobody is sent mail without end by somebody else.
pub const SEND_GAP: Duration = Duration::seconds(60);

/// How many registrations, recoveries or confirmations one address may ask
/// for in [`SIGNUP_WINDOW`]: ten, as many as an office behind one address
/// needs on a day when it takes on a team.
pub const SIGNUP_LIMIT: u32 = 10;

/// The hour in which [`SIGNUP_LIMIT`] counts.
pub const SIGNUP_WINDOW: Duration = Duration::hours(1);

/// How many browsers a person is known by: the one unused the longest goes
/// for the next.
pub const KNOWN_BROWSERS_PER_PERSON: i64 = 20;

/// The longest user agent a session keeps, in characters, which is what the
/// schema allows.
pub(crate) const USER_AGENT_MAX_CHARS: usize = 400;

/// The numbers that bound the counts of tries, from the configuration.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Wrong tries of one proof at one name in the window.
    pub login: Bound,
    /// Wrong tries from one address, whatever the names.
    pub address: Bound,
    /// Codes mailed to one name, and how far apart.
    pub send: Bound,
    /// Registrations, recoveries and confirmations asked for from one address.
    pub signup: Bound,
}

impl Limits {
    /// The limits that `[auth]` sets.
    #[must_use]
    pub fn of(auth: &Auth) -> Self {
        let seconds = |secs: u64| Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX));
        Self {
            login: Bound {
                limit: auth.login_failure_limit,
                window: seconds(auth.login_failure_window_secs),
                gap: None,
            },
            address: Bound {
                limit: auth.address_failure_limit,
                window: seconds(auth.address_failure_window_secs),
                gap: None,
            },
            // Codes are counted in the same three hours as wrong tries.
            send: Bound {
                limit: auth.email_code_send_limit,
                window: seconds(auth.login_failure_window_secs),
                gap: Some(SEND_GAP),
            },
            signup: Bound {
                limit: SIGNUP_LIMIT,
                window: SIGNUP_WINDOW,
                gap: None,
            },
        }
    }
}

/// What the system's policies say about getting in.
#[derive(Debug, Clone, Copy)]
pub struct SystemPolicy {
    /// Who may register.
    pub registration: RegistrationMode,
    /// Who may make a team org.
    pub org_creation: OrgCreation,
    /// Whether anybody may sign in by a password or a mailed code: when it is
    /// off, system admins alone may.
    pub local_login: bool,
    /// Whether an address has to be confirmed before the first sign-in.
    pub verification_required: bool,
    /// The system's mail server, where it has one.
    pub mail_channel: Option<Id>,
}

impl SystemPolicy {
    /// Whether the system has a mail server.
    #[must_use]
    pub fn mail(&self) -> bool {
        self.mail_channel.is_some()
    }
}

/// Reads the value of a setting of the system as the type it has.
pub fn setting<T: DeserializeOwned>(snapshot: &Snapshot, key: &str) -> ApiResult<Option<T>> {
    let def = registry::get(key)
        .ok_or_else(|| ApiError::internal(format!("the setting {key} is not in the registry")))?;
    match snapshot.resolve(def).value {
        None => Ok(None),
        Some(value) => serde_json::from_value(value).map(Some).map_err(|error| {
            ApiError::internal(format!("the setting {key} is unreadable: {error}"))
        }),
    }
}

/// The settings of the system that decide who gets in, and whether it has a
/// mail server. Where the system has none, an address cannot be confirmed
/// whatever the setting says.
pub async fn system_policy(conn: &mut Conn) -> ApiResult<SystemPolicy> {
    let defs = [
        keys::REGISTRATION_MODE,
        keys::ORGS_CREATED_BY,
        keys::AUTH_LOCAL_LOGIN,
        keys::REGISTRATION_EMAIL_VERIFICATION,
    ]
    .map(registry::get);
    let defs: Vec<_> = defs.into_iter().flatten().collect();
    // The general resolver includes every kind of place and supplied default.
    // Sign-in must not embed all of those futures in its own debug stack frame.
    let snapshot = Box::pin(Snapshot::load(conn, Place::System, Wanted::Keys(&defs)))
        .await
        .map_err(ApiError::internal)?;
    let mail_channel = identity::system_mail_channel(conn).await?;
    let verification: Option<String> = setting(&snapshot, keys::REGISTRATION_EMAIL_VERIFICATION)?;
    Ok(SystemPolicy {
        registration: setting(&snapshot, keys::REGISTRATION_MODE)?
            .unwrap_or(RegistrationMode::Invite),
        org_creation: setting(&snapshot, keys::ORGS_CREATED_BY)?.unwrap_or(OrgCreation::Invited),
        local_login: setting(&snapshot, keys::AUTH_LOCAL_LOGIN)?.unwrap_or(true),
        verification_required: mail_channel.is_some()
            && verification.as_deref() == Some("required"),
        mail_channel,
    })
}

/// Where a request came from, for the rows it causes.
#[must_use]
pub fn origin(ctx: &Ctx) -> Origin {
    Origin {
        as_admin: ctx
            .user()
            .ok()
            .filter(|user| user.as_admin && user.is_session())
            .map(|user| user.id()),
        ip: ctx.remote,
        user_agent: user_agent(ctx),
        request_id: Some(ctx.request_id.clone()),
    }
}

/// The browser that made a request, cut to what a session keeps.
#[must_use]
pub fn user_agent(ctx: &Ctx) -> Option<String> {
    ctx.headers
        .get(http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|agent| agent.chars().take(USER_AGENT_MAX_CHARS).collect())
}

/// Writes the audit row of something that was done, in the caller's
/// transaction. `object` is the user it is about, where it is about one.
pub async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    actor: Actor,
    action: AuditAction,
    object: Option<Id>,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        actor,
        action.as_str(),
        action.object_kind().unwrap_or("auth"),
        object,
        summary,
    );
    entry.history = action.is_history_line();
    entry.details = details;
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// A host-bound sign-in cookie, out of reach of scripts and sent only over TLS.
#[must_use]
pub fn cookie(name: &str, value: &str, life: Duration) -> String {
    format!(
        "{name}={value}; Max-Age={}; Path=/; HttpOnly; Secure; SameSite=Strict",
        life.num_seconds().max(0)
    )
}

/// The cookie that holds a session, good for as long as the session is.
#[must_use]
pub fn session_cookie(value: &str, life: Duration) -> String {
    cookie(SESSION_COOKIE, value, life)
}

/// How long a browser stays known after its person last got in from it.
#[must_use]
pub fn known_for(auth: &Auth) -> Duration {
    days(auth.known_device_days)
}

/// A number of days as a span, kept inside what a span can hold.
fn days(count: u64) -> Duration {
    Duration::days(i64::try_from(count).unwrap_or(i64::MAX / SECONDS_PER_DAY))
}

/// Seconds in a day, to keep a span of days from overflowing.
const SECONDS_PER_DAY: i64 = 86_400;

/// The cookie that marks a browser its person got in from, good for the
/// days that a browser stays known.
#[must_use]
pub fn device_cookie(value: &str, auth: &Auth) -> String {
    cookie(DEVICE_COOKIE, value, known_for(auth))
}

/// The cookie of a session, taken back.
#[must_use]
pub fn cleared_session_cookie() -> String {
    cookie(SESSION_COOKIE, "", Duration::zero())
}

/// How long a session lasts after its last use.
#[must_use]
pub fn session_life(auth: &Auth) -> Duration {
    days(auth.session_ttl_days)
}

/// How long an access token lasts.
#[must_use]
pub fn access_life(auth: &Auth) -> Duration {
    Duration::seconds(i64::try_from(auth.access_token_ttl_secs).unwrap_or(i64::MAX))
}

/// A person who got in: what to answer, and the cookies to set.
#[derive(Debug)]
pub struct Opened {
    /// The access token and who signed in.
    pub result: AuthResult,
    /// The `Set-Cookie` values.
    pub cookies: Vec<String>,
    /// The session that was opened.
    pub session: Id,
}

impl Opened {
    /// The answer, with its cookies.
    #[must_use]
    pub fn answer(self) -> Answer<AuthResult> {
        self.cookies.iter().fold(
            Answer::from(self.result).header(CACHE_CONTROL, NOT_KEPT),
            |answer, value| answer.header(SET_COOKIE, value),
        )
    }
}

/// The cookies that make the browser of this request one its person got in
/// from: the mark it carries, or a new one, noted for the person. The mark is
/// returned with the cookie that sets it.
pub async fn remember_browser(ctx: &Ctx, conn: &mut Conn, user: Id) -> ApiResult<String> {
    let auth = &ctx.app.config.auth;
    let mark = throttle::mark_of(ctx)
        .map_or_else(|| new_token(throttle::DEVICE_MARK_PREFIX), str::to_owned);
    identity::remember_device(
        conn,
        &throttle::mark_hash(&mark),
        user,
        ctx.now,
        ctx.now - known_for(auth),
        KNOWN_BROWSERS_PER_PERSON,
    )
    .await?;
    Ok(device_cookie(&mark, auth))
}

/// Opens a session for `user` on the browser of this request, in the caller's
/// transaction: the session and its first access token, the browser noted as
/// one the person got in from, and the sign-in audited. `how` says what got
/// them in, and is kept with the audit row.
pub async fn open_session(
    ctx: &Ctx,
    conn: &mut Conn,
    user: &UserRow,
    mut how: Json,
) -> ApiResult<Opened> {
    let user = orch_db::queries::transaction_guard::lock_person(conn, user.id)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidCredentials,
                "this account no longer exists",
            )
        })?;
    if user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    let mut current_ctx = ctx.clone();
    current_ctx.now = ctx.app.clock.now();
    let ctx = &current_ctx;
    let auth = &ctx.app.config.auth;
    let (session, cookie_value, access) = (
        orch_types::new_id(),
        new_token(SESSION_PREFIX),
        new_token(ACCESS_TOKEN_PREFIX),
    );
    let access_expires_at = ctx.now + access_life(auth);
    let life = session_life(auth);
    identity::create_session(
        conn,
        &NewSession {
            id: session,
            user_id: user.id,
            refresh_hash: &token_hash(&cookie_value),
            user_agent: &user_agent(ctx).unwrap_or_default(),
            ip: ctx.remote,
            at: ctx.now,
            expires_at: ctx.now + life,
        },
    )
    .await?;
    identity::add_access_token(conn, &token_hash(&access), session, access_expires_at).await?;
    let browser = remember_browser(ctx, conn, user.id).await?;
    identity::touch_last_seen(conn, user.id, ctx.now).await?;
    if let Some(details) = how.as_object_mut() {
        details.insert("session".to_owned(), serde_json::json!(session));
    }
    record(
        ctx,
        conn,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthLogin),
        Some(user.id),
        &format!("{} signed in", user.username),
        how,
    )
    .await?;
    let me = Box::pin(people::me(conn, &user, ctx.now)).await?;
    Ok(Opened {
        result: AuthResult {
            access_token: access,
            access_expires_at: Time::new(access_expires_at),
            user: me,
        },
        cookies: vec![session_cookie(&cookie_value, life), browser],
        session,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limits_as_shipped_are_eight_a_name_in_three_hours_thirty_an_address_in_a_quarter_hour_and_five_codes_in_three_hours()
     {
        let limits = Limits::of(&Auth::default());
        assert_eq!(
            (limits.login.limit, limits.login.window),
            (8, Duration::hours(3))
        );
        assert_eq!(
            (limits.address.limit, limits.address.window),
            (30, Duration::minutes(15))
        );
        assert_eq!(
            (limits.send.limit, limits.send.window, limits.send.gap),
            (5, Duration::hours(3), Some(Duration::minutes(1)))
        );
        assert_eq!(
            (limits.signup.limit, limits.signup.window),
            (10, Duration::hours(1))
        );
    }

    #[test]
    fn a_cookie_carries_the_attributes_every_cookie_of_the_sign_in_has() {
        assert_eq!(
            session_cookie("ors_abc", Duration::days(30)),
            "__Host-orch_session=ors_abc; Max-Age=2592000; Path=/; HttpOnly; Secure; SameSite=Strict"
        );
        assert_eq!(
            device_cookie("odv_abc", &Auth::default()),
            "__Host-orch_device=odv_abc; Max-Age=7776000; Path=/; HttpOnly; Secure; SameSite=Strict"
        );
        assert_eq!(
            cleared_session_cookie(),
            "__Host-orch_session=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Strict"
        );
    }
}
