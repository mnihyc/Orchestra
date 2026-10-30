//! Signing in through an identity provider: the start and the callback of a sign-in, of a link to the signed-in person and of a fresh proof for one sensitive act, the start of a link and the unlinking of an identity, and the kinds that speak to providers.
//!
//! Every sign-in through a provider goes the same way, whatever it is for. Its start keeps a row of
//! `oidc_states` for ten minutes, made by the kind ([`IdentityProviderKind::start`]): its state, nonce
//! and PKCE verifier, where the browser goes back to, and what the sign-in is for:
//!
//! - **a sign-in** (`GET /auth/oidc/{key}/start`), which may make a user where the provider registers
//!   people, under the system's `registration.mode` (an invitation that is still good satisfies
//!   `invite`);
//! - **a link** of an identity to the person who is signed in (`POST /me/identities/{key}/link`, from a
//!   browser sign-in alone, as a password is changed), which comes back to the Security page;
//! - **a fresh proof** of a signed-in person who has no password, for one sensitive act
//!   (`sensitive_purpose`), bound to the browser session whose cookie the start carried.
//!
//! **Only the browser that began a sign-in may end it.** The start gives the browser a mark, the
//! cookie `orch_oidc` (`SameSite=Lax`, so that the provider's redirect back carries it; for ten
//! minutes; for the requests of this module alone), and keeps its hash with the sign-in. A callback
//! that comes in another browser, as when somebody sends a person the address of a sign-in they began
//! themself, ends nothing: the state alone ties an answer to no browser, and without the mark a
//! stranger's identity could be signed in, or linked, in somebody else's browser.
//!
//! The callback takes the row once, whatever comes of it, and asks the kind to complete the sign-in:
//! the code is exchanged and the ID token checked there (signature, issuer, audience, times and nonce,
//! and the PKCE verifier at the provider). What the identity then leads to is [`landing`]'s. A sign-in
//! that ends without a session goes back to the sign-in page with one of the eight codes
//! (`/login?error=<code>`); a link that does not, to the Security page with the code in the fragment
//! (`#identity_link_error=<code>`); a proof that does not, to its form (`#identity_proof_error=<why>`).
//!
//! **Nobody is joined by an address.** An identity finds a user by the provider and the subject, never
//! by the address it carries: where a user has the same confirmed address and the identity is not
//! linked, the sign-in is refused with `link_required`, and that person links the identity from their
//! Security page, signed in the old way. Unlinking is refused where it would leave the person no way
//! to sign in: no password and no other identity.
//!
//! The kind that speaks a provider's protocol is installed by the server ([`install_kind`]); the
//! release installs OpenID Connect. Where none is, a sign-in ends with `idp_error`.

pub mod landing;

use crate::middleware::authentication::{SESSION_PREFIX, new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::resources::sealing::{binding, master_key};
use crate::sign_in::second_step::SECURITY_PATH;
use crate::sign_in::{NOT_KEPT, record, throttle};
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use chrono::Duration;
use http::header::{CACHE_CONTROL, SET_COOKIE};
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::auth::{
    AuthOidcCallback, AuthOidcCallbackQuery, AuthOidcStart, AuthOidcStartQuery, MeIdentityDelete,
    MeIdentityLink, MeIdentityLinkReply, SensitivePurpose,
};
use orch_api_contract::common::{BASE_PATH, ErrorCode, SESSION_COOKIE};
use orch_db::audit::Actor;
use orch_db::queries::identity::{
    self, IDENTITY_PROVIDER_SECRET_NAME, IDENTITY_PROVIDER_SECRET_OWNER, IdentityProviderRow,
    SignInState,
};
use orch_db::queries::tokens;
use orch_db::{Conn, DbError};
use orch_identity_contract::claims::{ClaimNames, SignInError};
use orch_identity_contract::provider::{
    CallbackParams, CompleteRequest, IdentityProviderKind, KIND_OIDC, PendingSignIn,
    ProviderSettings, SIGN_IN_LIFETIME_SECS, SignInStart, StartRequest, callback_url,
    clean_redirect,
};
use orch_plugin_contract::Secret;
use orch_types::{Id, Time};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use subtle::ConstantTimeEq;

/// How long a proof given for one act may be spent: long enough to come back from a provider's sign-in
/// page to the form, short enough that a browser left open does not hold lasting proof.
pub const PROOF_LIFETIME: Duration = Duration::minutes(5);
/// What a one-use proof begins with.
pub const PROOF_PREFIX: &str = "oip_";
/// The member of the fragment that carries the proof back to the form.
pub const PROOF_FRAGMENT: &str = "identity_proof";
/// The member of the fragment that says why no proof came back.
pub const PROOF_ERROR_FRAGMENT: &str = "identity_proof_error";
/// Why no proof came back: the provider did not say it signed the person in at or after the start.
pub const NOT_FRESH: &str = "not_fresh";
/// Why no proof came back: the browser that started it has no live session here.
pub const NO_SESSION: &str = "no_session";
/// The member of the fragment of the Security page that says why an identity was not linked.
pub const LINK_ERROR_FRAGMENT: &str = "identity_link_error";
/// The cookie that marks the browser a sign-in was begun in.
pub const BROWSER_COOKIE: &str = "orch_oidc";
/// What the mark of a browser begins with.
pub const BROWSER_MARK_PREFIX: &str = "oib_";

/// The kinds of identity provider this server speaks to, by the value of `kind` that selects each.
static KINDS: LazyLock<RwLock<HashMap<&'static str, Arc<dyn IdentityProviderKind>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Makes the kind `kind` speak for the providers of its `kind`, in place of any other.
pub fn install_kind(kind: Arc<dyn IdentityProviderKind>) {
    if let Ok(mut kinds) = KINDS.write() {
        kinds.insert(kind.kind(), kind);
    }
}

/// The kind installed for `name`, if one is.
pub(crate) fn kind_of(name: &str) -> Option<Arc<dyn IdentityProviderKind>> {
    KINDS.read().ok().and_then(|kinds| kinds.get(name).cloned())
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_raw(AuthOidcStart, start)
        .on_raw(AuthOidcCallback, callback)
        .on(MeIdentityLink, link_start)
        .on(MeIdentityDelete, unlink);
}

/// The word a purpose is kept by.
fn purpose_word(purpose: SensitivePurpose) -> &'static str {
    match purpose {
        SensitivePurpose::TokenCreate => "token_create",
        SensitivePurpose::SecondStepSetup => "second_step_setup",
        SensitivePurpose::RecoveryCodes => "recovery_codes",
    }
}

/// A `302` to `location`, which no cache keeps.
pub(crate) fn redirect(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(location) {
        headers.insert(header::LOCATION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(NOT_KEPT));
    response
}

/// `response`, setting `cookies`.
pub(crate) fn with_cookies(mut response: Response, cookies: &[String]) -> Response {
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(SET_COOKIE, value);
        }
    }
    response
}

/// The in-app address `back` without a fragment of its own, so that one can be given to it.
pub(crate) fn without_fragment(back: &str) -> &str {
    back.split('#').next().unwrap_or("/")
}

/// The mark of the browser of this request: the one it carries, or a new one.
fn browser_mark(ctx: &Ctx) -> String {
    ctx.cookie(BROWSER_COOKIE)
        .filter(|mark| mark.starts_with(BROWSER_MARK_PREFIX))
        .map_or_else(|| new_token(BROWSER_MARK_PREFIX), str::to_owned)
}

/// The cookie that gives the browser its mark for as long as a sign-in lives: `Lax`, so that the
/// provider's redirect back, a navigation from another site, carries it, and sent to this module's
/// requests alone.
fn browser_cookie(mark: &str) -> String {
    format!(
        "{BROWSER_COOKIE}={mark}; Max-Age={SIGN_IN_LIFETIME_SECS}; Path={BASE_PATH}/auth/oidc; HttpOnly; Secure; SameSite=Lax"
    )
}

/// Whether the browser of this request is the one whose mark hashes to `hash`.
fn same_browser(ctx: &Ctx, hash: &[u8]) -> bool {
    ctx.cookie(BROWSER_COOKIE)
        .is_some_and(|mark| bool::from(token_hash(mark).as_slice().ct_eq(hash)))
}

/// The settings of a provider as its kind reads them.
pub(crate) fn settings_of(row: &IdentityProviderRow) -> ProviderSettings {
    ProviderSettings {
        key: row.key.clone(),
        name: row.name.clone(),
        issuer: row.issuer.clone(),
        client_id: row.client_id.clone(),
        scopes: row.scopes.clone(),
        claims: ClaimNames {
            subject: row.claim_subject.clone(),
            email: row.claim_email.clone(),
            name: row.claim_name.clone(),
            username: row.claim_username.clone(),
        },
        allowed_email_domains: row.allowed_email_domains.clone(),
        auto_register: row.auto_register,
        enabled: row.enabled,
    }
}

/// The client secret of the provider `provider`, unsealed for one call; none for a client without one.
async fn client_secret(
    conn: &mut Conn,
    provider: &IdentityProviderRow,
) -> ApiResult<Option<Secret>> {
    let Some(row) = orch_db::queries::settings::secret(
        conn,
        IDENTITY_PROVIDER_SECRET_OWNER,
        provider.id,
        IDENTITY_PROVIDER_SECRET_NAME,
    )
    .await?
    else {
        return Ok(None);
    };
    let plain = master_key()?
        .open(
            &row.sealed,
            &binding(
                IDENTITY_PROVIDER_SECRET_OWNER,
                provider.id,
                IDENTITY_PROVIDER_SECRET_NAME,
            ),
        )
        .map_err(|_| ApiError::internal("the client secret of a provider could not be opened"))?;
    let text = String::from_utf8(plain)
        .map_err(|_| ApiError::internal("the client secret of a provider is not text"))?;
    Ok(Some(Secret::new(text)))
}

/// Asks the installed kind to begin a sign-in at `provider`; none where no kind is installed or the
/// provider cannot be asked, which is logged.
async fn ask_kind(ctx: &Ctx, provider: &IdentityProviderRow, fresh: bool) -> Option<SignInStart> {
    let kind = kind_of(KIND_OIDC)?;
    let request = StartRequest {
        settings: settings_of(provider),
        callback_url: callback_url(&ctx.app.config.system.public_url, &provider.key),
        fresh,
    };
    match kind.start(&request).await {
        Ok(started) => Some(started),
        Err(error) => {
            tracing::warn!(provider = %provider.key, %error, "an identity provider could not begin a sign-in");
            None
        }
    }
}

/// The row of a sign-in begun now at `provider` in the browser marked `mark`, for nothing in
/// particular yet: the caller says what it is for.
fn state_row(ctx: &Ctx, provider: Id, pending: PendingSignIn, mark: &str) -> SignInState {
    SignInState {
        state: pending.state,
        identity_provider_id: provider,
        nonce: pending.nonce,
        pkce_verifier: pending.pkce_verifier,
        redirect_path: "/".to_owned(),
        link_user_id: None,
        invitation: None,
        accept_invitation: false,
        session_id: None,
        sensitive_purpose: None,
        service_account_id: None,
        browser_hash: token_hash(mark),
        started_at: ctx.now,
        expires_at: ctx.now + Duration::seconds(SIGN_IN_LIFETIME_SECS),
    }
}

/// Sends the browser to the provider: to sign in, or, from a signed-in browser with a purpose, to prove
/// its person again for one act.
async fn start(ctx: Ctx, query: AuthOidcStartQuery, _body: Body) -> ApiResult<Response> {
    let key = ctx.param("key")?.to_owned();
    let back = clean_redirect(query.redirect.as_deref());
    if query.accept_invitation
        && (query.sensitive_purpose.is_some()
            || query
                .invitation
                .as_deref()
                .is_none_or(|link| link.trim().is_empty()))
    {
        return Err(ApiError::invalid(
            "accept_invitation",
            "acceptance requires an invitation and is separate from a sensitive proof",
        ));
    }
    {
        let mut conn = ctx.app.db.acquire().await?;
        throttle::identity_start(&ctx, &mut conn).await?;
    }
    match query.sensitive_purpose {
        Some(purpose) => start_proof(&ctx, &key, purpose, query.service_account_id, back).await,
        None => {
            start_sign_in(
                &ctx,
                &key,
                query.invitation.as_deref(),
                query.accept_invitation,
                back,
            )
            .await
        }
    }
}

/// Begins a sign-in, which may come with an invitation.
async fn start_sign_in(
    ctx: &Ctx,
    key: &str,
    invitation: Option<&str>,
    accept_invitation: bool,
    back: String,
) -> ApiResult<Response> {
    let refused = |error: SignInError| Ok(redirect(&error.login_path()));
    let provider = {
        let mut conn = ctx.app.db.acquire().await?;
        identity::identity_provider_by_key(&mut conn, key).await?
    };
    let Some(provider) = provider.filter(|row| row.enabled) else {
        return refused(SignInError::Disabled);
    };
    let Some(started) = ask_kind(ctx, &provider, false).await else {
        return refused(SignInError::IdpError);
    };
    let mark = browser_mark(ctx);
    let mut conn = ctx.app.db.acquire().await?;
    identity::begin_sign_in(
        &mut conn,
        &SignInState {
            redirect_path: back,
            accept_invitation,
            invitation: invitation
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_owned),
            ..state_row(ctx, provider.id, started.pending, &mark)
        },
    )
    .await?;
    Ok(with_cookies(
        redirect(&started.redirect_url),
        &[browser_cookie(&mark)],
    ))
}

/// Begins proving a signed-in person again at their linked provider, for one sensitive act. The
/// session is the browser's own: the cookie this navigation from the form carries (`SameSite=Strict`
/// sends it on a navigation from the application itself).
async fn start_proof(
    ctx: &Ctx,
    key: &str,
    purpose: SensitivePurpose,
    service_account: Option<Id>,
    back: String,
) -> ApiResult<Response> {
    let refused = |why: &str| {
        Ok(redirect(&format!(
            "{}#{PROOF_ERROR_FRAGMENT}={why}",
            without_fragment(&back)
        )))
    };
    if service_account.is_some() && purpose != SensitivePurpose::TokenCreate {
        return refused(SignInError::Denied.code());
    }
    let Some(cookie) = ctx
        .cookie(SESSION_COOKIE)
        .filter(|value| value.starts_with(SESSION_PREFIX))
    else {
        return refused(NO_SESSION);
    };
    let (session, provider) = {
        let mut conn = ctx.app.db.acquire().await?;
        let Some(session) =
            identity::session_by_refresh_hash(&mut conn, &token_hash(cookie), ctx.now).await?
        else {
            return refused(NO_SESSION);
        };
        let Some(provider) = identity::identity_provider_by_key(&mut conn, key)
            .await?
            .filter(|row| row.enabled)
        else {
            return refused(SignInError::Disabled.code());
        };
        // Only the provider the person linked can prove them: any other would be somebody else's word.
        if !tokens::has_identity_at(&mut conn, provider.id, session.user_id).await? {
            return refused(SignInError::Denied.code());
        }
        throttle::identity_account(
            ctx,
            &mut conn,
            &throttle::Who::named(Some(session.user_id), ""),
        )
        .await?;
        (session, provider)
    };
    let Some(started) = ask_kind(ctx, &provider, true).await else {
        return refused(SignInError::IdpError.code());
    };
    let mark = browser_mark(ctx);
    let mut conn = ctx.app.db.acquire().await?;
    identity::begin_sign_in(
        &mut conn,
        &SignInState {
            accept_invitation: false,
            redirect_path: back,
            session_id: Some(session.id),
            sensitive_purpose: Some(purpose_word(purpose).to_owned()),
            service_account_id: service_account,
            ..state_row(ctx, provider.id, started.pending, &mark)
        },
    )
    .await?;
    Ok(with_cookies(
        redirect(&started.redirect_url),
        &[browser_cookie(&mark)],
    ))
}

/// What a begun sign-in is for, as its row says.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Purpose {
    /// Signing a person in, with the invitation it came with.
    SignIn {
        invitation: Option<String>,
        accept_invitation: bool,
    },
    /// Linking the identity to the signed-in user.
    Link { user: Id },
    /// Proving the person of the session again for the act `act`.
    Proof { session: Id, act: String },
}

impl Purpose {
    fn of(row: &SignInState) -> Self {
        match (row.session_id, &row.sensitive_purpose, row.link_user_id) {
            (Some(session), Some(act), _) => Self::Proof {
                session,
                act: act.clone(),
            },
            (_, _, Some(user)) => Self::Link { user },
            _ => Self::SignIn {
                invitation: row.invitation.clone(),
                accept_invitation: row.accept_invitation,
            },
        }
    }

    /// Where the browser is sent when the sign-in ends without what it was for, with `why`: the
    /// sign-in page, the Security page, or the form that asked for a proof.
    fn refused(&self, back: &str, why: &str) -> Response {
        match self {
            Self::SignIn { .. } => redirect(&format!("/login?error={why}")),
            Self::Link { .. } => redirect(&format!("{SECURITY_PATH}#{LINK_ERROR_FRAGMENT}={why}")),
            Self::Proof { .. } => redirect(&format!(
                "{}#{PROOF_ERROR_FRAGMENT}={why}",
                without_fragment(back)
            )),
        }
    }
}

/// Ends a sign-in begun by [`start`] or by a link: what the provider vouches for is checked by the
/// kind, and [`landing`] does what the sign-in was for.
async fn callback(ctx: Ctx, query: AuthOidcCallbackQuery, _body: Body) -> ApiResult<Response> {
    let key = ctx.param("key")?.to_owned();
    let expired = || redirect(&SignInError::StateExpired.login_path());
    let Some(state) = query.state.as_deref().filter(|state| !state.is_empty()) else {
        return Ok(expired());
    };
    // The sign-in serves once: it is taken now, in a statement of its own, whatever comes of it.
    let row = {
        let mut conn = ctx.app.db.acquire().await?;
        identity::take_sign_in(&mut conn, state).await?
    };
    let Some(row) = row else {
        return Ok(expired());
    };
    let purpose = Purpose::of(&row);
    let back = row.redirect_path.clone();
    let fail = |error: SignInError| purpose.refused(&back, error.code());
    // Only the browser that began it may end it.
    if !same_browser(&ctx, &row.browser_hash) {
        return Ok(fail(SignInError::StateExpired));
    }
    let (provider, secret) = {
        let mut conn = ctx.app.db.acquire().await?;
        let provider = identity::identity_provider(&mut conn, row.identity_provider_id).await?;
        let Some(provider) =
            provider.filter(|provider| provider.key == key && row.expires_at > ctx.now)
        else {
            return Ok(fail(SignInError::StateExpired));
        };
        let secret = client_secret(&mut conn, &provider).await?;
        (provider, secret)
    };
    let params = CallbackParams {
        code: query.code.clone(),
        state: query.state.clone(),
        error: query.error.clone(),
        error_description: query.error_description.clone(),
    };
    let code = match params.authorization_code() {
        Ok(code) => code.to_owned(),
        Err(error) => {
            tracing::info!(
                provider = %key,
                reported = query.error.as_deref().unwrap_or_default(),
                description = query.error_description.as_deref().unwrap_or_default(),
                "an identity provider ended a sign-in without a code"
            );
            return Ok(fail(error));
        }
    };
    let Some(kind) = kind_of(KIND_OIDC) else {
        return Ok(fail(SignInError::IdpError));
    };
    let request = CompleteRequest {
        settings: settings_of(&provider),
        client_secret: secret,
        callback_url: callback_url(&ctx.app.config.system.public_url, &key),
        pending: PendingSignIn {
            state: row.state.clone(),
            nonce: row.nonce.clone(),
            pkce_verifier: row.pkce_verifier.clone(),
        },
        code,
        now: Time::new(ctx.now),
    };
    let identity = match kind.complete(&request).await {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(provider = %key, %error, "an identity provider's answer did not pass");
            return Ok(fail(SignInError::IdpError));
        }
    };
    match purpose {
        Purpose::SignIn {
            invitation,
            accept_invitation,
        } => {
            landing::sign_in(
                &ctx,
                &provider,
                &identity,
                invitation.as_deref(),
                accept_invitation,
                &back,
            )
            .await
        }
        Purpose::Link { user } => landing::link(&ctx, &provider, &identity, user).await,
        Purpose::Proof { session, act } => {
            landing::proof(&ctx, &provider, &identity, &row, session, &act).await
        }
    }
}

/// `403 auth.session_required`: how a person signs in is changed from a browser sign-in alone, as a
/// password is, never with a token.
fn browser_only() -> ApiError {
    ApiError::new(
        ErrorCode::AuthSessionRequired,
        "a way to sign in is linked and unlinked from a browser sign-in",
    )
}

/// Begins linking the identity provider `key` to the signed-in person: the address of the provider to
/// send the browser to, which comes back to the Security page.
async fn link_start(ctx: Ctx, (): (), (): ()) -> ApiResult<Answer<MeIdentityLinkReply>> {
    let key = ctx.param("key")?.to_owned();
    if !ctx.user()?.is_session() {
        return Err(browser_only());
    }
    let provider = {
        let mut conn = ctx.app.db.acquire().await?;
        throttle::identity_start(&ctx, &mut conn).await?;
        throttle::identity_account(
            &ctx,
            &mut conn,
            &throttle::Who::named(Some(ctx.user()?.id()), ""),
        )
        .await?;
        identity::identity_provider_by_key(&mut conn, &key).await?
    };
    let provider = provider
        .filter(|row| row.enabled)
        .ok_or_else(ApiError::not_found)?;
    // The provider is asked before the transaction, so that no lock waits on another server.
    let started = ask_kind(&ctx, &provider, false).await.ok_or_else(|| {
        ApiError::new(
            ErrorCode::ProviderUnavailable,
            "the identity provider could not be reached",
        )
    })?;
    let mark = browser_mark(&ctx);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let person = guard.user();
    if !person.is_session() {
        return Err(browser_only());
    }
    identity::begin_sign_in(
        &mut tx,
        &SignInState {
            accept_invitation: false,
            redirect_path: SECURITY_PATH.to_owned(),
            link_user_id: Some(person.id()),
            ..state_row(&ctx, provider.id, started.pending, &mark)
        },
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(Answer::from(MeIdentityLinkReply {
        redirect_url: started.redirect_url,
    })
    .header(CACHE_CONTROL, NOT_KEPT)
    .header(SET_COOKIE, &browser_cookie(&mark)))
}

/// Unlinks the signed-in person's identity at the provider `key`, unless it is the last way they sign
/// in: they have no password and no other identity.
async fn unlink(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let person = guard.user();
    if !person.is_session() {
        return Err(browser_only());
    }
    let provider = identity::identity_provider_by_key(&mut tx, &key)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ways = identity::lock_ways_in(&mut tx, person.id()).await?;
    if !ways.providers.contains(&provider.id) {
        return Err(ApiError::not_found());
    }
    let others = ways
        .providers
        .iter()
        .filter(|each| **each != provider.id)
        .count();
    if !ways.password && others == 0 {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "this is your only way to sign in: set a password or link another first",
        ));
    }
    identity::unlink_identity(&mut tx, provider.id, person.id()).await?;
    record(
        &ctx,
        &mut tx,
        Actor::User(person.id()),
        orch_db::audit::action!(AuditAction::AuthIdentityUnlinked),
        Some(person.id()),
        &format!(
            "{} unlinked their sign-in through {}",
            person.user.username, provider.name
        ),
        json!({ "identity_provider": provider.key }),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_goes_where_the_sign_in_was_for() {
        assert_eq!(without_fragment("/settings/tokens#top"), "/settings/tokens");
        let at = |response: Response| {
            assert_eq!(response.status(), StatusCode::FOUND);
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .to_owned()
        };
        let proof = Purpose::Proof {
            session: Id::nil(),
            act: "token_create".to_owned(),
        };
        assert_eq!(
            at(proof.refused("/settings/security#x", NOT_FRESH)),
            "/settings/security#identity_proof_error=not_fresh"
        );
        let link = Purpose::Link { user: Id::nil() };
        assert_eq!(
            at(link.refused("/", "denied")),
            "/settings/security#identity_link_error=denied"
        );
        let sign_in = Purpose::SignIn {
            invitation: None,
            accept_invitation: false,
        };
        assert_eq!(
            at(sign_in.refused("/projects", "link_required")),
            "/login?error=link_required"
        );
    }

    #[test]
    fn the_mark_of_a_browser_is_sent_back_by_a_navigation_from_the_provider_and_only_here() {
        let cookie = browser_cookie("oib_abc");
        assert!(cookie.starts_with("orch_oidc=oib_abc; Max-Age=600;"));
        assert!(cookie.contains("Path=/api/v1/auth/oidc;"));
        assert!(cookie.ends_with("HttpOnly; Secure; SameSite=Lax"));
    }

    #[test]
    fn a_proof_lasts_five_minutes() {
        assert_eq!(PROOF_LIFETIME, Duration::minutes(5));
    }
}
