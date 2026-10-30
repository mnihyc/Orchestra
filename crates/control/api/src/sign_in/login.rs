//! The sign-in, and the request for a code by mail; the second answers the same to every name.
//!
//! A name nobody has goes through the same steps as one somebody has, in the
//! same statements and with the same hashing, and is counted and locked in
//! the same way; the answers that depend on the person (that they are
//! suspended, that their address is not confirmed, that they have a second
//! step) come only after a first step that was right, so that they tell
//! nothing to a stranger.

use super::mail::{self, Addressee};
use super::proofs::{self, Capabilities};
use super::throttle::{self, Located, Who};
use super::{Limits, Opened, SystemPolicy, open_session, people, record, system_policy};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::auth::{AuthEmailCodeSendBody, AuthLoginBody, SignInProof};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::users::EmailCodeUse;
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::queries::identity::{self, UserRow};
use orch_types::Id;
use serde_json::json;

/// `401 auth.invalid_credentials`: the one answer for a wrong name, password
/// or mailed code.
pub fn invalid_credentials() -> ApiError {
    ApiError::new(
        ErrorCode::AuthInvalidCredentials,
        "the name, the password or the code is wrong",
    )
}

/// `401 auth.second_step_invalid`: the second step was wrong, or a code that
/// was used already.
pub fn second_step_invalid() -> ApiError {
    ApiError::new(
        ErrorCode::AuthSecondStepInvalid,
        "the second step is wrong, or that code was used already",
    )
}

/// `401 auth.second_step_required`, with the proofs of which one more
/// completes the sign-in.
pub fn second_step_required(methods: &[SignInProof]) -> ApiError {
    ApiError::new(
        ErrorCode::AuthSecondStepRequired,
        "a second step is needed to sign in",
    )
    .with("methods", methods)
}

/// A proof that was given: one that is not empty. A form that sends the
/// fields it did not fill as empty ones has tried nothing with them.
fn given(proof: &Option<String>) -> Option<&str> {
    proof.as_deref().filter(|text| !text.is_empty())
}

/// What no name can make valid: a request with no proof, with an
/// authenticator's code and nothing to begin with, or with all three.
fn shape(
    password: Option<&str>,
    code: Option<&str>,
    totp: Option<&str>,
    recovery: Option<&str>,
) -> ApiResult<()> {
    if password.is_none() && code.is_none() {
        let field = if totp.is_some() {
            "totp"
        } else if recovery.is_some() {
            "recovery_code"
        } else {
            ""
        };
        return Err(ApiError::invalid(
            field,
            "give a password or a mailed code before a second step",
        ));
    }
    if [password, code, totp, recovery]
        .iter()
        .filter(|proof| proof.is_some())
        .count()
        > 2
    {
        return Err(ApiError::invalid("", "a sign-in takes one proof, or two"));
    }
    Ok(())
}

/// Audits a sign-in that failed, in the same statement whether the name is
/// somebody's or nobody's.
async fn failed(ctx: &Ctx, conn: &mut Conn, who: &Who, way: SignInProof) -> ApiResult<()> {
    record(
        ctx,
        conn,
        Actor::System,
        orch_db::audit::action!(AuditAction::AuthLoginFailed),
        who.user(),
        "a sign-in failed",
        json!({
            "way": way,
            "known_browser": who.from_known_browser(),
        }),
    )
    .await
}

/// What the person's own state refuses once they have proved who they are
/// with a first step: they are suspended, signing in here is for system
/// admins only, or their address is not confirmed where it has to be.
fn refuse_the_person(user: &UserRow, policy: &SystemPolicy) -> ApiResult<()> {
    if user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    if !policy.local_login && !user.is_system_admin {
        return Err(ApiError::new(
            ErrorCode::AuthLocalLoginDisabled,
            "signing in with a password or a code is switched off here",
        ));
    }
    // A system admin is let in without it: they are the way back in when mail
    // is wrong, and the first of them was made before there was a mail server.
    if policy.verification_required
        && user.email.is_some()
        && !user.email_confirmed()
        && !user.is_system_admin
    {
        return Err(ApiError::new(
            ErrorCode::AuthEmailUnverified,
            "the address is not confirmed yet",
        ));
    }
    Ok(())
}

/// Signs a person in with the proofs the request carries.
pub async fn login(ctx: &Ctx, body: &AuthLoginBody) -> ApiResult<Opened> {
    let (password, code, totp, recovery) = (
        given(&body.password),
        given(&body.email_code),
        given(&body.totp),
        given(&body.recovery_code),
    );
    shape(password, code, totp, recovery)?;
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let Located { user, who } = throttle::locate(ctx, &mut conn, &body.login).await?;
    let found = user.as_ref();

    let mut right: Vec<SignInProof> = Vec::new();
    let mut code_to_spend: Option<Vec<u8>> = None;
    let mut step_to_accept: Option<i64> = None;
    let mut recovery_to_spend: Option<Vec<u8>> = None;

    // The first step: the password, or else a mailed code.
    if let Some(password) = password {
        if !proofs::password_right(ctx, &mut conn, &limits, &who, found, password, true).await? {
            failed(ctx, &mut conn, &who, SignInProof::Password).await?;
            return Err(invalid_credentials());
        }
        right.push(SignInProof::Password);
    } else if let Some(typed) = code {
        match proofs::mailed_code_right(ctx, &mut conn, &limits, &who, found, typed, true).await? {
            Some(hash) => {
                code_to_spend = Some(hash);
                right.push(SignInProof::EmailCode);
            }
            None => {
                failed(ctx, &mut conn, &who, SignInProof::EmailCode).await?;
                return Err(invalid_credentials());
            }
        }
    }
    // A proof that is right is somebody's.
    let Some(user) = found else {
        return Err(invalid_credentials());
    };

    let policy = system_policy(&mut conn).await?;
    refuse_the_person(user, &policy)?;
    let capabilities = proofs::capabilities(&mut conn, user).await?;

    // The second step is looked at only after a first that was right, and only
    // where the person has one that the proof can be.
    if capabilities.has_second() {
        if let (Some(_), Some(typed)) = (password, code)
            && capabilities.second(SignInProof::EmailCode)
        {
            match proofs::mailed_code_right(ctx, &mut conn, &limits, &who, found, typed, true)
                .await?
            {
                Some(hash) => {
                    code_to_spend = Some(hash);
                    right.push(SignInProof::EmailCode);
                }
                None => {
                    failed(ctx, &mut conn, &who, SignInProof::EmailCode).await?;
                    return Err(second_step_invalid());
                }
            }
        }
        if let Some(typed) = totp
            && capabilities.second(SignInProof::Totp)
        {
            match proofs::authenticator_right(ctx, &mut conn, &limits, &who, user, typed, true)
                .await?
            {
                Some(step) => {
                    step_to_accept = Some(step);
                    right.push(SignInProof::Totp);
                }
                None => {
                    failed(ctx, &mut conn, &who, SignInProof::Totp).await?;
                    return Err(second_step_invalid());
                }
            }
        }
    }
    if let Some(typed) = recovery {
        let Some(hash) =
            proofs::recovery_right(ctx, &mut conn, &limits, &who, user, typed, true).await?
        else {
            failed(ctx, &mut conn, &who, SignInProof::RecoveryCode).await?;
            return Err(second_step_invalid());
        };
        recovery_to_spend = Some(hash);
        right.push(SignInProof::RecoveryCode);
    }
    complete(&capabilities, &right)?;

    // In: what was used is spent, and the session opened, together.
    let mut tx = ctx.app.db.begin().await?;
    let current = orch_db::queries::transaction_guard::lock_person(&mut tx, user.id)
        .await?
        .ok_or_else(invalid_credentials)?;
    // Proofs are expensive and are checked before this short transaction. A
    // password, address or authenticator replaced meanwhile cannot open it.
    if (password.is_some() && current.password_hash != user.password_hash)
        || (code_to_spend.is_some()
            && (current.email != user.email || current.email_verified_at != user.email_verified_at))
        || ((step_to_accept.is_some() || recovery_to_spend.is_some())
            && current.totp_secret != user.totp_secret)
    {
        return Err(invalid_credentials());
    }
    let user = &current;
    let mut current_ctx = ctx.clone();
    current_ctx.now = ctx.app.clock.now();
    let ctx = &current_ctx;
    let policy = system_policy(&mut tx).await?;
    refuse_the_person(user, &policy)?;
    let capabilities = proofs::capabilities(&mut tx, user).await?;
    if recovery_to_spend.is_some() && !capabilities.second(SignInProof::RecoveryCode) {
        return Err(second_step_invalid());
    }
    complete(&capabilities, &right)?;
    if let Some(hash) = code_to_spend
        && !identity::spend_sign_in_code(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(if password.is_some() {
            second_step_invalid()
        } else {
            invalid_credentials()
        });
    }
    if let Some(step) = step_to_accept
        && !identity::accept_totp_step(&mut tx, user.id, step).await?
    {
        return Err(second_step_invalid());
    }
    let recovered = recovery_to_spend.is_some();
    if let Some(hash) = recovery_to_spend
        && !orch_db::queries::recovery_codes::spend(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(second_step_invalid());
    }
    let opened = open_session(ctx, &mut tx, user, json!({ "proofs": right })).await?;
    if recovered {
        super::recovery_codes::grant_management(&mut tx, user, opened.session, ctx.now).await?;
    }
    // A sign-in that took the second step has given it for its session: the orgs that require
    // one do not ask this session again.
    if capabilities.has_second() {
        orch_db::queries::tokens::stamp_second_step(&mut tx, opened.session, ctx.now).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(opened)
}

/// Whether the proofs that were right complete the sign-in; if not, what is
/// asked for.
fn complete(capabilities: &Capabilities, right: &[SignInProof]) -> ApiResult<()> {
    if capabilities.complete(right) {
        return Ok(());
    }
    let methods = capabilities.missing(right);
    if methods.is_empty() {
        // A person with no second step, and a proof that is no way in for them.
        return Err(invalid_credentials());
    }
    Err(second_step_required(&methods))
}

/// Mails a code to the person the name is, if a code may be sent to them: it
/// answers nothing, and does the same work whatever the name. The code goes
/// out when the name is an active person's with a confirmed address, and
/// either a code may get them in by itself, or the password is given, right,
/// and a mailed code is a second step of theirs; and when no more than one
/// went to that name in the last minute and five in three hours.
pub async fn send_code(ctx: &Ctx, body: &AuthEmailCodeSendBody) -> ApiResult<()> {
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let Located { user, who } = throttle::locate(ctx, &mut conn, &body.login).await?;
    let found = user.as_ref();

    // A password given here is a try at the password like any other, and
    // counted with them. Where that count is full, or the address has failed
    // too often, the password is not looked at: it is not right, and the
    // answer is the same `202`.
    let mut password_right = false;
    if let Some(password) = given(&body.password) {
        match proofs::password_right(ctx, &mut conn, &limits, &who, found, password, true).await {
            Ok(right) => {
                password_right = right;
                if !right {
                    failed(ctx, &mut conn, &who, SignInProof::Password).await?;
                }
            }
            Err(error)
                if matches!(error.code(), ErrorCode::AuthLocked | ErrorCode::RateLimited) => {}
            Err(error) => return Err(error),
        }
    }

    let policy = system_policy(&mut conn).await?;
    let uses = people::email_code_uses(&mut conn, found).await?;
    let may_go = found.is_some_and(|user| {
        !user.suspended()
            && user.email_confirmed()
            && policy.mail()
            && (policy.local_login || user.is_system_admin)
            && (uses.contains(&EmailCodeUse::SignIn)
                || (password_right
                    && user.email_code_on()
                    && uses.contains(&EmailCodeUse::SecondStep)))
    });

    // Only a code that goes out is held against the pace: one in a minute and
    // five in three hours to a name, and to a known browser of its person.
    let go = identity::pace(
        &mut conn,
        &who.bucket("send"),
        ctx.now,
        &limits.send,
        may_go,
    )
    .await?;

    // One statement, which writes the code and the mail only if `go`.
    let to = Addressee {
        user_id: found.map_or_else(Id::nil, |user| user.id),
        email: found
            .and_then(|user| user.email.as_deref())
            .unwrap_or_default(),
        locale: found.map_or("en", |user| user.locale.as_str()),
    };
    // Order issuance with changes to the address or first proof. An old mailbox must not receive
    // a newly valid code after the replacement transaction has already cleared its old codes.
    drop(conn);
    let mut tx = ctx.app.db.begin().await?;
    let current = orch_db::queries::transaction_guard::lock_person(
        &mut tx,
        found.map_or_else(Id::nil, |user| user.id),
    )
    .await?;
    let still_current = found
        .zip(current.as_ref())
        .is_some_and(|(before, current)| {
            !current.suspended()
                && current.email_confirmed()
                && current.email == before.email
                && current.password_hash == before.password_hash
                && current.email_code_at == before.email_code_at
        });
    mail::send_code(ctx, &mut tx, policy.mail_channel, &to, go && still_current).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
