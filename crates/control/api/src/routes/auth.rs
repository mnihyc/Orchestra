//! Registration, signing in by a password, by a code sent by mail and through an identity provider, the second step, sessions, recovery and the change of an address.
//!
//! The requests that get a person in or give them back their way in are here:
//! registering, signing in, asking for a code, refreshing and ending a
//! session, recovering a password, and confirming an address. What a signed-in
//! person does to their own identity (their profile, password, second steps,
//! sessions and API tokens) is in [`me`]. Signing in through an identity
//! provider, and linking an identity to a person and unlinking it, are in
//! [`oidc`].
//!
//! The requests that answer `202` whatever the name or the address do the
//! same work for every one, so that they tell nothing.

mod bootstrap;
pub mod me;
pub mod oidc;
pub mod tokens;

use crate::middleware::authentication::{SESSION_PREFIX, new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use crate::sign_in::accounts::{self, Details, How};
use crate::sign_in::mail::{self, Addressee, Link, Purpose};
use crate::sign_in::throttle::{self, Who};
use crate::sign_in::{
    Limits, NOT_KEPT, Opened, access_life, cleared_session_cookie, login as sign_in_login,
    open_session, passwords, people, record, remember_browser, require_json, session_cookie,
    session_life, system_policy,
};
use http::StatusCode;
use http::header::{CACHE_CONTROL, SET_COOKIE};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::auth::{
    AuthEmailCodeSend, AuthEmailVerify, AuthEmailVerifyBody, AuthEmailVerifyResend,
    AuthEmailVerifyResendBody, AuthLogin, AuthLogout, AuthRecoveryCheck, AuthRecoveryConfirm,
    AuthRecoveryConfirmBody, AuthRecoveryStart, AuthRecoveryStartBody, AuthRefresh, AuthRegister,
    AuthRegisterBody, AuthRegisterReply, AuthRegisterResult, AuthResult, RegistrationMode,
    SignInProof, VerificationRequired,
};
use orch_api_contract::common::{ErrorCode, SESSION_COOKIE};
use orch_db::audit::Actor;
use orch_db::queries::identity;
use orch_types::{Id, Time};
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AuthRegister, register)
        .on(AuthLogin, login)
        .on(AuthEmailCodeSend, email_code)
        .on(AuthRefresh, refresh)
        .on(AuthLogout, logout)
        .on(AuthRecoveryStart, recovery)
        .on(AuthRecoveryCheck, recovery_check)
        .on(AuthRecoveryConfirm, recovery_confirm)
        .on(AuthEmailVerify, verify_email)
        .on(AuthEmailVerifyResend, verify_resend);
    me::mount(registry);
    oidc::mount(registry);
    tokens::mount(registry);
}

/// `410 gone`: a link that was used, that ended, or that nobody made, which
/// are not told apart.
fn gone() -> ApiError {
    ApiError::new(ErrorCode::Gone, "this link has been used, or has ended")
}

/// `403 registration.invite_required`: an invitation that is needed, or that
/// does not fit.
fn invitation_needed(detail: &str) -> ApiError {
    ApiError::new(ErrorCode::RegistrationInviteRequired, detail)
}

/// Whether two addresses are the same one, without regard to case.
fn same_address(first: &str, second: &str) -> bool {
    first.trim().to_lowercase() == second.trim().to_lowercase()
}

/// An answer with no body that sets these cookies.
fn with_cookies(cookies: &[String]) -> Answer<()> {
    cookies.iter().fold(Answer::from(()), |answer, value| {
        answer.header(SET_COOKIE, value)
    })
}

/// Registers a person: their own org is made with them, and they are signed
/// in, or asked to confirm their address first.
async fn register(
    mut ctx: Ctx,
    (): (),
    body: AuthRegisterBody,
) -> ApiResult<Answer<AuthRegisterReply>> {
    require_json(&ctx)?;
    let auth = &ctx.app.config.auth;
    let limits = Limits::of(auth);
    let (policy, invitation) = {
        let mut conn = ctx.app.db.acquire().await?;
        throttle::by_address(&ctx, &mut conn, &limits, "register").await?;
        let policy = system_policy(&mut conn).await?;
        let invitation = super::users::registration_offer(
            &mut conn,
            body.invitation.as_deref(),
            body.accept_invitation,
            ctx.now,
        )
        .await?;
        (policy, invitation)
    };
    match policy.registration {
        RegistrationMode::Closed => {
            return Err(ApiError::new(
                ErrorCode::RegistrationClosed,
                "registration is closed here",
            ));
        }
        RegistrationMode::Invite if invitation.is_none() => {
            return Err(invitation_needed(
                "this system takes new people by invitation, and none that is still good was given",
            ));
        }
        _ => {}
    }

    // A personal link is for one address, which the link reached, and which
    // therefore counts as confirmed.
    let bound = invitation.as_ref().and_then(|each| each.email.as_deref());
    let given = body
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty());
    let email = match (given, bound) {
        (Some(given), Some(bound)) if !same_address(given, bound) => {
            return Err(invitation_needed(
                "this invitation is for the address it was sent to",
            ));
        }
        (None, Some(bound)) => Some(bound),
        (given, _) => given,
    };
    let confirmed = bound.is_some();
    let ask_to_confirm = policy.verification_required && !confirmed;
    accounts::check(
        &Details {
            username: &body.username,
            display_name: &body.display_name,
            password: &body.password,
            email,
        },
        auth,
        ask_to_confirm,
    )?;
    let hash = passwords::hash(&body.password)
        .await
        .map_err(ApiError::internal)?;

    let mut tx = ctx.app.db.begin().await?;
    ctx.now = ctx.app.clock.now();
    let policy = system_policy(&mut tx).await?;
    let current_invitation = super::users::registration_offer(
        &mut tx,
        body.invitation.as_deref(),
        body.accept_invitation,
        ctx.now,
    )
    .await?;
    if policy.registration == RegistrationMode::Closed {
        return Err(ApiError::new(
            ErrorCode::RegistrationClosed,
            "registration is closed here",
        ));
    }
    if current_invitation.is_none()
        && (policy.registration == RegistrationMode::Invite || invitation.is_some())
    {
        return Err(invitation_needed(
            "the invitation no longer admits this registration",
        ));
    }
    let ask_to_confirm = policy.verification_required && !confirmed;
    let clean = accounts::check(
        &Details {
            username: &body.username,
            display_name: &body.display_name,
            password: &body.password,
            email,
        },
        auth,
        ask_to_confirm,
    )?;
    if ask_to_confirm
        && !accounts::public_name_taken(&mut tx, &clean.username).await?
        && let Some(email) = clean.email.as_deref()
        && identity::email_in_use(&mut tx, email, Id::nil()).await?
    {
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(
            Answer::from(AuthRegisterReply::Pending(VerificationRequired {
                verification_required: true,
            }))
            .header(CACHE_CONTROL, NOT_KEPT)
            .status(StatusCode::ACCEPTED),
        );
    }
    let user = accounts::make(
        &ctx,
        &mut tx,
        &clean,
        &hash,
        &How {
            email_confirmed: confirmed,
            is_system_admin: false,
            timezone: body.timezone.clone(),
        },
    )
    .await?;
    let redeemed = if current_invitation.is_some() {
        Some(
            super::users::redeem(
                &ctx,
                &mut tx,
                &user,
                body.invitation.as_deref().unwrap_or_default(),
            )
            .await
            .map_err(|error| {
                if error.code() == ErrorCode::Gone {
                    let mut refusal =
                        invitation_needed("the invitation no longer admits this registration");
                    if error
                        .problem
                        .extra
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        == Some(super::users::ISSUER_AUTHORITY_LOST)
                    {
                        refusal = refusal.with("reason", super::users::ISSUER_AUTHORITY_LOST);
                    }
                    refusal
                } else {
                    error
                }
            })?,
        )
    } else {
        None
    };
    if ask_to_confirm {
        // Signing in waits for the link, which is mailed in this transaction.
        let to = Addressee {
            user_id: user.id,
            email: clean.email.as_deref().unwrap_or_default(),
            locale: &user.locale,
        };
        mail::send_link(
            &ctx,
            &mut tx,
            policy.mail_channel,
            &Link {
                purpose: Purpose::Verify,
                to,
                clear_second_step: false,
            },
            true,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(
            Answer::from(AuthRegisterReply::Pending(VerificationRequired {
                verification_required: true,
            }))
            .header(CACHE_CONTROL, NOT_KEPT)
            .status(StatusCode::ACCEPTED),
        );
    }
    let how = json!({ "proofs": [SignInProof::Password], "registered": true });
    let opened = open_session(&ctx, &mut tx, &user, how).await?;
    let accepted_invitation = match redeemed.filter(|_| body.accept_invitation) {
        Some(row) => Some(Box::new(
            super::users::accepted_for_session(&ctx, &mut tx, &user, opened.session, &row).await?,
        )),
        None => None,
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let Opened {
        result, cookies, ..
    } = opened;
    Ok(cookies.iter().fold(
        Answer::from(AuthRegisterReply::Registered(AuthRegisterResult {
            auth: result,
            accepted_invitation,
        }))
        .header(CACHE_CONTROL, NOT_KEPT),
        |answer, value| answer.header(SET_COOKIE, value),
    ))
}

/// Signs a person in with one proof, or with two for somebody who has a
/// second step.
async fn login(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::auth::AuthLoginBody,
) -> ApiResult<Answer<AuthResult>> {
    require_json(&ctx)?;
    // The proof and session futures include policy and notification branches;
    // keep them out of the routing and authority wrappers' inline state.
    Ok(Box::pin(sign_in_login::login(&ctx, &body)).await?.answer())
}

/// Mails a sign-in code where one may be sent, and answers `202` whatever the
/// name.
async fn email_code(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::auth::AuthEmailCodeSendBody,
) -> ApiResult<()> {
    sign_in_login::send_code(&ctx, &body).await
}

/// The value of the session cookie of a request, where it has one in the
/// form of one.
fn session_value(ctx: &Ctx) -> Option<&str> {
    ctx.cookie(SESSION_COOKIE)
        .filter(|value| value.starts_with(SESSION_PREFIX))
}

/// Gives a new access token to the browser that holds the session cookie, and
/// renews the session: a session lasts as long after its last use as the
/// system says.
async fn refresh(mut ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<AuthResult>> {
    let Some(value) = session_value(&ctx).map(str::to_owned) else {
        return Err(ApiError::new(
            ErrorCode::AuthRequired,
            "there is no session cookie",
        ));
    };
    let invalid = || {
        ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the session has ended or is unknown",
        )
    };
    let auth = &ctx.app.config.auth;
    let mut tx = ctx.app.db.begin().await?;
    let hash = token_hash(&value);
    let session = identity::session_by_refresh_hash(&mut tx, &hash, ctx.now)
        .await?
        .ok_or_else(invalid)?;
    let user = orch_db::queries::transaction_guard::lock_person(&mut tx, session.user_id)
        .await?
        .ok_or_else(invalid)?;
    ctx.now = ctx.app.clock.now();
    let session = identity::session_by_refresh_hash(&mut tx, &hash, ctx.now)
        .await?
        .ok_or_else(invalid)?;
    if user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    let life = session_life(auth);
    identity::slide_session(&mut tx, session.id, ctx.now, ctx.now + life).await?;
    let access = new_token(crate::middleware::authentication::ACCESS_TOKEN_PREFIX);
    let access_expires_at = ctx.now + access_life(auth);
    identity::add_access_token(&mut tx, &token_hash(&access), session.id, access_expires_at)
        .await?;
    identity::touch_last_seen(&mut tx, user.id, ctx.now).await?;
    let me = people::me(&mut tx, &user, ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Answer::from(AuthResult {
        access_token: access,
        access_expires_at: Time::new(access_expires_at),
        user: me,
    })
    // The same cookie again, so that its life slides with the session's.
    .header(CACHE_CONTROL, NOT_KEPT)
    .header(SET_COOKIE, &session_cookie(&value, life)))
}

/// Ends the session of the browser and revokes it. A browser that has none is
/// as signed out as it can be, and is answered the same.
async fn logout(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<()>> {
    if let Some(value) = session_value(&ctx) {
        let mut tx = ctx.app.db.begin().await?;
        if let Some(session) =
            identity::session_by_refresh_hash(&mut tx, &token_hash(value), ctx.now).await?
            && orch_db::queries::transaction_guard::lock_person(&mut tx, session.user_id)
                .await?
                .is_some()
            && identity::session_by_refresh_hash(&mut tx, &token_hash(value), ctx.app.clock.now())
                .await?
                .is_some()
            && identity::revoke_session(&mut tx, session.id, session.user_id, ctx.now).await?
        {
            record(
                &ctx,
                &mut tx,
                Actor::User(session.user_id),
                orch_db::audit::action!(AuditAction::AuthLogout),
                Some(session.user_id),
                "a session was ended by signing out",
                json!({ "session": session.id }),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
    }
    // The mark of the browser stays: signing out does not make it a stranger.
    Ok(Answer::from(()).header(SET_COOKIE, &cleared_session_cookie()))
}

/// Mails a link to reset a password to the person whose confirmed address it
/// is, if there is one. It answers `202` whatever the address, after the same
/// work.
async fn recovery(ctx: Ctx, (): (), body: AuthRecoveryStartBody) -> ApiResult<()> {
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.begin().await?;
    throttle::by_address(&ctx, &mut conn, &limits, "recovery").await?;
    let email = throttle::normalize(&body.email);
    let user = identity::user_by_confirmed_email(&mut conn, &email).await?;
    let user = orch_db::queries::transaction_guard::lock_person(
        &mut conn,
        user.as_ref().map_or_else(Id::nil, |user| user.id),
    )
    .await?
    .filter(|user| {
        !user.suspended()
            && user.email_confirmed()
            && user
                .email
                .as_deref()
                .is_some_and(|current| same_address(current, &email))
    });
    let policy = system_policy(&mut conn).await?;
    // So that nobody is sent mail without end by somebody else, links are
    // paced like codes are: by the name, here the address.
    let who = Who::named(user.as_ref().map(|user| user.id), &email);
    let may_go = user.as_ref().is_some_and(|user| !user.suspended()) && policy.mail();
    let go = identity::pace(
        &mut conn,
        &who.bucket("recovery"),
        ctx.now,
        &limits.send,
        may_go,
    )
    .await?;
    let to = Addressee {
        user_id: user.as_ref().map_or_else(Id::nil, |user| user.id),
        email: user
            .as_ref()
            .and_then(|user| user.email.as_deref())
            .unwrap_or_default(),
        locale: user.as_ref().map_or("en", |user| user.locale.as_str()),
    };
    mail::send_link(
        &ctx,
        &mut conn,
        policy.mail_channel,
        &Link {
            purpose: Purpose::Reset,
            to,
            clear_second_step: false,
        },
        go,
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Checks a reset link without spending it or returning the account it belongs to.
async fn recovery_check(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<()>> {
    let mut conn = ctx.app.db.acquire().await?;
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM email_tokens t JOIN users u ON u.id=t.user_id
          WHERE t.token_hash=$1 AND t.purpose=$2 AND t.used_at IS NULL AND t.expires_at>$3
            AND u.status='active' AND lower(trim(t.email))=lower(trim(COALESCE(u.email,''))))",
    )
    .bind(token_hash(ctx.param("token")?))
    .bind(Purpose::Reset.stored())
    .bind(ctx.now)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if !live {
        return Err(gone());
    }
    Ok(Answer::from(()).header(CACHE_CONTROL, NOT_KEPT))
}

/// Sets a new password with the token of a reset link, ends every session of
/// the person, and makes this browser one they got in from, so that somebody
/// locked on a browser that is new gets in with the new password. A link that
/// a system admin made for somebody who lost their second step switches it off
/// too.
async fn recovery_confirm(
    mut ctx: Ctx,
    (): (),
    body: AuthRecoveryConfirmBody,
) -> ApiResult<Answer<()>> {
    let auth = &ctx.app.config.auth;
    // Hash before taking the person's row. The token and password rules are
    // checked again in the transaction, after any concurrent identity change.
    let candidate = {
        let mut conn = ctx.app.db.acquire().await?;
        let person = identity::email_token_owner(
            &mut conn,
            &token_hash(&body.token),
            Purpose::Reset.stored(),
            ctx.now,
        )
        .await?
        .ok_or_else(gone)?;
        identity::user_by_id(&mut conn, person)
            .await?
            .ok_or_else(gone)?
    };
    passwords::check_new(
        &body.password,
        auth.password_min_length,
        &candidate.username,
        candidate.email.as_deref(),
    )
    .map_err(|refused| refused.into_api_error("password"))?;
    let password_hash = passwords::hash(&body.password)
        .await
        .map_err(ApiError::internal)?;
    let mut tx = ctx.app.db.begin().await?;
    let hash = token_hash(&body.token);
    let person = identity::email_token_owner(&mut tx, &hash, Purpose::Reset.stored(), ctx.now)
        .await?
        .ok_or_else(gone)?;
    let user = orch_db::queries::transaction_guard::lock_person(&mut tx, person)
        .await?
        .ok_or_else(gone)?;
    ctx.now = ctx.app.clock.now();
    let token = identity::use_email_token(&mut tx, &hash, Purpose::Reset.stored(), ctx.now)
        .await?
        .ok_or_else(gone)?;
    if user.suspended() || !same_address(user.email.as_deref().unwrap_or_default(), &token.email) {
        return Err(gone());
    }
    // A password that is refused leaves the link unused: the transaction ends
    // here, and so does the use of it.
    if let Err(refused) = passwords::check_new(
        &body.password,
        auth.password_min_length,
        &user.username,
        user.email.as_deref(),
    ) {
        return Err(refused.into_api_error("password"));
    }
    identity::set_password(&mut tx, user.id, &password_hash, ctx.now).await?;
    identity::void_email_tokens(&mut tx, user.id, Purpose::Reset.stored(), ctx.now).await?;
    let ended = identity::revoke_sessions_of(&mut tx, user.id, None, ctx.now).await?;
    if token.clear_second_step {
        identity::disable_totp(&mut tx, user.id, ctx.now).await?;
        identity::set_email_code_second_step(&mut tx, user.id, None, ctx.now).await?;
        record(
            &ctx,
            &mut tx,
            Actor::User(user.id),
            orch_db::audit::action!(AuditAction::AuthSecondStepCleared),
            Some(user.id),
            "the second step was switched off by a reset link made for it",
            json!({ "totp": user.totp_on(), "email_code": user.email_code_on() }),
        )
        .await?;
    }
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthPasswordReset),
        Some(user.id),
        &format!(
            "the password of {} was set anew by a reset link",
            user.username
        ),
        json!({ "sessions_ended": ended }),
    )
    .await?;
    let browser = remember_browser(&ctx, &mut tx, user.id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(with_cookies(&[browser]))
}

/// Confirms an address with the token of the link that was mailed. It takes
/// the place of the address the person has, when they asked to change it. And
/// it signs the browser in, as the link that reached the address does, unless
/// the person has a second step that a link cannot stand for.
async fn verify_email(mut ctx: Ctx, (): (), body: AuthEmailVerifyBody) -> ApiResult<Answer<()>> {
    require_json(&ctx)?;
    let limits = Limits::of(&ctx.app.config.auth);
    {
        let mut conn = ctx.app.db.acquire().await?;
        throttle::by_address(&ctx, &mut conn, &limits, "verify").await?;
    }
    let mut tx = ctx.app.db.begin().await?;
    let hash = token_hash(&body.token);
    let person = identity::email_token_owner(&mut tx, &hash, Purpose::Verify.stored(), ctx.now)
        .await?
        .ok_or_else(gone)?;
    let user = orch_db::queries::transaction_guard::lock_person(&mut tx, person)
        .await?
        .ok_or_else(gone)?;
    ctx.now = ctx.app.clock.now();
    let token = identity::use_email_token(&mut tx, &hash, Purpose::Verify.stored(), ctx.now)
        .await?
        .ok_or_else(gone)?;
    if user.suspended() {
        return Err(gone());
    }
    // The address they have takes another's place, or is given for the first time.
    let changes = user
        .email
        .as_deref()
        .is_none_or(|current| !same_address(current, &token.email));
    identity::set_email(&mut tx, user.id, &token.email, true, ctx.now)
        .await
        .map_err(|error| match error {
            orch_db::DbError::Unique { .. } => ApiError::new(
                ErrorCode::ConflictExists,
                "that address is in use by somebody else",
            ),
            other => ApiError::from(other),
        })?;
    if changes {
        // Mail sent to the former address must not remain a way into this account.
        identity::void_email_tokens(&mut tx, user.id, Purpose::Reset.stored(), ctx.now).await?;
        sqlx::query("DELETE FROM sign_in_codes WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        record(
            &ctx,
            &mut tx,
            Actor::User(user.id),
            orch_db::audit::action!(AuditAction::AuthEmailChanged),
            Some(user.id),
            &format!("the address of {} was changed", user.username),
            json!({ "from": user.email, "to": token.email }),
        )
        .await?;
    }
    // Links to confirm other addresses than this one are no longer wanted.
    identity::void_email_tokens(&mut tx, user.id, Purpose::Verify.stored(), ctx.now).await?;
    let policy = system_policy(&mut tx).await?;
    let signs_in = !user.suspended()
        && !user.totp_on()
        && !user.email_code_on()
        && (policy.local_login || user.is_system_admin);
    let cookies = if signs_in {
        open_session(&ctx, &mut tx, &user, json!({ "by": "email_link" }))
            .await?
            .cookies
    } else {
        Vec::new()
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(with_cookies(&cookies))
}

/// Mails the confirmation link again to the person whose address is not
/// confirmed, if there is one. It answers `202` whatever the name, after the
/// same work.
async fn verify_resend(ctx: Ctx, (): (), body: AuthEmailVerifyResendBody) -> ApiResult<()> {
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.begin().await?;
    throttle::by_address(&ctx, &mut conn, &limits, "verify").await?;
    let name = throttle::normalize(&body.login);
    let user = identity::user_by_login(&mut conn, &name).await?;
    let user = orch_db::queries::transaction_guard::lock_person(
        &mut conn,
        user.as_ref().map_or_else(Id::nil, |user| user.id),
    )
    .await?;
    let policy = system_policy(&mut conn).await?;
    let who = Who::named(user.as_ref().map(|user| user.id), &name);
    let may_go = policy.mail()
        && user.as_ref().is_some_and(|user| {
            !user.suspended() && user.email.is_some() && !user.email_confirmed()
        });
    let go = identity::pace(
        &mut conn,
        &who.bucket("verify"),
        ctx.now,
        &limits.send,
        may_go,
    )
    .await?;
    let to = Addressee {
        user_id: user.as_ref().map_or_else(Id::nil, |user| user.id),
        email: user
            .as_ref()
            .and_then(|user| user.email.as_deref())
            .unwrap_or_default(),
        locale: user.as_ref().map_or("en", |user| user.locale.as_str()),
    };
    mail::send_link(
        &ctx,
        &mut conn,
        policy.mail_channel,
        &Link {
            purpose: Purpose::Verify,
            to,
            clear_second_step: false,
        },
        go,
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
