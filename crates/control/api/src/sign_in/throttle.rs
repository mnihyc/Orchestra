//! Counting wrong tries: by the name that was tried, each proof apart, a browser its person got in from by itself, and one address across all names.
//!
//! A count belongs to the name and not to the address a try came from, so
//! that a thousand addresses give a guesser nothing: whoever tries a name,
//! from wherever, shares one count of each proof with everybody else who
//! tries it. A name that nobody has is counted exactly like one somebody
//! has, in the same statements. A browser its person got in from before
//! carries a mark, and its tries are counted for that browser alone, so that
//! whoever locks a name by guessing at it locks it for strangers and not for
//! its person. Nothing clears a count but time.
//!
//! A try is counted before the proof is looked at, and given back if the
//! proof was right: otherwise a thousand tries made at once would all pass
//! the limit before the first of them was counted.
//!
//! Identity-provider starts use the same configured bounds. Their address
//! is counted before any external work, and their account as soon as it is
//! known; unlike a wrong-proof count, a start remains a counted request.

use super::{Limits, known_for, record};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::auth::SignInProof;
use orch_api_contract::common::{DEVICE_COOKIE, ErrorCode};
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::queries::identity::{
    self, Reservation, UserRow, mark_noticed, refund_try, reserve_try,
};
use orch_types::Id;
use serde_json::json;
use sha2::{Digest, Sha256};

/// What the mark of a browser begins with.
pub const DEVICE_MARK_PREFIX: &str = "odv_";

/// How often a person is told that strangers' tries filled a count at their
/// name: once a day, however many times it fills.
const NOTICE_INTERVAL: Duration = Duration::days(1);

/// Who a count is kept for.
#[derive(Debug, Clone)]
pub struct Who {
    /// `u:<user id>` for somebody's name, `n:<hash>` for a name nobody has,
    /// `s:<session id>` or `t:<token id>` for a signed-in caller's own tries.
    key: String,
    /// The browser that the person got in from before, where this try comes
    /// from one.
    device: Option<Id>,
    /// The person the name is, where somebody has it.
    user: Option<Id>,
    /// Whether the person is to be told when strangers fill the count.
    tells_the_person: bool,
}

impl Who {
    /// The count of the tries of a signed-in caller, for that session or that
    /// token alone.
    #[must_use]
    pub fn caller(kind: char, id: Id, user: Id) -> Self {
        Self {
            key: format!("{kind}:{id}"),
            device: None,
            user: Some(user),
            tells_the_person: false,
        }
    }

    /// The count of a name, which is somebody's where `user` says so, and
    /// kept under the hash of the name otherwise. No browser is considered.
    #[must_use]
    pub fn named(user: Option<Id>, normalized: &str) -> Self {
        Self {
            key: match user {
                Some(user) => format!("u:{user}"),
                None => format!("n:{}", name_hash(normalized)),
            },
            device: None,
            user,
            tells_the_person: false,
        }
    }

    /// The name of the bucket for the count of `what`.
    #[must_use]
    pub fn bucket(&self, what: &str) -> String {
        match self.device {
            Some(device) => format!("{what}|{}|{device}", self.key),
            None => format!("{what}|{}", self.key),
        }
    }

    /// The person the name is, where somebody has it.
    #[must_use]
    pub fn user(&self) -> Option<Id> {
        self.user
    }

    /// Whose tries the count holds, in a word: a session's or a token's own,
    /// those of a known browser, or those of everybody else.
    fn whose(&self) -> &'static str {
        if self.key.starts_with("s:") || self.key.starts_with("t:") {
            "session"
        } else if self.device.is_some() {
            "known_browser"
        } else {
            "strangers"
        }
    }

    /// Whether the try comes from a browser its person got in from.
    #[must_use]
    pub fn from_known_browser(&self) -> bool {
        self.device.is_some()
    }
}

/// The name that was given, brought to the form it is looked up and counted
/// in: without the spaces around it, in lower case.
#[must_use]
pub fn normalize(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The SHA-256 of a name in hexadecimal: what a name nobody has is counted
/// under, so that the count tells nothing and keeps nothing readable.
#[must_use]
pub fn name_hash(normalized: &str) -> String {
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The SHA-256 of the mark of a browser, which is what is stored of it.
#[must_use]
pub fn mark_hash(mark: &str) -> Vec<u8> {
    Sha256::digest(mark.as_bytes()).to_vec()
}

/// The mark a request carries, where it is in the form of one.
pub fn mark_of(ctx: &Ctx) -> Option<&str> {
    ctx.cookie(DEVICE_COOKIE)
        .filter(|mark| mark.starts_with(DEVICE_MARK_PREFIX))
}

/// A name that was given, and what is known about it.
#[derive(Debug, Clone)]
pub struct Located {
    /// The person, where somebody has the name.
    pub user: Option<UserRow>,
    /// Whose count it is.
    pub who: Who,
}

/// Finds who a name is and whether the request comes from a browser they got
/// in from. A name nobody has is found as well as any other, in the same two
/// statements, and is counted under its hash.
pub async fn locate(ctx: &Ctx, conn: &mut Conn, given: &str) -> ApiResult<Located> {
    let name = normalize(given);
    let user = identity::user_by_login(conn, &name).await?;
    let mark = mark_of(ctx).map(mark_hash);
    let horizon = ctx.now - known_for(&ctx.app.config.auth);
    let device = identity::known_device(
        conn,
        mark.as_deref(),
        user.as_ref().map(|user| user.id),
        horizon,
    )
    .await?;
    let who = Who {
        key: match &user {
            Some(user) => format!("u:{}", user.id),
            None => format!("n:{}", name_hash(&name)),
        },
        device: device.map(|device| device.id),
        user: user.as_ref().map(|user| user.id),
        tells_the_person: true,
    };
    Ok(Located { user, who })
}

/// A try that was counted ahead and is given back if the proof was right.
#[derive(Debug)]
pub struct Tried {
    address: Option<(String, DateTime<Utc>)>,
    name: (String, DateTime<Utc>),
    filled: bool,
    proof: SignInProof,
}

fn word(proof: SignInProof) -> &'static str {
    match proof {
        SignInProof::Password => "password",
        SignInProof::EmailCode => "email_code",
        // A backup is the same second factor: switching methods must not buy more guesses.
        SignInProof::Totp | SignInProof::RecoveryCode => "totp",
    }
}

/// `423 auth.locked`: too many wrong tries of that proof at that name.
fn locked(proof: SignInProof, retry_after: u64) -> ApiError {
    ApiError::new(
        ErrorCode::AuthLocked,
        format!(
            "too many wrong tries of the {} at this name",
            word(proof).replace('_', " ")
        ),
    )
    .with("way", proof)
    .retry_after(retry_after)
}

/// `429 rate_limited`, with when to try again.
pub fn rate_limited(retry_after: u64) -> ApiError {
    ApiError::new(
        ErrorCode::RateLimited,
        "too many requests from this address",
    )
    .retry_after(retry_after)
}

/// Counts a try of `proof` at the name `who`, ahead of looking at it. The
/// address it came from is counted too, when `by_address` says so: a guard
/// against one machine trying name after name. A proof whose count is full is
/// refused unseen, as `423 auth.locked` with the proof and when it may be
/// tried again; an address whose count is full, as `429`.
pub async fn begin(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    proof: SignInProof,
    by_address: bool,
) -> ApiResult<Tried> {
    let now = ctx.now;
    let address = match ctx.remote.filter(|_| by_address) {
        Some(ip) => {
            let bucket = format!("address|{ip}");
            match reserve_try(conn, &bucket, now, &limits.address).await? {
                Reservation::Counted { .. } => Some((bucket, now)),
                Reservation::Refused { retry_after } => return Err(rate_limited(retry_after)),
            }
        }
        None => None,
    };
    let bucket = who.bucket(word(proof));
    match reserve_try(conn, &bucket, now, &limits.login).await? {
        Reservation::Counted { filled } => Ok(Tried {
            address,
            name: (bucket, now),
            filled,
            proof,
        }),
        Reservation::Refused { retry_after } => {
            // Refused unseen is not a try: the address is not charged for it.
            if let Some((bucket, at)) = &address {
                refund_try(conn, bucket, *at).await?;
            }
            Err(locked(proof, retry_after))
        }
    }
}

/// Settles a try. A proof that was right gives its counts back; one that was
/// wrong keeps them, and if it was the try that filled the count of the name,
/// the lock is recorded, and the person told if the count was strangers'.
pub async fn end(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    tried: Tried,
    right: bool,
) -> ApiResult<()> {
    if right {
        refund_try(conn, &tried.name.0, tried.name.1).await?;
        if let Some((bucket, at)) = &tried.address {
            refund_try(conn, bucket, *at).await?;
        }
        return Ok(());
    }
    if tried.filled {
        filled(ctx, conn, limits, who, &tried).await?;
    }
    Ok(())
}

/// The count of a proof at a name just became as large as its limit: audit
/// it, and tell the person if the count is the one strangers share.
async fn filled(
    ctx: &Ctx,
    conn: &mut Conn,
    limits: &Limits,
    who: &Who,
    tried: &Tried,
) -> ApiResult<()> {
    let way = word(tried.proof);
    let strangers = who.device.is_none();
    record(
        ctx,
        conn,
        Actor::System,
        orch_db::audit::action!(AuditAction::AuthLocked),
        who.user,
        &format!(
            "too many wrong tries of the {} at a name",
            way.replace('_', " ")
        ),
        json!({
            "way": way,
            "for": who.whose(),
            "name": if who.user.is_none() { Some(who.key.clone()) } else { None },
        }),
    )
    .await?;
    // Whether to tell is decided for every name, by the same statement; only
    // somebody who has the name has anybody to tell.
    let first_today = mark_noticed(conn, &tried.name.0, ctx.now, NOTICE_INTERVAL).await?;
    if let (true, true, Some(user)) = (first_today, strangers && who.tells_the_person, who.user) {
        let times = identity::throttle_times(conn, &tried.name.0).await?;
        let until = ctx.now
            + Duration::seconds(
                i64::try_from(identity::retry_after(&times, ctx.now, &limits.login)).unwrap_or(0),
            );
        let data = json!({ "way": way, "until": orch_types::Time::new(until) });
        let day = ctx.now.format("%Y-%m-%d");
        // This branch is stored in every proof check, even for a correct password.
        // Keep its settings and notification futures out of the sign-in state machine.
        Box::pin(orch_engine::notifications::tell(
            conn,
            &orch_engine::notifications::Event {
                kind: orch_engine::notifications::Kind::SignInAttention,
                node:None, thread:None, approval:None, actor:None,
                title: "Somebody tried to sign in as you".to_owned(),
                body: format!(
                    "Too many wrong tries of the {} were made at your name. It is locked for browsers you have not signed in from until {}.",
                    way.replace('_', " "),
                    until.format("%H:%M UTC"),
                ),
                data,
                source: format!("locked:{way}:{day}"),
                at: ctx.now,
            },
            &[user],
            None,
        ))
        .await.map_err(ApiError::internal)?;
    }
    Ok(())
}

/// Counts a request that makes mail or a person, by the address it came from:
/// at most [`Limits::signup`] of each kind in the window. A request that
/// cannot be placed has no address to count.
pub async fn by_address(ctx: &Ctx, conn: &mut Conn, limits: &Limits, what: &str) -> ApiResult<()> {
    let Some(ip) = ctx.remote else {
        return Ok(());
    };
    match reserve_try(conn, &format!("{what}|{ip}"), ctx.now, &limits.signup).await? {
        Reservation::Counted { .. } => Ok(()),
        Reservation::Refused { retry_after } => Err(rate_limited(retry_after)),
    }
}

/// Bounds the work of starting an identity-provider sign-in before its state
/// is made or the provider is asked. It uses the sign-in address limit and
/// the same address bucket as local proofs. Starts are requests, so their
/// count remains even when the provider later proves the person correctly.
pub async fn identity_start(ctx: &Ctx, conn: &mut Conn) -> ApiResult<()> {
    let Some(ip) = ctx.remote else {
        return Ok(());
    };
    let limits = Limits::of(&ctx.app.config.auth);
    match reserve_try(conn, &format!("address|{ip}"), ctx.now, &limits.address).await? {
        Reservation::Counted { .. } => Ok(()),
        Reservation::Refused { retry_after } => Err(rate_limited(retry_after)),
    }
}

/// Bounds starts for one account across addresses, sessions and providers,
/// with the configured sign-in name limit. A signed-in link or fresh proof
/// knows its account at the start; an anonymous sign-in knows it only after
/// the provider's answer is verified. An identity not linked yet is counted
/// by its provider and subject, hashed by [`Who::named`], never by an email
/// address or another unverified hint. Call on an independent connection so
/// that a refusal or a failed registration cannot roll its count back.
pub async fn identity_account(ctx: &Ctx, conn: &mut Conn, who: &Who) -> ApiResult<()> {
    let limits = Limits::of(&ctx.app.config.auth);
    match reserve_try(
        conn,
        &who.bucket("identity_provider"),
        ctx.now,
        &limits.login,
    )
    .await?
    {
        Reservation::Counted { .. } => Ok(()),
        Reservation::Refused { retry_after } => Err(ApiError::new(
            ErrorCode::RateLimited,
            "too many identity-provider sign-in starts for this account",
        )
        .retry_after(retry_after)),
    }
}
