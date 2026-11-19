//! A fresh proof for a sensitive act: the caller's password, or the one-use proof of their identity provider where they have none, with their second step where they have one, all in the request that does the act.
//!
//! Neither the session that sends the request nor a remembered sign-in stands
//! for the proof: the act asks for it again. A person who has a password gives
//! it, and cannot give an identity provider's proof instead; a person without
//! one gives the one-use proof their identity provider made for this act and
//! this session (five minutes, spent once). Where they have a second step, one
//! of its codes is given beside it. Each proof is counted as a wrong try of
//! this session when it is wrong, as at signing in, and what is spent (a mailed
//! code, an authenticator's step, the one-use proof) is spent in the act's own
//! transaction, so that an act that fails spends nothing.

use super::throttle::Who;
use super::{Limits, proofs};
use crate::middleware::authentication::{Authed, Via, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::auth::{SensitiveProof, SignInProof};
use orch_api_contract::common::ErrorCode;
use orch_db::Conn;
use orch_db::queries::{identity, tokens};
use orch_types::Id;

/// What a fresh proof is for: a proof made for one act cannot be spent on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Making an API token, personal or for a service identity.
    TokenCreate,
    /// Setting up a second step without a password.
    SecondStepSetup,
    /// Replacing an authenticator's saved one-use recovery codes.
    RecoveryCodes,
}

impl Purpose {
    /// The word it is kept by.
    #[must_use]
    pub fn stored(self) -> &'static str {
        match self {
            Self::TokenCreate => "token_create",
            Self::SecondStepSetup => "second_step_setup",
            Self::RecoveryCodes => "recovery_codes",
        }
    }
}

/// A proof that was right, with what it spends when the act is done.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proven {
    session: Option<Id>,
    key_authenticated: bool,
    code_to_spend: Option<Vec<u8>>,
    step_to_accept: Option<i64>,
    recovery_to_spend: Option<Vec<u8>>,
    recovery_management: Option<(Vec<u8>, Vec<u8>)>,
    identity_proof: Option<Vec<u8>>,
    target: Option<Id>,
}

/// Whether this account is authenticated by the current operation's explicit key
/// admission. This chooses the existing proof mechanism; it grants no permission.
/// The shared registry installs the original caller only after actual operation
/// admission, and the action guard still rechecks it before any effect.
///
/// Every retained legacy ceiling keeps its historical password/browser behavior.
/// Checked-empty operations retain their exact selection and declared action/resource
/// evidence from the shared builder; an empty facet list alone proves nothing.
pub(crate) fn key_proves_account(ctx: &Ctx, authed: &Authed) -> ApiResult<bool> {
    if authed.is_session() {
        return Ok(false);
    }
    match ctx.admitted_caller() {
        Some(caller) => key_proves_account_as(ctx, authed, caller),
        None => {
            let caller = crate::middleware::authorization::caller_of(authed)?;
            if caller
                .grant()
                .is_none_or(|grant| grant.requires_legacy_compatibility())
            {
                Ok(false)
            } else {
                Err(key_admission_required())
            }
        }
    }
}

fn key_admission_required() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "this operation needs the key's current scope admission",
    )
}

/// The same proof mechanism at the actual read/action checkpoint, including any
/// retained legacy ceiling acquired while the request waited. No proof-mode flag
/// is accepted from the wire, and a prepared caller never replaces this current one.
pub(crate) fn key_proves_account_as(
    ctx: &Ctx,
    authed: &Authed,
    admitted: &orch_authz::Caller,
) -> ApiResult<bool> {
    let Via::Token { token_id, .. } = &authed.via else {
        return Ok(false);
    };
    let authenticated = crate::middleware::authorization::caller_of(authed)?;
    let Some(current) = authenticated.grant() else {
        return Ok(false);
    };
    let grant = admitted.grant().ok_or_else(key_admission_required)?;
    if current.requires_legacy_compatibility() || grant.requires_legacy_compatibility() {
        return Ok(false);
    }
    let selection = admitted
        .admitted_operation()
        .ok_or_else(key_admission_required)?;
    let operation =
        orch_api_contract::operations::for_route(&ctx.route).map_err(ApiError::internal)?;
    let selected = operation
        .variants
        .iter()
        .any(|variant| variant.operation == selection);
    let facets = admitted
        .admitted_facets()
        .ok_or_else(key_admission_required)?;
    let checked_empty = admitted
        .admitted_empty_facets()
        .is_some_and(|required| !required.is_empty());
    if token_id.is_nil()
        || admitted.user() != authed.id()
        || admitted.as_admin()
        || !grant.token_ids().any(|id| id == *token_id)
        || !selected
        || operation.delegation != orch_api_contract::operations::Delegation::Business
        || (facets.is_empty() && !checked_empty)
    {
        return Err(key_admission_required());
    }
    Ok(true)
}

/// Looks at the fresh proof `proof` given by `authed` for `purpose`. Nothing is spent here: the act
/// spends it with [`Proven::spend`] in its own transaction.
pub async fn prove(
    ctx: &Ctx,
    authed: &Authed,
    proof: &SensitiveProof,
    purpose: Purpose,
) -> ApiResult<Proven> {
    if key_proves_account(ctx, authed)? {
        return Ok(Proven {
            key_authenticated: true,
            ..Proven::default()
        });
    }
    let Via::Session { session_id } = authed.via else {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "this is done from a browser sign-in",
        ));
    };
    let user = &authed.user;
    let limits = Limits::of(&ctx.app.config.auth);
    let who = Who::caller('s', session_id, user.id);
    let mut conn = ctx.app.db.acquire().await?;
    let given = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    if [&proof.totp, &proof.email_code, &proof.recovery_code]
        .into_iter()
        .filter(|value| given(value).is_some())
        .count()
        > 1
    {
        return Err(ApiError::invalid("proof", "give one second-step code"));
    }
    let mut proven = Proven {
        session: Some(session_id),
        ..Proven::default()
    };

    // The first proof: the password where the person has one, else their identity provider's.
    if user.password_hash.is_some() {
        let Some(password) = given(&proof.password) else {
            return Err(reauthenticate(purpose, "password"));
        };
        if !proofs::password_right(ctx, &mut conn, &limits, &who, Some(user), &password, false)
            .await?
        {
            return Err(super::login::invalid_credentials());
        }
    } else {
        let Some(identity_proof) = given(&proof.identity_proof) else {
            return Err(reauthenticate(purpose, "identity_proof"));
        };
        proven.identity_proof = Some(token_hash(&identity_proof));
    }

    // The second step, where the person has one: one of its codes beside the first proof.
    let capabilities = proofs::capabilities(&mut conn, user).await?;
    if capabilities.has_second() {
        if let Some(code) = given(&proof.totp).filter(|_| capabilities.second(SignInProof::Totp)) {
            proven.step_to_accept = Some(
                proofs::authenticator_right(ctx, &mut conn, &limits, &who, user, &code, false)
                    .await?
                    .ok_or_else(super::login::second_step_invalid)?,
            );
        } else if let Some(code) =
            given(&proof.email_code).filter(|_| capabilities.second(SignInProof::EmailCode))
        {
            proven.code_to_spend = Some(
                proofs::mailed_code_right(ctx, &mut conn, &limits, &who, Some(user), &code, false)
                    .await?
                    .ok_or_else(super::login::second_step_invalid)?,
            );
        } else if let Some(code) = given(&proof.recovery_code) {
            proven.recovery_to_spend = Some(
                proofs::recovery_right(ctx, &mut conn, &limits, &who, user, &code, false)
                    .await?
                    .ok_or_else(super::login::second_step_invalid)?,
            );
        } else if purpose == Purpose::RecoveryCodes && user.totp_on() {
            let secret = user
                .totp_secret
                .as_deref()
                .ok_or_else(super::login::second_step_invalid)?;
            let factor = super::recovery_codes::factor_hash(user.id, secret);
            let Some(reference) = orch_db::queries::recovery_codes::management(
                &mut conn, user.id, session_id, &factor, ctx.now,
            )
            .await?
            else {
                let methods: Vec<_> = [
                    SignInProof::Totp,
                    SignInProof::EmailCode,
                    SignInProof::RecoveryCode,
                ]
                .into_iter()
                .filter(|method| capabilities.second(*method))
                .collect();
                return Err(super::login::second_step_required(&methods));
            };
            proven.recovery_management = Some((factor, reference));
        } else {
            let methods: Vec<SignInProof> = [
                SignInProof::Totp,
                SignInProof::EmailCode,
                SignInProof::RecoveryCode,
            ]
            .into_iter()
            .filter(|method| capabilities.second(*method))
            .collect();
            return Err(super::login::second_step_required(&methods));
        }
    }
    Ok(proven)
}

impl Proven {
    /// Whether a shared key admission substituted for caller reauthentication.
    /// The credential producer rechecks this against its current action guard.
    pub(crate) fn key_authenticated(&self) -> bool {
        self.key_authenticated
    }

    /// The same proof, for an act on the service identity `target`: an identity provider's proof made for
    /// another identity's token, or for the person's own, is not spent here.
    #[must_use]
    pub fn for_target(mut self, target: Option<Id>) -> Self {
        self.target = target;
        self
    }

    /// Spends what the proof used, in the act's transaction: the one-use proof of the identity provider,
    /// a mailed code, an authenticator's step. A proof that was spent meanwhile refuses the act.
    pub async fn spend(
        &self,
        ctx: &Ctx,
        tx: &mut Conn,
        user: Id,
        purpose: Purpose,
    ) -> ApiResult<()> {
        if let (Some(hash), Some(session)) = (&self.identity_proof, self.session)
            && !tokens::spend_sensitive_proof(
                tx,
                hash,
                session,
                purpose.stored(),
                self.target,
                ctx.now,
            )
            .await?
        {
            return Err(reauthenticate(purpose, "identity_proof"));
        }
        if let Some(hash) = &self.code_to_spend
            && !identity::spend_sign_in_code(tx, user, hash, ctx.now).await?
        {
            return Err(super::login::second_step_invalid());
        }
        if let Some(step) = self.step_to_accept
            && !identity::accept_totp_step(tx, user, step).await?
        {
            return Err(super::login::second_step_invalid());
        }
        if let Some(hash) = &self.recovery_to_spend
            && !orch_db::queries::recovery_codes::spend(tx, user, hash, ctx.now).await?
        {
            return Err(super::login::second_step_invalid());
        }
        if let Some((factor, reference)) = &self.recovery_management {
            let session = self.session.ok_or_else(super::login::second_step_invalid)?;
            if purpose != Purpose::RecoveryCodes
                || !orch_db::queries::recovery_codes::spend_management(
                    tx, user, session, factor, reference, ctx.now,
                )
                .await?
            {
                return Err(super::login::second_step_invalid());
            }
        }
        if self.recovery_to_spend.is_some()
            && purpose != Purpose::RecoveryCodes
            && let Some(session) = self.session
            && let Some(current) = identity::user_by_id(tx, user).await?
        {
            super::recovery_codes::grant_management(tx, &current, session, ctx.now).await?;
        }
        // A second proof given here is a second proof of this session as well.
        if (self.code_to_spend.is_some()
            || self.step_to_accept.is_some()
            || self.recovery_to_spend.is_some())
            && let Some(session) = self.session
        {
            tokens::stamp_second_step(tx, session, ctx.now).await?;
        }
        Ok(())
    }
}

/// `403 auth.reauthentication_required`: the act lacks a fresh password or a valid proof of the
/// session's identity provider, named in `available`.
pub(crate) fn reauthenticate(purpose: Purpose, available: &str) -> ApiError {
    ApiError::new(
        ErrorCode::AuthReauthenticationRequired,
        "prove who you are again for this, in this request",
    )
    .with("purpose", purpose.stored())
    .with("available", available)
}
