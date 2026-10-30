//! What an identity a provider vouched for leads to: a person signed in, a person made and signed in, the identity linked to the person who asked, or a one-use proof for one sensitive act.
//!
//! The rules are the identity contract's ([`decide`]): a provider that is switched off lets nobody
//! through; its list of domains is looked at on every sign-in; an identity finds its user by the
//! provider and the subject alone; an identity that is not linked, carrying the confirmed address of
//! an existing user, is refused with `link_required` and joins nobody; and a new person is made only
//! where the provider registers people and the system's `registration.mode` lets them in (an
//! invitation that is still good, and for a personal link the address it was sent to, satisfies
//! `invite`).
//!
//! A person made here has an org of their own as anybody who registers, no password, the address the
//! provider confirmed (where no other person holds it), their device's time zone once their browser
//! reports it, the identity linked, and, where the provider names one, a place in its org with its
//! role. Their first browser is signed in. What was done is in the audit, each in the transaction that
//! did it.

use super::{
    LINK_ERROR_FRAGMENT, NO_SESSION, NOT_FRESH, PROOF_ERROR_FRAGMENT, PROOF_FRAGMENT,
    PROOF_LIFETIME, PROOF_PREFIX, redirect, settings_of, with_cookies, without_fragment,
};
use crate::middleware::authentication::{new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::placed;
use crate::sign_in::accounts::{locale_of, taken};
use crate::sign_in::second_step::SECURITY_PATH;
use crate::sign_in::{open_session, record, system_policy, throttle};
use axum::response::Response;
use http::{HeaderValue, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages::StoredKind;
use orch_api_contract::auth::RegistrationMode as SystemMode;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeKind;
use orch_authz::epoch;
use orch_db::audit::Actor;
use orch_db::queries::access;
use orch_db::queries::identity::{self, IdentityLink, IdentityProviderRow, SignInState, UserRow};
use orch_db::queries::tokens::{self, NewSensitiveProof};
use orch_db::{Conn, DbError};
use orch_identity_contract::claims::{
    self, Account, Decision, Flow, Lookup, RegistrationMode, SignInError, USERNAME_MAX_CHARS,
    VerifiedIdentity, decide, username_base,
};
use orch_types::{Id, Time, new_id};
use serde_json::json;
use std::collections::HashSet;

/// The role a person made through a provider holds in the provider's org where it names none: Viewer,
/// what an invitation into an org gives when it names none.
const DEFAULT_ROLE: &str = "viewer";
/// The most characters a number added to a username takes: the names tried go up to a thousand.
const SUFFIX_MAX_CHARS: usize = 4;

/// The system's registration mode, as the rules read it.
fn mode_of(mode: SystemMode) -> RegistrationMode {
    match mode {
        SystemMode::Closed => RegistrationMode::Closed,
        SystemMode::Invite => RegistrationMode::Invite,
        SystemMode::Open => RegistrationMode::Open,
    }
}

/// What the database says of an identity: the user it is linked to, and, where it is linked to
/// nobody and carries a confirmed address, the user whose confirmed address that is.
async fn lookup(conn: &mut Conn, provider: Id, identity: &VerifiedIdentity) -> ApiResult<Lookup> {
    let linked = identity::identity_account(conn, provider, &identity.subject)
        .await?
        .map(|(user, suspended)| Account { user, suspended });
    let same_email = match (linked, identity.trusted_email()) {
        (None, Some(email)) => identity::user_by_confirmed_email(conn, email)
            .await?
            .map(|user| user.id),
        _ => None,
    };
    Ok(Lookup { linked, same_email })
}

/// Whether the invitation `token` is still good for the person an identity is: a shared link is for
/// whoever holds it; a personal link is for the address it was sent to, which the provider has to
/// have confirmed.
async fn invitation_fits(
    conn: &mut Conn,
    token: &str,
    accept: bool,
    identity: &VerifiedIdentity,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<bool> {
    let Some(found) =
        crate::routes::users::registration_offer(conn, Some(token), accept, now).await?
    else {
        return Ok(false);
    };
    if found.email.as_deref().is_some_and(|bound| {
        identity
            .trusted_email()
            .is_none_or(|email| email.trim().to_lowercase() != bound.trim().to_lowercase())
    }) {
        return Err(ApiError::new(
            ErrorCode::RegistrationInviteRequired,
            "the invitation is for the verified address it was sent to",
        ));
    }
    Ok(true)
}

/// Signs the person an identity is in: the user it is linked to, or a user made from it where that is
/// allowed. The browser goes to `back` with its session, or to the sign-in page with the code of why
/// not.
pub(super) async fn sign_in(
    ctx: &Ctx,
    provider: &IdentityProviderRow,
    identity: &VerifiedIdentity,
    invitation: Option<&str>,
    accept_invitation: bool,
    back: &str,
) -> ApiResult<Response> {
    let scope = if provider.auto_register || accept_invitation {
        orch_db::queries::transaction_guard::Scope::Creation
    } else {
        orch_db::queries::transaction_guard::Scope::Personal { user: None }
    };
    orch_db::queries::transaction_guard::request_scope(
        scope,
        sign_in_inner(ctx, provider, identity, invitation, accept_invitation, back),
    )
    .await
}

async fn sign_in_inner(
    ctx: &Ctx,
    provider: &IdentityProviderRow,
    identity: &VerifiedIdentity,
    invitation: Option<&str>,
    accept_invitation: bool,
    back: &str,
) -> ApiResult<Response> {
    let refused = |error: SignInError| Ok(redirect(&error.login_path()));
    // An anonymous start has no account to count. Count only the verified
    // provider's subject, or the user it is already linked to, before the
    // landing transaction: rolling back a refusal must not erase the limit.
    {
        let mut conn = ctx.app.db.acquire().await?;
        let account = identity::identity_account(&mut conn, provider.id, &identity.subject).await?;
        let who = throttle::Who::named(
            account.map(|(user, _)| user),
            &format!("{}:{}", provider.id, identity.subject),
        );
        if let Err(error) = throttle::identity_account(ctx, &mut conn, &who).await {
            if error.code() != ErrorCode::RateLimited {
                return Err(error);
            }
            let mut response = redirect(&SignInError::IdpError.login_path());
            if let Some(seconds) = error.retry_after {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
            }
            return Ok(response);
        }
    }
    let mut tx = ctx.app.db.begin().await?;
    let Some(current_provider) = identity::identity_provider(&mut tx, provider.id).await? else {
        return refused(SignInError::Disabled);
    };
    if current_provider.revision != provider.revision {
        return refused(SignInError::StateExpired);
    }
    let provider = &current_provider;
    let mut current_ctx = ctx.clone();
    current_ctx.now = ctx.app.clock.now();
    let ctx = &current_ctx;
    let policy = system_policy(&mut tx).await?;
    let good_invitation = match invitation {
        Some(token) => {
            match invitation_fits(&mut tx, token, accept_invitation, identity, ctx.now).await {
                Ok(good) => good,
                Err(error) if error.code() == ErrorCode::RegistrationInviteRequired => {
                    return refused(SignInError::InviteRequired);
                }
                Err(error) => return Err(error),
            }
        }
        None => false,
    };
    let found = lookup(&mut tx, provider.id, identity).await?;
    let decision = match decide(
        &settings_of(provider),
        mode_of(policy.registration),
        Flow::SignIn { good_invitation },
        identity,
        &found,
    ) {
        Ok(decision) => decision,
        Err(error) => return refused(error),
    };
    let registering = matches!(&decision, Decision::Register);
    let user = match decision {
        Decision::SignIn { user } => {
            let person = orch_db::queries::transaction_guard::lock_person(&mut tx, user)
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthInvalidCredentials,
                        "this account no longer exists",
                    )
                })?;
            if person.suspended() {
                return refused(SignInError::Disabled);
            }
            if identity::identity_account(&mut tx, provider.id, &identity.subject)
                .await?
                .is_none_or(|(linked, _)| linked != user)
            {
                return refused(SignInError::StateExpired);
            }
            identity::note_identity_sign_in(
                &mut tx,
                provider.id,
                &identity.subject,
                identity.email.as_deref(),
                ctx.now,
            )
            .await?;
            person
        }
        Decision::Register => match register(ctx, &mut tx, provider, identity, !good_invitation)
            .await?
        {
            Some(user) => {
                // The verified subject was counted while it had no local
                // account. Its first sign-in also belongs to the new user's
                // shared account count, so registering grants no extra start.
                // Counted in this transaction: the count is the new person's
                // from the moment they exist, and goes with them if they do
                // not; a second pooled connection asked for while this one is
                // held can wait for ever on a busy pool.
                throttle::identity_account(ctx, &mut tx, &throttle::Who::named(Some(user.id), ""))
                    .await?;
                user
            }
            None => return refused(SignInError::IdpError),
        },
        // A sign-in never decides a link.
        Decision::Link { .. } => return refused(SignInError::IdpError),
    };
    let redeemed = if accept_invitation || registering && good_invitation {
        let Some(token) = invitation else {
            return refused(SignInError::InviteRequired);
        };
        match crate::routes::users::redeem(ctx, &mut tx, &user, token).await {
            Ok(row) => Some(row),
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::Gone | ErrorCode::RegistrationInviteRequired
                ) =>
            {
                return refused(SignInError::InviteRequired);
            }
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::AuthzDenied | ErrorCode::OrgClosed | ErrorCode::MemberExists
                ) =>
            {
                return refused(SignInError::Denied);
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let opened = open_session(
        ctx,
        &mut tx,
        &user,
        json!({ "by": "identity_provider", "identity_provider": provider.key }),
    )
    .await?;
    let destination = match redeemed {
        Some(row) if accept_invitation => {
            let reply = crate::routes::users::accepted_for_session(
                ctx,
                &mut tx,
                &user,
                opened.session,
                &row,
            )
            .await?;
            crate::routes::users::accepted_path(&reply)
        }
        // A consumed system-admission link needs no second acceptance page.
        Some(_) if back.starts_with("/invite/") => "/".to_owned(),
        _ => back.to_owned(),
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(with_cookies(redirect(&destination), &opened.cookies))
}

/// The part of `base` that every name made from it begins with: a long base is cut to make room for
/// the number added to it.
fn stem_of(base: &str) -> String {
    base.chars()
        .take(
            usize::try_from(USERNAME_MAX_CHARS)
                .unwrap_or(usize::MAX)
                .saturating_sub(SUFFIX_MAX_CHARS),
        )
        .collect()
}

/// Makes the person an identity is, as a first sign-in through a provider that registers people does
/// (see the module). `None` where no username is left to give.
async fn register(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: &IdentityProviderRow,
    identity: &VerifiedIdentity,
    enroll_default: bool,
) -> ApiResult<Option<UserRow>> {
    let taken_names: HashSet<String> =
        identity::names_taken_from(conn, &stem_of(&username_base(identity)))
            .await?
            .into_iter()
            .collect();
    let Some(new) = claims::NewUser::from_identity(identity, |name| taken_names.contains(name))
    else {
        return Ok(None);
    };
    // An address another person holds, not confirmed, is theirs to confirm or lose: the new person
    // starts without it.
    let email = match new.email.as_deref() {
        Some(email) => (!identity::email_in_use(conn, email, Id::nil()).await?).then_some(email),
        None => None,
    };
    let personal_org = crate::sign_in::accounts::personal_org_start(conn, None).await?;
    let id = new_id();
    let created = match identity::create_user(
        conn,
        &identity::NewUser {
            id,
            username: &new.username,
            display_name: &new.display_name,
            email,
            email_confirmed: email.is_some(),
            password_hash: None,
            locale: locale_of(&ctx.headers),
            is_system_admin: false,
            created_by: None,
            personal_org,
        },
        ctx.now,
    )
    .await
    {
        Ok(created) => created,
        // The chosen name was free when read, but another registration may
        // have won it before this insert. The landing rolls back and sends
        // the browser to sign-in with its ordinary failure reason.
        Err(error) if error.is_unique("users_username_key") || error.is_unique("orgs_slug_key") => {
            return Ok(None);
        }
        Err(error) => return Err(taken(error)),
    };
    // Their zone follows the device they come from, UTC until a browser of theirs reports one.
    identity::start_device_timezone(conn, id, None, ctx.now).await?;
    if let Some(org) = created.personal_org_id {
        orch_authz::pages::start(conn, StoredKind::Node, org, id, Time::new(ctx.now))
            .await
            .map_err(ApiError::internal)?;
    }
    identity::link_identity(
        conn,
        &IdentityLink {
            provider: provider.id,
            subject: &identity.subject,
            user: id,
            email: identity.email.as_deref(),
            at: ctx.now,
        },
    )
    .await?;
    let joined = if enroll_default {
        join_default_org(ctx, conn, provider, id).await?
    } else {
        None
    };
    record(
        ctx,
        conn,
        Actor::User(id),
        AuditAction::UserRegistered,
        Some(id),
        &format!("{} was registered through {}", new.username, provider.name),
        json!({
            "identity_provider": provider.key,
            "email_confirmed": email.is_some(),
            "org": joined.as_ref().map(|(org, _)| *org),
            "role": joined.as_ref().map(|(_, role)| role.clone()),
        }),
    )
    .await?;
    record(
        ctx,
        conn,
        Actor::User(id),
        orch_db::audit::action!(AuditAction::AuthIdentityLinked),
        Some(id),
        &format!(
            "{} linked a sign-in through {}",
            new.username, provider.name
        ),
        json!({ "identity_provider": provider.key, "registered": true }),
    )
    .await?;
    Ok(identity::user_by_id(conn, id).await?)
}

/// Makes a person made through `provider` a member of the org it names, at their domain there, with
/// its role; the org and the role, where it names one that still takes members. Who gave the role is
/// the first system admin, who keeps the providers, or the person where the system has none.
async fn join_default_org(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: &IdentityProviderRow,
    user: Id,
) -> ApiResult<Option<(Id, String)>> {
    let Some(org) = provider.default_org_id else {
        return Ok(None);
    };
    let Ok((place, kind)) = placed(conn, org).await else {
        tracing::warn!(provider = %provider.key, %org, "the org of an identity provider is gone; the person is made without it");
        return Ok(None);
    };
    let closed = orch_authz::orgs::limits(conn, place.org)
        .await
        .map_err(ApiError::internal)?
        .is_some_and(|limits| limits.closed);
    if kind != NodeKind::Org || closed {
        tracing::warn!(provider = %provider.key, %org, "the org of an identity provider takes no member; the person is made without it");
        return Ok(None);
    }
    access::add_member(conn, place.org, user, org, None, ctx.now).await?;
    let role = provider
        .default_role
        .clone()
        .unwrap_or_else(|| DEFAULT_ROLE.to_owned());
    let by = identity::first_system_admin(conn).await?.unwrap_or(user);
    access::put_role(
        conn,
        &access::NewRole {
            id: new_id(),
            node: org,
            subject_kind: "user",
            subject: user,
            subject_role: None,
            role: &role,
            expires_at: None,
            note: "",
            by,
            at: ctx.now,
        },
    )
    .await?;
    epoch::bump(conn).await.map_err(ApiError::internal)?;
    Ok(Some((org, role)))
}

/// Links an identity to the person who asked for it from their Security page, which the browser goes
/// back to; with the code of why not in its fragment where it is refused.
pub(super) async fn link(
    ctx: &Ctx,
    provider: &IdentityProviderRow,
    identity: &VerifiedIdentity,
    user: Id,
) -> ApiResult<Response> {
    let refused = |error: SignInError| {
        Ok(redirect(&format!(
            "{SECURITY_PATH}#{LINK_ERROR_FRAGMENT}={}",
            error.code()
        )))
    };
    let mut tx = ctx.app.db.begin().await?;
    let Some(person) = orch_db::queries::transaction_guard::lock_person(&mut tx, user)
        .await?
        .filter(|person| person.kind == "human")
    else {
        return refused(SignInError::StateExpired);
    };
    if person.suspended() {
        return refused(SignInError::Disabled);
    }
    let Some(current_provider) = identity::identity_provider(&mut tx, provider.id).await? else {
        return refused(SignInError::Disabled);
    };
    if current_provider.revision != provider.revision {
        return refused(SignInError::StateExpired);
    }
    let provider = &current_provider;
    let linked = identity::identity_account(&mut tx, provider.id, &identity.subject)
        .await?
        .map(|(user, suspended)| Account { user, suspended });
    // The mode is not asked of a link: the person exists.
    if let Err(error) = decide(
        &settings_of(provider),
        RegistrationMode::Closed,
        Flow::Link { user },
        identity,
        &Lookup {
            linked,
            same_email: None,
        },
    ) {
        return refused(error);
    }
    // One identity at a provider for one person: another subject there is unlinked first.
    if identity::identity_at(&mut tx, provider.id, user)
        .await?
        .is_some_and(|subject| subject != identity.subject)
    {
        return refused(SignInError::Denied);
    }
    let added = identity::link_identity(
        &mut tx,
        &IdentityLink {
            provider: provider.id,
            subject: &identity.subject,
            user,
            email: identity.email.as_deref(),
            at: ctx.now,
        },
    )
    .await?;
    if added {
        record(
            ctx,
            &mut tx,
            Actor::User(user),
            orch_db::audit::action!(AuditAction::AuthIdentityLinked),
            Some(user),
            &format!(
                "{} linked a sign-in through {}",
                person.username, provider.name
            ),
            json!({ "identity_provider": provider.key }),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(redirect(SECURITY_PATH))
}

/// Gives the browser session that asked for it a one-use proof of its person, for one act: where the
/// identity is the one linked to that person, the provider is switched on, and the provider says it
/// signed them in at or after the start (both in whole UTC seconds; a provider that says nothing of
/// when proves nothing fresh). Only the proof's hash is kept, for five minutes, bound to the session,
/// the act and the service identity a token is for. It opens no session.
pub(super) async fn proof(
    ctx: &Ctx,
    provider: &IdentityProviderRow,
    identity: &VerifiedIdentity,
    row: &SignInState,
    session: Id,
    act: &str,
) -> ApiResult<Response> {
    let back = without_fragment(&row.redirect_path);
    let refused = |why: &str| Ok(redirect(&format!("{back}#{PROOF_ERROR_FRAGMENT}={why}")));
    if !provider.enabled {
        return refused(SignInError::Disabled.code());
    }
    let mut tx = ctx.app.db.begin().await?;
    let Some(person) = tokens::live_session_user(&mut tx, session, ctx.now).await? else {
        return refused(NO_SESSION);
    };
    let Some(current) = orch_db::queries::transaction_guard::lock_person(&mut tx, person).await?
    else {
        return refused(NO_SESSION);
    };
    let now = ctx.app.clock.now();
    if current.suspended()
        || tokens::live_session_user(&mut tx, session, now).await? != Some(person)
    {
        return refused(NO_SESSION);
    }
    let Some(current_provider) = identity::identity_provider(&mut tx, provider.id).await? else {
        return refused(SignInError::Disabled.code());
    };
    if !current_provider.enabled || current_provider.revision != provider.revision {
        return refused(SignInError::StateExpired.code());
    }
    // The identity has to be the one linked to the session's person: another's proves nothing of them.
    if tokens::identity_owner(&mut tx, provider.id, &identity.subject).await? != Some(person) {
        return refused(SignInError::Denied.code());
    }
    let fresh = identity
        .auth_time
        .is_some_and(|at| at.get().timestamp() >= row.started_at.timestamp());
    if !fresh {
        return refused(NOT_FRESH);
    }
    let proof = new_token(PROOF_PREFIX);
    tokens::keep_sensitive_proof(
        &mut tx,
        &NewSensitiveProof {
            proof_hash: &token_hash(&proof),
            session,
            purpose: act,
            target: row.service_account_id,
            verified_at: now,
            expires_at: now + PROOF_LIFETIME,
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(redirect(&format!("{back}#{PROOF_FRAGMENT}={proof}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_base_is_cut_to_where_every_numbered_name_begins() {
        assert_eq!(stem_of("dana"), "dana");
        let long = "a".repeat(32);
        assert_eq!(stem_of(&long).len(), 28);
        let names = [
            format!("{}2", "a".repeat(31)),
            format!("{}1000", "a".repeat(28)),
        ];
        for name in names {
            assert!(name.starts_with(&stem_of(&long)), "{name}");
        }
    }

    #[test]
    fn the_registration_mode_of_the_system_is_the_one_the_rules_read() {
        assert_eq!(mode_of(SystemMode::Closed), RegistrationMode::Closed);
        assert_eq!(mode_of(SystemMode::Invite), RegistrationMode::Invite);
        assert_eq!(mode_of(SystemMode::Open), RegistrationMode::Open);
    }
}
