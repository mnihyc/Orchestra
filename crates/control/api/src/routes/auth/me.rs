//! The signed-in person: what they set for themself, their password, their second steps, their sessions and their API tokens.
//!
//! A wrong password, or a wrong code, given by a signed-in caller to change
//! the password or the address or to set up or switch off a second step is
//! counted for that session alone (or that token), with the limit of signing
//! in, and refused the same way when it is full: whoever holds a session that
//! was not theirs gets no more tries at the password than a stranger does.

use crate::middleware::authentication::{API_TOKEN_PREFIX, Authed, Via, new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use crate::sign_in::accounts::{self, taken};
use crate::sign_in::login::{invalid_credentials, second_step_invalid};
use crate::sign_in::mail::{self, Addressee, Link, Message, Purpose};
use crate::sign_in::throttle::{Who, rate_limited};
use crate::sign_in::{Limits, NOT_KEPT, codes, passwords, people, proofs, record, system_policy};
use http::header::CACHE_CONTROL;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::auth::{
    AdminMode, AdminModeSetBody, AdminOpeningBody, ApiTokenCreateBody, ApiTokenInfo,
    MeAdminModeGet, MeAdminModeSet, MeAdminOpening, MeEmailCodeDisable, MeEmailCodeDisableBody,
    MeEmailCodeEnable, MeEmailCodeEnableBody, MeEmailCodeSend, MeEmailCodeSendBody,
    MePasswordChange, MePasswordChangeBody, MeRecoveryCodesGet, MeRecoveryCodesRegenerate,
    MeRecoveryCodesRegenerateBody, MeSecondStepVerify, MeSecondStepVerifyBody, MeSessionDelete,
    MeSessionList, MeSessionOthersDelete, MeTokenCreate, MeTokenDelete, MeTokenList, MeTotpDisable,
    MeTotpDisableBody, MeTotpEnable, MeTotpEnableBody, MeTotpEnableReply, MeTotpSetup,
    MeTotpSetupBody, MeTotpSetupReply, MeUpdate, MeUpdateBody, RecoveryCodeBatch,
    RecoveryCodeStatus, SessionInfo, SignInProof, TokenCreated,
};
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::users::{EmailCodeUse, Me, TimezoneMode};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::queries::access;
use orch_db::queries::identity::{
    self, ApiTokenListed, NewApiToken, ProfileChange, Reservation, UserRow,
};
use orch_types::{Id, Time};
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

/// The fewest and the most characters of a display name: what the schema
/// allows.
const DISPLAY_NAME_CHARS: std::ops::RangeInclusive<usize> = 1..=64;

/// The twelve identity palette indexes accepted by every avatar control.
const AVATAR_COLORS: std::ops::RangeInclusive<u16> = 0..=11;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(orch_api_contract::auth::MeGet, me_get)
        .on(
            orch_api_contract::auth::MeBootstrapGet,
            super::bootstrap::get,
        )
        .on(
            orch_api_contract::auth::MeAuthorityGet,
            super::bootstrap::authority,
        )
        .on(MeUpdate, me_update)
        .on(MeAdminModeGet, admin_mode_get)
        .on(MeAdminModeSet, admin_mode_set)
        .on(MeAdminOpening, admin_opening)
        .on(MePasswordChange, password_change)
        .on(MeTotpSetup, totp_setup)
        .on(MeTotpEnable, totp_enable)
        .on(MeRecoveryCodesGet, recovery_codes_get)
        .on(MeRecoveryCodesRegenerate, recovery_codes_regenerate)
        .on(MeTotpDisable, totp_disable)
        .on(MeEmailCodeSend, code_send)
        .on(MeEmailCodeEnable, code_enable)
        .on(MeEmailCodeDisable, code_disable)
        .on(MeSecondStepVerify, second_step_verify)
        .on(MeSessionList, sessions)
        .on(MeSessionOthersDelete, session_others_delete)
        .on(MeSessionDelete, session_delete)
        .on(MeTokenList, tokens)
        .on(MeTokenCreate, token_create)
        .on(MeTokenDelete, token_delete);
}

fn fault(path: &str, message: &str) -> FieldFault {
    FieldFault {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

async fn admin_mode_get(ctx: Ctx, (): (), (): ()) -> ApiResult<AdminMode> {
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    require_browser(guard.user())?;
    let available =
        identity::admin_mode_available(&mut tx, guard.user().id(), guard.now().get()).await?;
    let reply = AdminMode {
        as_admin: guard.user().as_admin,
        available,
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}

async fn admin_opening(ctx: Ctx, (): (), body: AdminOpeningBody) -> ApiResult<()> {
    crate::middleware::admin_openings::cached(&ctx, &body.kind, body.id, body.opening_id).await
}

async fn admin_mode_set(ctx: Ctx, (): (), body: AdminModeSetBody) -> ApiResult<AdminMode> {
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let user = guard.user();
    require_browser(user)?;
    let session = current_session(user).ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthSessionRequired,
            "administrator mode belongs to a browser session",
        )
    })?;
    let now = guard.now().get();
    let available = identity::admin_mode_available(&mut tx, user.id(), now).await?;
    if body.as_admin && !available {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "you administer no place",
        ));
    }
    if user.as_admin != body.as_admin {
        if !identity::set_session_admin_mode(&mut tx, user.id(), session, body.as_admin, now)
            .await?
        {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "this session has ended",
            ));
        }
        orch_db::audit::set_admin_mode(user.id(), body.as_admin);
        orch_db::sequences::notify_user_event(
            &mut tx,
            &[user.id()],
            &serde_json::to_value(orch_realtime_contract::feeds::UserEvent::AdminMode {
                session_id: session,
                as_admin: body.as_admin,
            })
            .map_err(ApiError::internal)?,
        )
        .await?;
        record(
            &ctx,
            &mut tx,
            Actor::User(user.id()),
            orch_db::audit::action!(AuditAction::AuthAdminModeChanged),
            Some(user.id()),
            "changed this session's As admin switch",
            json!({ "session": session, "as_admin": body.as_admin }),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(AdminMode {
        as_admin: body.as_admin,
        available,
    })
}

/// A member of a body that is a word, as the database keeps it.
fn word<T: Serialize>(value: Option<&T>) -> Option<String> {
    value
        .and_then(|value| serde_json::to_value(value).ok())
        .and_then(|value| value.as_str().map(str::to_owned))
}

/// Whose tries these are: the session's, or the token's.
fn caller_who(authed: &Authed) -> Who {
    match &authed.via {
        Via::Session { session_id } => Who::caller('s', *session_id, authed.id()),
        Via::Token { token_id, .. } => Who::caller('t', *token_id, authed.id()),
    }
}

/// The session of the request, where it is one.
fn current_session(authed: &Authed) -> Option<Id> {
    match authed.via {
        Via::Session { session_id } => Some(session_id),
        Via::Token { .. } => None,
    }
}

/// Passwords and browser sign-ins are managed from a browser sign-in alone.
fn require_browser(authed: &Authed) -> ApiResult<()> {
    if authed.is_session() {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "this is done from a browser sign-in",
        ))
    }
}

/// Counts and looks at a password given by a signed-in caller to change
/// something. A person who signs in only through an identity provider has no
/// password to confirm.
async fn confirm_password(ctx: &Ctx, authed: &Authed, given: &str) -> ApiResult<()> {
    if authed.user.password_hash.is_none() {
        return Ok(());
    }
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let who = caller_who(authed);
    if proofs::password_right(
        ctx,
        &mut conn,
        &limits,
        &who,
        Some(&authed.user),
        given,
        false,
    )
    .await?
    {
        Ok(())
    } else {
        Err(invalid_credentials())
    }
}

/// A member of a body that was filled in: a form that sends what it left empty as an empty text has
/// given nothing.
fn filled(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// What setting a second step up takes, beside the code that switches it on: the password where the
/// person has one; where they have none, the one-use proof their identity provider gave this browser
/// session for setting one up, whose hash is answered so that the act spends it in its transaction.
/// A person with a password cannot give the identity provider's proof instead.
async fn setup_proof(
    ctx: &Ctx,
    authed: &Authed,
    password: Option<&str>,
    identity_proof: Option<&str>,
) -> ApiResult<Option<Vec<u8>>> {
    use crate::sign_in::fresh::{Purpose, reauthenticate};
    if authed.user.password_hash.is_some() {
        let Some(password) = password else {
            return Err(reauthenticate(Purpose::SecondStepSetup, "password"));
        };
        confirm_password(ctx, authed, password).await?;
        return Ok(None);
    }
    let Some(proof) = identity_proof else {
        return Err(reauthenticate(Purpose::SecondStepSetup, "identity_proof"));
    };
    // The proof is bound to the browser session it was made for.
    require_browser(authed)?;
    Ok(Some(token_hash(proof)))
}

/// Spends, in the act's transaction, the identity provider's proof that a setup was given: a proof
/// that was spent already, ran out, or was made for another session or another act refuses it.
async fn spend_setup_proof(
    ctx: &Ctx,
    tx: &mut Conn,
    authed: &Authed,
    proof: Option<&[u8]>,
) -> ApiResult<()> {
    use crate::sign_in::fresh::{Purpose, reauthenticate};
    let Some(hash) = proof else {
        return Ok(());
    };
    let spent = match current_session(authed) {
        Some(session) => {
            orch_db::queries::tokens::spend_sensitive_proof(
                tx,
                hash,
                session,
                Purpose::SecondStepSetup.stored(),
                None,
                ctx.now,
            )
            .await?
        }
        None => false,
    };
    if spent {
        Ok(())
    } else {
        Err(reauthenticate(Purpose::SecondStepSetup, "identity_proof"))
    }
}

/// Notes that this browser session gave its second proof, in the act's transaction: setting a step
/// up with its confirming code gives it, and the orgs that require one stop waiting for the session.
async fn stamp_given(ctx: &Ctx, tx: &mut Conn, authed: &Authed) -> ApiResult<()> {
    if let Some(session) = current_session(authed) {
        orch_db::queries::tokens::stamp_second_step(tx, session, ctx.now).await?;
    }
    Ok(())
}

/// Reads the signed-in person.
async fn me_get(ctx: Ctx, (): (), (): ()) -> ApiResult<Me> {
    let authed = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    people::me(&mut conn, &authed.user, ctx.now).await
}

/// Changes personal settings, with an address change confirmed separately where mail is available.
async fn me_update(mut ctx: Ctx, (): (), body: MeUpdateBody) -> ApiResult<Me> {
    let mut faults = Vec::new();
    let display_name = body.display_name.as_deref().map(str::trim);
    if display_name.is_some_and(|name| !DISPLAY_NAME_CHARS.contains(&name.chars().count())) {
        faults.push(fault("display_name", "must be 1 to 64 characters"));
    }
    if body
        .timezone
        .as_deref()
        .is_some_and(|zone| zone.parse::<chrono_tz::Tz>().is_err())
    {
        faults.push(fault("timezone", "is not the name of a time zone"));
    }
    // A browser's own report of its zone names the zone, and is never a choice of a fixed one.
    let report = body.timezone_report == Some(true);
    if report && (body.timezone.is_none() || body.timezone_mode == Some(TimezoneMode::Fixed)) {
        faults.push(fault(
            "timezone_report",
            "is a device's report of its zone: it comes with timezone and timezone_mode device",
        ));
    }
    if body
        .avatar_color
        .flatten()
        .is_some_and(|color| !AVATAR_COLORS.contains(&color))
    {
        faults.push(fault("avatar_color", "must be from 0 to 11"));
    }
    let new_email = body
        .email
        .as_ref()
        .and_then(Option::as_deref)
        .map(str::trim);
    if new_email.is_some_and(|email| !accounts::email_fits(email)) {
        faults.push(fault("email", "is not an address"));
    }
    if !faults.is_empty() {
        return Err(Problem::invalid(faults).into());
    }

    let (mut tx, guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let authed = guard.user();
    let user = &authed.user;
    let address_change = body.email.as_ref().and_then(|requested| {
        let requested = requested.as_deref().map(str::trim);
        match requested {
            Some(new)
                if user
                    .email
                    .as_deref()
                    .is_some_and(|old| old.trim().to_lowercase() == new.to_lowercase()) =>
            {
                None
            }
            other => Some(other),
        }
    });
    if address_change.is_some() && user.password_hash.is_some() {
        let current = body.current_password.as_deref().ok_or_else(|| {
            ApiError::invalid("current_password", "is needed to change the address")
        })?;
        // Count failures on this connection: the guard already holds the user row,
        // and the lockout notice can update that user's Inbox sequence as well.
        let limits = Limits::of(&ctx.app.config.auth);
        let proof = proofs::password_right(
            &ctx,
            &mut tx,
            &limits,
            &caller_who(authed),
            Some(user),
            current,
            false,
        )
        .await;
        if !matches!(proof, Ok(true)) {
            // No profile field has been written; failed-try bookkeeping must survive.
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Err(match proof {
                Err(error) => error,
                _ => invalid_credentials(),
            });
        }
    }
    if let Some(Some(new)) = address_change
        && identity::email_in_use(&mut tx, new, user.id).await?
    {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "the account change could not be completed",
        ));
    }
    let policy = if address_change.is_some() {
        Some(system_policy(&mut tx).await?)
    } else {
        None
    };
    if matches!(address_change, Some(None)) {
        if user.email_code_on() {
            return Err(ApiError::invalid(
                "email",
                "switch off the mailed second step before clearing the address",
            ));
        }
        let password_works = user.password_hash.is_some()
            && policy
                .as_ref()
                .is_some_and(|policy| policy.local_login || user.is_system_admin);
        let identity_works: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_identities i JOIN identity_providers p
               ON p.id = i.identity_provider_id WHERE i.user_id = $1 AND p.enabled)",
        )
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        if !password_works && !identity_works {
            return Err(ApiError::invalid(
                "email",
                "keep another allowed way to sign in before clearing the address",
            ));
        }
    }
    if let Some(choice) = body.avatar_color {
        let color = match choice {
            Some(color) => i16::try_from(color).map_err(ApiError::internal)?,
            None => identity::avatar_color(user.id),
        };
        sqlx::query("UPDATE users SET avatar_color = $2, updated_at = $3 WHERE id = $1")
            .bind(user.id)
            .bind(color)
            .bind(ctx.now)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    // A zone given without a mode is one the person chose, and stays on every device. A device's
    // report is written apart, and only while the zone still follows the device, so that a report
    // that arrives after a choice changes nothing.
    let timezone_mode = match (body.timezone_mode, body.timezone.is_some()) {
        _ if report => None,
        (Some(mode), _) => Some(mode),
        (None, true) => Some(TimezoneMode::Fixed),
        (None, false) => None,
    };
    let mut row = identity::update_profile(
        &mut tx,
        user.id,
        &ProfileChange {
            display_name,
            locale: word(body.locale.as_ref()).as_deref(),
            timezone: body.timezone.as_deref().filter(|_| !report),
            timezone_mode: word(timezone_mode.as_ref()).as_deref(),
            theme: word(body.theme.as_ref()).as_deref(),
            density: word(body.density.as_ref()).as_deref(),
        },
        ctx.now,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    if let Some(zone) = body.timezone.as_deref().filter(|_| report)
        && identity::report_device_timezone(&mut tx, user.id, zone, ctx.now).await?
    {
        row = identity::user_by_id(&mut tx, user.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
    }
    match (address_change, policy) {
        (Some(Some(new)), Some(policy)) if policy.mail() => {
            identity::void_email_tokens(&mut tx, user.id, Purpose::Verify.stored(), ctx.now)
                .await?;
            mail::send_link(
                &ctx,
                &mut tx,
                policy.mail_channel,
                &Link {
                    purpose: Purpose::Verify,
                    to: Addressee {
                        user_id: user.id,
                        email: new,
                        locale: &row.locale,
                    },
                    clear_second_step: false,
                },
                true,
            )
            .await?;
            if let (true, Some(old)) = (row.email_confirmed(), row.email.as_deref()) {
                mail::send_notice(
                    &ctx,
                    &mut tx,
                    policy.mail_channel,
                    &Addressee {
                        user_id: user.id,
                        email: old,
                        locale: &row.locale,
                    },
                    &Message::AddressChanged { new_email: new },
                )
                .await?;
            }
        }
        (Some(requested), policy) => {
            if requested.is_none()
                && row.email_confirmed()
                && let (Some(old), Some(policy)) = (row.email.as_deref(), policy.as_ref())
                && let Some(channel) = policy.mail_channel
            {
                identity::queue_mail(&mut tx, mail::CHANNEL_KIND, channel, &json!({
                    "purpose": "email_changed", "to": old, "user_id": user.id,
                    "language": row.locale, "data": { "new_email": null },
                    "subject": format!("The address of your {} account was cleared", ctx.app.config.system.name),
                    "body": "The address of your account was cleared. Codes and resets no longer go to this address. If this was not you, change your password.",
                }), ctx.now).await?;
            }
            identity::void_email_tokens(&mut tx, user.id, Purpose::Verify.stored(), ctx.now)
                .await?;
            identity::void_email_tokens(&mut tx, user.id, Purpose::Reset.stored(), ctx.now).await?;
            sqlx::query("DELETE FROM sign_in_codes WHERE user_id = $1")
                .bind(user.id)
                .execute(&mut *tx)
                .await
                .map_err(orch_db::DbError::from)?;
            let old = row.email.clone();
            sqlx::query("UPDATE users SET email = $2, email_verified_at = NULL, updated_at = $3 WHERE id = $1")
                .bind(user.id).bind(requested).bind(ctx.now).execute(&mut *tx)
                .await.map_err(orch_db::DbError::from).map_err(taken)?;
            record(
                &ctx,
                &mut tx,
                Actor::User(user.id),
                AuditAction::AuthEmailChanged,
                Some(user.id),
                &format!("the address of {} was changed", user.username),
                json!({ "from": old, "to": requested, "confirmed": false }),
            )
            .await?;
            orch_authz::epoch::bump(&mut tx)
                .await
                .map_err(ApiError::internal)?;
            row = identity::user_by_id(&mut tx, user.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
        }
        _ => {}
    }
    let me = people::me(&mut tx, &row, ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(me)
}

/// Locks the current personal identity and rejects a proof of a replaced credential state.
async fn credential_action(ctx: &mut Ctx) -> ApiResult<orch_db::Tx> {
    let (tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    let original = &ctx.user()?.user;
    let current = &guard.user().user;
    if original.password_hash != current.password_hash
        || original.totp_secret != current.totp_secret
        || original.totp_enabled_at != current.totp_enabled_at
        || original.email_code_at != current.email_code_at
        || original.email != current.email
        || original.email_verified_at != current.email_verified_at
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidCredentials,
            "your ways to sign in changed; try again",
        ));
    }
    ctx.now = guard.now().get();
    Ok(tx)
}

/// Changes the password after rechecking the identity and the proof's credential state.
async fn password_change(mut ctx: Ctx, (): (), body: MePasswordChangeBody) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    let user = &authed.user;
    confirm_password(&ctx, &authed, &body.current_password).await?;
    if let Err(refused) = passwords::check_new(
        &body.new_password,
        ctx.app.config.auth.password_min_length,
        &user.username,
        user.email.as_deref(),
    ) {
        return Err(refused.into_api_error("new_password"));
    }
    let hash = passwords::hash(&body.new_password)
        .await
        .map_err(ApiError::internal)?;
    let mut tx = credential_action(&mut ctx).await?;
    identity::set_password(&mut tx, user.id, &hash, ctx.now).await?;
    let ended =
        identity::revoke_sessions_of(&mut tx, user.id, current_session(&authed), ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthPasswordChanged),
        Some(user.id),
        &format!("{} changed their password", user.username),
        json!({ "sessions_ended": ended }),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// The reply of a setup: the secret in text and as an address an app can read
/// from a picture, which no cache keeps.
fn setup_reply(ctx: &Ctx, user: &UserRow, secret: Vec<u8>) -> ApiResult<Answer<MeTotpSetupReply>> {
    let totp = codes::authenticator(secret, &user.username, &ctx.app.config.system.name)
        .map_err(ApiError::internal)?;
    Ok(Answer::from(MeTotpSetupReply {
        secret: totp.secret().to_base32(),
        otpauth_url: totp
            .to_url()
            .map_err(|error| ApiError::internal(format!("{error:?}")))?,
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// Begins the setup of an authenticator app and returns its secret. Asking
/// again before it is switched on gives the same secret. It takes the password,
/// or for a person who has none the one-use proof of their identity provider,
/// which is spent with the secret it gave.
async fn totp_setup(
    mut ctx: Ctx,
    (): (),
    body: MeTotpSetupBody,
) -> ApiResult<Answer<MeTotpSetupReply>> {
    let authed = Arc::clone(ctx.user()?);
    let proof = setup_proof(
        &ctx,
        &authed,
        filled(&body.password),
        filled(&body.identity_proof),
    )
    .await?;
    let mut tx = credential_action(&mut ctx).await?;
    spend_setup_proof(&ctx, &mut tx, &authed, proof.as_deref()).await?;
    // What is stored now, which may have been changed since the request began.
    let user = identity::user_by_id(&mut tx, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if user.totp_on() {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "the authenticator is on already",
        ));
    }
    let secret = match user.totp_secret.as_deref() {
        Some(sealed) => codes::open_secret(user.id, sealed).map_err(ApiError::internal)?,
        None => {
            let secret = codes::new_secret();
            let sealed = codes::seal_secret(user.id, &secret).map_err(ApiError::internal)?;
            if identity::put_pending_totp(&mut tx, user.id, &sealed, ctx.now).await? {
                secret
            } else {
                // Another request stored a secret first: that one is the secret.
                let stored = identity::user_by_id(&mut tx, user.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let sealed = stored.totp_secret.as_deref().ok_or_else(|| {
                    ApiError::new(ErrorCode::ConflictExists, "the authenticator is on already")
                })?;
                codes::open_secret(user.id, sealed).map_err(ApiError::internal)?
            }
        }
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    setup_reply(&ctx, &user, secret)
}

/// Switches the authenticator app on, with a code it shows.
async fn totp_enable(
    mut ctx: Ctx,
    (): (),
    body: MeTotpEnableBody,
) -> ApiResult<Answer<MeTotpEnableReply>> {
    let authed = Arc::clone(ctx.user()?);
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let user = identity::user_by_id(&mut conn, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if user.totp_on() {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "the authenticator is on already",
        ));
    }
    if user.totp_secret.is_none() {
        return Err(ApiError::invalid("code", "set the authenticator up first"));
    }
    let who = caller_who(&authed);
    let step =
        proofs::authenticator_right(&ctx, &mut conn, &limits, &who, &user, &body.code, false)
            .await?
            .ok_or_else(second_step_invalid)?;
    let issued = crate::sign_in::recovery_codes::issue();
    let hashes = crate::sign_in::recovery_codes::hashes(user.id, &issued)
        .ok_or_else(|| ApiError::internal("a generated recovery code is malformed"))?;
    drop(conn);
    let mut tx = credential_action(&mut ctx).await?;
    if !identity::enable_totp(&mut tx, user.id, step, ctx.now).await? {
        return Err(second_step_invalid());
    }
    orch_db::queries::recovery_codes::replace(&mut tx, user.id, &hashes, ctx.now).await?;
    stamp_given(&ctx, &mut tx, &authed).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthTotpEnabled),
        Some(user.id),
        &format!("{} switched on an authenticator", user.username),
        json!({}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    second_step_set_up(&ctx, user.id).await;
    let mut conn = ctx.app.db.acquire().await?;
    let current = identity::user_by_id(&mut conn, user.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let me = Box::pin(people::me(&mut conn, &current, ctx.now)).await?;
    let status = recovery_status(&mut conn, &current, current_session(&authed), ctx.now).await?;
    Ok(Answer::from(MeTotpEnableReply {
        me,
        recovery_codes: RecoveryCodeBatch {
            codes: issued,
            status,
        },
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// The explicit management dialog loads metadata once; no read can recover a plaintext batch.
async fn recovery_codes_get(ctx: Ctx, (): (), (): ()) -> ApiResult<RecoveryCodeStatus> {
    let authed = ctx.user()?;
    require_browser(authed)?;
    let mut conn = ctx.app.db.acquire().await?;
    recovery_status(&mut conn, &authed.user, current_session(authed), ctx.now).await
}

async fn recovery_status(
    conn: &mut Conn,
    user: &UserRow,
    session: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<RecoveryCodeStatus> {
    let status = orch_db::queries::recovery_codes::status(conn, user.id).await?;
    let capabilities = proofs::capabilities_with_recovery(conn, user, status.remaining > 0).await?;
    let methods = [
        SignInProof::Totp,
        SignInProof::RecoveryCode,
        SignInProof::EmailCode,
    ]
    .into_iter()
    .filter(|proof| capabilities.second(*proof))
    .collect();
    let recent_recovery = if let (Some(session), Some(secret)) = (
        session,
        user.totp_secret.as_deref().filter(|_| user.totp_on()),
    ) {
        let factor = crate::sign_in::recovery_codes::factor_hash(user.id, secret);
        orch_db::queries::recovery_codes::management(conn, user.id, session, &factor, now)
            .await?
            .is_some()
    } else {
        false
    };
    Ok(RecoveryCodeStatus {
        remaining: status.remaining,
        created_at: status.created_at.map(Time::new),
        methods,
        recent_recovery,
    })
}

/// Replaces the saved backups only after a fresh first and second proof, in one personal transaction.
async fn recovery_codes_regenerate(
    mut ctx: Ctx,
    (): (),
    body: MeRecoveryCodesRegenerateBody,
) -> ApiResult<Answer<RecoveryCodeBatch>> {
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    if !authed.user.totp_on() {
        return Err(ApiError::invalid(
            "",
            "set up an authenticator before making recovery codes",
        ));
    }
    let proven = crate::sign_in::fresh::prove(
        &ctx,
        &authed,
        &body.proof,
        crate::sign_in::fresh::Purpose::RecoveryCodes,
    )
    .await?;
    let issued = crate::sign_in::recovery_codes::issue();
    let hashes = crate::sign_in::recovery_codes::hashes(authed.id(), &issued)
        .ok_or_else(|| ApiError::internal("a generated recovery code is malformed"))?;
    let mut tx = credential_action(&mut ctx).await?;
    proven
        .spend(
            &ctx,
            &mut tx,
            authed.id(),
            crate::sign_in::fresh::Purpose::RecoveryCodes,
        )
        .await?;
    orch_db::queries::recovery_codes::replace(&mut tx, authed.id(), &hashes, ctx.now).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(authed.id()),
        orch_db::audit::action!(AuditAction::AuthRecoveryCodesReplaced),
        Some(authed.id()),
        "replaced authenticator recovery codes",
        json!({ "count": issued.len() }),
    )
    .await?;
    let status = recovery_status(&mut tx, &authed.user, current_session(&authed), ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Answer::from(RecoveryCodeBatch {
        codes: issued,
        status,
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// The work of a person that waited for their second step is looked at again now that one is set up: what
/// may go is released by its thread's ordinary pre-flight, and nothing is started by anything else.
async fn second_step_set_up(ctx: &Ctx, user: Id) {
    if let Err(error) = ctx.app.engine.reevaluate_second_step(Some(user)).await {
        tracing::warn!(%error, "the work that waited for a second step could not be looked at again");
    }
}

/// Switches the authenticator app off with the password and an app or saved recovery code.
async fn totp_disable(mut ctx: Ctx, (): (), body: MeTotpDisableBody) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    confirm_password(&ctx, &authed, &body.password).await?;
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let user = identity::user_by_id(&mut conn, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !user.totp_on() {
        return Err(ApiError::invalid("code", "the authenticator is not on"));
    }
    let who = caller_who(&authed);
    let (step, recovery) = if crate::sign_in::recovery_codes::normalize(&body.code).is_some() {
        (
            None,
            Some(
                proofs::recovery_right(&ctx, &mut conn, &limits, &who, &user, &body.code, false)
                    .await?
                    .ok_or_else(second_step_invalid)?,
            ),
        )
    } else {
        (
            Some(
                proofs::authenticator_right(
                    &ctx, &mut conn, &limits, &who, &user, &body.code, false,
                )
                .await?
                .ok_or_else(second_step_invalid)?,
            ),
            None,
        )
    };
    drop(conn);
    let mut tx = credential_action(&mut ctx).await?;
    if let Some(step) = step
        && !identity::accept_totp_step(&mut tx, user.id, step).await?
    {
        return Err(second_step_invalid());
    }
    if let Some(hash) = recovery
        && !orch_db::queries::recovery_codes::spend(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(second_step_invalid());
    }
    identity::disable_totp(&mut tx, user.id, ctx.now).await?;
    // Without it an org that requires a second step may wait for its setup again.
    access::bump_epoch(&mut tx).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthTotpDisabled),
        Some(user.id),
        &format!("{} switched off the authenticator", user.username),
        json!({}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Mails a code to the confirmed address, to make it a second step (with the
/// password, or for a person who has none the one-use proof of their identity
/// provider), or to switch it off; and, with neither, for this session to give
/// its second step where a mailed code is one already. It answers
/// `403 auth.email_unverified` where the person has no confirmed address, or
/// the system no mail server to send it with.
async fn code_send(mut ctx: Ctx, (): (), body: MeEmailCodeSendBody) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    let (password, identity_proof) = (filled(&body.password), filled(&body.identity_proof));
    // A code for giving a step the person has needs no other proof: it goes to their own
    // confirmed address, at the pace of every code.
    let proof = if password.is_none() && identity_proof.is_none() && authed.user.email_code_on() {
        None
    } else {
        setup_proof(&ctx, &authed, password, identity_proof).await?
    };
    let limits = Limits::of(&ctx.app.config.auth);
    let mut tx = credential_action(&mut ctx).await?;
    spend_setup_proof(&ctx, &mut tx, &authed, proof.as_deref()).await?;
    let user = identity::user_by_id(&mut tx, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let policy = system_policy(&mut tx).await?;
    let Some(email) = user
        .email
        .as_deref()
        .filter(|_| user.email_confirmed() && policy.mail())
    else {
        return Err(ApiError::new(
            ErrorCode::AuthEmailUnverified,
            "a code goes to a confirmed address, by a mail server",
        ));
    };
    // The same pace as a code asked for before signing in.
    let who = Who::named(Some(user.id), "");
    if let Reservation::Refused { retry_after } =
        identity::reserve_try(&mut tx, &who.bucket("send"), ctx.now, &limits.send).await?
    {
        return Err(rate_limited(retry_after));
    }
    let to = Addressee {
        user_id: user.id,
        email,
        locale: &user.locale,
    };
    mail::send_code(&ctx, &mut tx, policy.mail_channel, &to, true).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Makes a mailed code a second step of the person, with a code just mailed.
async fn code_enable(mut ctx: Ctx, (): (), body: MeEmailCodeEnableBody) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let user = identity::user_by_id(&mut conn, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let uses = people::email_code_uses(&mut conn, Some(&user)).await?;
    if !uses.contains(&EmailCodeUse::SecondStep) {
        return Err(ApiError::new(
            ErrorCode::AuthEmailCodeNotAllowed,
            "an org of yours does not allow a mailed code as a second step",
        ));
    }
    let who = caller_who(&authed);
    let hash = proofs::mailed_code_right(
        &ctx,
        &mut conn,
        &limits,
        &who,
        Some(&user),
        &body.code,
        false,
    )
    .await?
    .ok_or_else(second_step_invalid)?;
    let mut tx = credential_action(&mut ctx).await?;
    if !people::email_code_uses(&mut tx, Some(&user))
        .await?
        .contains(&EmailCodeUse::SecondStep)
    {
        return Err(ApiError::new(
            ErrorCode::AuthEmailCodeNotAllowed,
            "an org of yours does not allow a mailed code as a second step",
        ));
    }
    if !identity::spend_sign_in_code(&mut tx, user.id, &hash, ctx.now).await? {
        return Err(second_step_invalid());
    }
    identity::set_email_code_second_step(&mut tx, user.id, Some(ctx.now), ctx.now).await?;
    stamp_given(&ctx, &mut tx, &authed).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthEmailCodeEnabled),
        Some(user.id),
        &format!("{} made a mailed code their second step", user.username),
        json!({}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    second_step_set_up(&ctx, user.id).await;
    Ok(())
}

/// Stops a mailed code being a second step: it takes the password and a code,
/// a mailed one or one of the authenticator.
async fn code_disable(mut ctx: Ctx, (): (), body: MeEmailCodeDisableBody) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    confirm_password(&ctx, &authed, &body.password).await?;
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let user = identity::user_by_id(&mut conn, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let who = caller_who(&authed);
    // A mailed code has eight characters and an authenticator's six digits,
    // so the code says which it is; anything else is a wrong mailed code.
    let mailed = codes::normalize(&body.code).is_some() || !user.totp_on();
    let (hash, step) = if mailed {
        let hash = proofs::mailed_code_right(
            &ctx,
            &mut conn,
            &limits,
            &who,
            Some(&user),
            &body.code,
            false,
        )
        .await?
        .ok_or_else(second_step_invalid)?;
        (Some(hash), None)
    } else {
        let step =
            proofs::authenticator_right(&ctx, &mut conn, &limits, &who, &user, &body.code, false)
                .await?
                .ok_or_else(second_step_invalid)?;
        (None, Some(step))
    };
    // Failure counts were written before the transaction, so a refused proof
    // keeps its count and cannot wait on this transaction's own user lock.
    let mut tx = credential_action(&mut ctx).await?;
    if let Some(hash) = hash
        && !identity::spend_sign_in_code(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(second_step_invalid());
    }
    if let Some(step) = step
        && !identity::accept_totp_step(&mut tx, user.id, step).await?
    {
        return Err(second_step_invalid());
    }
    identity::set_email_code_second_step(&mut tx, user.id, None, ctx.now).await?;
    // Without it an org that requires a second step may wait for its setup again.
    access::bump_epoch(&mut tx).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(user.id),
        orch_db::audit::action!(AuditAction::AuthEmailCodeDisabled),
        Some(user.id),
        &format!(
            "{} stopped using a mailed code as a second step",
            user.username
        ),
        json!({}),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Gives this browser session's second proof: a code of the authenticator, or a mailed code, where
/// that is a second step of the person. Counted as a wrong try of this session when it is wrong, and
/// spent once. From then on the team orgs that require a second step stop waiting for this session;
/// another session of the person gives its own.
async fn second_step_verify(mut ctx: Ctx, (): (), body: MeSecondStepVerifyBody) -> ApiResult<Me> {
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    if [&body.totp, &body.email_code, &body.recovery_code]
        .into_iter()
        .filter(|value| filled(value).is_some())
        .count()
        > 1
    {
        return Err(ApiError::invalid("", "give one second-step code"));
    }
    let limits = Limits::of(&ctx.app.config.auth);
    let mut conn = ctx.app.db.acquire().await?;
    let user = identity::user_by_id(&mut conn, authed.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let capabilities = proofs::capabilities(&mut conn, &user).await?;
    if !capabilities.has_second() {
        return Err(ApiError::new(
            ErrorCode::AuthSecondStepSetup,
            "there is no second step to give: set one up first",
        )
        .with("path", crate::sign_in::second_step::SECURITY_PATH));
    }
    let who = caller_who(&authed);
    let (hash, step, recovery) = if let Some(code) =
        filled(&body.totp).filter(|_| capabilities.second(SignInProof::Totp))
    {
        let step = proofs::authenticator_right(&ctx, &mut conn, &limits, &who, &user, code, false)
            .await?
            .ok_or_else(second_step_invalid)?;
        (None, Some(step), None)
    } else if let Some(code) =
        filled(&body.email_code).filter(|_| capabilities.second(SignInProof::EmailCode))
    {
        let hash =
            proofs::mailed_code_right(&ctx, &mut conn, &limits, &who, Some(&user), code, false)
                .await?
                .ok_or_else(second_step_invalid)?;
        (Some(hash), None, None)
    } else if let Some(code) = filled(&body.recovery_code) {
        let hash = proofs::recovery_right(&ctx, &mut conn, &limits, &who, &user, code, false)
            .await?
            .ok_or_else(second_step_invalid)?;
        (None, None, Some(hash))
    } else {
        let methods: Vec<SignInProof> = [
            SignInProof::Totp,
            SignInProof::EmailCode,
            SignInProof::RecoveryCode,
        ]
        .into_iter()
        .filter(|method| capabilities.second(*method))
        .collect();
        return Err(crate::sign_in::login::second_step_required(&methods));
    };
    let mut tx = credential_action(&mut ctx).await?;
    let current = proofs::capabilities(&mut tx, &user).await?;
    if (step.is_some() && !current.second(SignInProof::Totp))
        || (hash.is_some() && !current.second(SignInProof::EmailCode))
        || (recovery.is_some() && !current.second(SignInProof::RecoveryCode))
    {
        return Err(second_step_invalid());
    }
    if let Some(step) = step
        && !identity::accept_totp_step(&mut tx, user.id, step).await?
    {
        return Err(second_step_invalid());
    }
    if let Some(hash) = hash
        && !identity::spend_sign_in_code(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(second_step_invalid());
    }
    let recovered = recovery.is_some();
    if let Some(hash) = recovery
        && !orch_db::queries::recovery_codes::spend(&mut tx, user.id, &hash, ctx.now).await?
    {
        return Err(second_step_invalid());
    }
    stamp_given(&ctx, &mut tx, &authed).await?;
    if recovered && let Some(session) = current_session(&authed) {
        crate::sign_in::recovery_codes::grant_management(&mut tx, &user, session, ctx.now).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    people::me(&mut conn, &user, ctx.now).await
}

/// Lists the sessions of the signed-in person, and marks the one that makes
/// this request.
async fn sessions(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<SessionInfo>> {
    let authed = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    let current = current_session(authed);
    session_infos(&mut conn, authed.id(), current, ctx.now).await
}

async fn session_infos(
    conn: &mut Conn,
    user: Id,
    current: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<SessionInfo>> {
    Ok(identity::live_sessions(conn, user, now)
        .await?
        .into_iter()
        .map(|session| SessionInfo {
            id: session.id,
            current: Some(session.id) == current,
            user_agent: session.user_agent,
            ip: session.ip,
            created_at: Time::new(session.created_at),
            last_used_at: Time::new(session.last_used_at),
            expires_at: Time::new(session.expires_at),
        })
        .collect())
}

/// Revocation is one personal act, with one reply describing the sessions that remain.
async fn session_others_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<SessionInfo>> {
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    let keep = current_session(&authed).ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthSessionRequired,
            "use a browser sign-in to end its other sessions",
        )
    })?;
    let mut tx = credential_action(&mut ctx).await?;
    let ended = identity::revoke_sessions_of(&mut tx, authed.id(), Some(keep), ctx.now).await?;
    if ended > 0 {
        record(
            &ctx,
            &mut tx,
            Actor::User(authed.id()),
            orch_db::audit::action!(AuditAction::AuthOtherSessionsRevoked),
            Some(authed.id()),
            "signed out of all other browsers",
            json!({"sessions_ended":ended,"kept_session":keep}),
        )
        .await?;
    }
    let sessions = session_infos(&mut tx, authed.id(), Some(keep), ctx.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(sessions)
}

/// Ends one of the sessions of the signed-in person. A session that is not
/// theirs is none.
async fn session_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    let id = ctx.id("id")?;
    let mut tx = credential_action(&mut ctx).await?;
    if !identity::revoke_session(&mut tx, id, authed.id(), ctx.now).await? {
        return Err(ApiError::not_found());
    }
    record(
        &ctx,
        &mut tx,
        Actor::User(authed.id()),
        orch_db::audit::action!(AuditAction::AuthSessionRevoked),
        Some(authed.id()),
        &format!("{} ended a session", authed.user.username),
        json!({ "session": id, "this_one": current_session(&authed) == Some(id) }),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// An API token as it is listed.
fn token_info(row: ApiTokenListed) -> ApiTokenInfo {
    let node = row.node_id.map(|id| NodeRef {
        thread_display: None,
        id,
        kind: row
            .node_kind
            .as_deref()
            .and_then(|kind| serde_json::from_value::<NodeKind>(json!(kind)).ok())
            .unwrap_or(NodeKind::Org),
        name: row.node_name.clone(),
        redacted: false,
    });
    ApiTokenInfo {
        id: row.id,
        name: row.name,
        scopes: row.scopes,
        node,
        created_at: Time::new(row.created_at),
        last_used_at: row.last_used_at.map(Time::new),
        expires_at: row.expires_at.map(Time::new),
    }
}

/// Lists the API tokens of the signed-in person: never the secrets.
async fn tokens(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<ApiTokenInfo>> {
    let authed = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    Ok(identity::api_tokens_of(&mut conn, authed.id())
        .await?
        .into_iter()
        .map(token_info)
        .collect())
}

/// Whether the person may see the node, which is what it takes to keep a
/// token to it. A node they may not see is none to them.
async fn may_see(ctx: &Ctx, conn: &mut Conn, user: Id, node: Id) -> ApiResult<bool> {
    let principal = Principal::load(conn, Caller::session(user), Time::new(ctx.now))
        .await
        .map_err(ApiError::internal)?;
    let name = PermissionName::OfNode(NodePermission::View);
    match nodes::check_named(conn, &principal, node, name, Act::Use, Policy::default()).await {
        Ok(Decision::Allowed { .. }) | Err(orch_authz::Error::Archived { .. }) => Ok(true),
        Ok(Decision::Refused(_))
        | Err(orch_authz::Error::NotFound(_) | orch_authz::Error::Suspended { .. }) => Ok(false),
        Err(error) => Err(ApiError::internal(error)),
    }
}

/// Makes an API token, narrow (no scopes, none of the person's permissions) and with the maker's fresh
/// proof in this request. The secret is shown here and never again: only its hash is kept, and the same
/// creation asked again by its key answers what was made, without it.
async fn token_create(
    mut ctx: Ctx,
    (): (),
    body: ApiTokenCreateBody,
) -> ApiResult<Answer<TokenCreated>> {
    use super::tokens;
    use crate::sign_in::fresh::{self, Purpose};
    let authed = Arc::clone(ctx.user()?);
    require_browser(&authed)?;
    {
        let mut tx = credential_action(&mut ctx).await?;
        let again = tokens::replayed(&ctx, &mut tx, authed.id()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if let Some(again) = again {
            return Ok(Answer::from(again).header(CACHE_CONTROL, NOT_KEPT));
        }
    }
    let asked = tokens::asked(&body, ctx.now)?;
    let proven = fresh::prove(&ctx, &authed, &body.proof, Purpose::TokenCreate).await?;

    let mut tx = credential_action(&mut ctx).await?;
    if let Some(node) = body.node_id
        && !may_see(&ctx, &mut tx, authed.id(), node).await?
    {
        return Err(ApiError::not_found());
    }
    let place = body
        .node_id
        .map_or(orch_settings::Place::System, orch_settings::Place::Node);
    tokens::within_lifetime(&mut tx, place, ctx.now, asked.expires_at).await?;
    proven
        .spend(&ctx, &mut tx, authed.id(), Purpose::TokenCreate)
        .await?;
    let id = orch_types::new_id();
    let secret = new_token(API_TOKEN_PREFIX);
    let name = asked.name.as_str();
    identity::create_api_token(
        &mut tx,
        &NewApiToken {
            id,
            user_id: authed.id(),
            name,
            token_hash: &token_hash(&secret),
            scopes: &asked.scopes,
            node_id: body.node_id,
            at: ctx.now,
            expires_at: asked.expires_at,
        },
    )
    .await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(authed.id()),
        orch_db::audit::action!(AuditAction::AuthTokenCreated),
        Some(authed.id()),
        &format!("{} made the API token {name}", authed.user.username),
        json!({ "token": id, "name": name, "scopes": asked.scopes, "node_id": body.node_id,
                "expires_at": body.expires_at }),
    )
    .await?;
    let info = identity::api_token_listed(&mut tx, id, authed.id())
        .await?
        .map(token_info)
        .ok_or_else(|| ApiError::internal("a token that was just made is not there"))?;
    tokens::keep(&ctx, &mut tx, authed.id(), &info).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Answer::from(TokenCreated {
        token: Some(secret),
        info,
        already_created: false,
    })
    .header(CACHE_CONTROL, NOT_KEPT))
}

/// Revokes an API token of the signed-in person.
async fn token_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let authed = Arc::clone(ctx.user()?);
    let id = ctx.id("id")?;
    let mut tx = credential_action(&mut ctx).await?;
    if !identity::revoke_api_token(&mut tx, id, authed.id(), ctx.now).await? {
        return Err(ApiError::not_found());
    }
    // What was decided under the token's reach is decided again.
    access::bump_epoch(&mut tx).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(authed.id()),
        orch_db::audit::action!(AuditAction::AuthTokenRevoked),
        Some(authed.id()),
        &format!("{} revoked an API token", authed.user.username),
        json!({ "token": id }),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
