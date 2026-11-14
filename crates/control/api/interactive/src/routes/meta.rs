//! Health, setup and public metadata.
//!
//! Health is answered here; so is what a client may know before anybody has
//! signed in, and the making of the first administrator while the system has
//! no user.

use crate::middleware::authentication;

use crate::middleware::authentication::Caller;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::make::may_make_org;

use crate::sign_in::accounts;

use crate::sign_in::accounts::Details;

use crate::sign_in::accounts::How;

use crate::sign_in::open_session;

use crate::sign_in::passwords;

use crate::sign_in::setting;

use crate::sign_in::system_policy;

use crate::state::App;

use axum::body::Body;

use axum::extract::State;

use axum::response::IntoResponse;

use axum::response::Response;

use http::StatusCode;

use orch_api_contract::auth::HealthGet;

use orch_api_contract::auth::LinkTrust;

use orch_api_contract::auth::Meta;

use orch_api_contract::auth::MetaBanner;

use orch_api_contract::auth::MetaGet;

use orch_api_contract::auth::MetaIdentityProvider;

use orch_api_contract::auth::MetaLimits;

use orch_api_contract::auth::MetaLinkEndpoint;

use orch_api_contract::auth::SetupCreate;

use orch_api_contract::auth::SetupCreateBody;

use orch_api_contract::auth::SetupCreateReply;

use orch_api_contract::auth::SignInProof;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::threads::IMAGE_UPLOAD_MAX_BYTES;

use orch_api_contract::threads::INPUT_TEXT_MAX_CHARS;

use orch_api_contract::users::Locale;

use orch_db::queries::identity;

use orch_settings::Place;

use orch_settings::Snapshot;

use orch_settings::Wanted;

use orch_settings_contract::registry;

use orch_settings_contract::registry::keys;

use serde_json::json;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.creation(SetupCreate, super::auth::creation::setup_binding());
    registry
        .on_raw(HealthGet, health)
        .on(MetaGet, meta)
        .on(SetupCreate, setup);
}
orch_api_base::__api_bind_2af41ef4f9!();

async fn healthy(app: &App) -> bool {
    let Ok(mut conn) = app.db.acquire().await else {
        return false;
    };
    orch_db::heartbeat::worker_ready(&mut conn, app.clock.now())
        .await
        .unwrap_or(false)
}

const NOT_READY: &str =
    "not ready: the database is unavailable or the worker heartbeat is missing or stale";

async fn health(ctx: Ctx, (): (), _: Body) -> ApiResult<Response> {
    if healthy(&ctx.app).await {
        Ok((StatusCode::OK, "ok").into_response())
    } else {
        Ok((StatusCode::SERVICE_UNAVAILABLE, NOT_READY).into_response())
    }
}

/// The same check at `/health`, outside the API's base path, for the
/// proxy in front and for an operator's probe.
pub fn health_router() -> axum::Router<App> {
    axum::Router::new().route(
        "/health",
        axum::routing::get(|State(app): State<App>| async move {
            if healthy(&app).await {
                (StatusCode::OK, "ok")
            } else {
                (StatusCode::SERVICE_UNAVAILABLE, NOT_READY)
            }
        }),
    )
}
orch_api_base::__api_bind_ec9f465d00!();

/// The address a connector reaches the server at, as the configuration
/// names its trust.
fn trust_of(trust: orch_link_contract::enrollment::Trust) -> LinkTrust {
    match trust {
        orch_link_contract::enrollment::Trust::Public => LinkTrust::Public,
        orch_link_contract::enrollment::Trust::Pinned => LinkTrust::Pinned,
    }
}

/// What a client may know before anybody has signed in.
pub(crate) async fn meta(ctx: Ctx, (): (), (): ()) -> ApiResult<Meta> {
    let identity = authentication::authenticate(&ctx.app, &ctx.headers, ctx.now)
        .await
        .ok();
    let caller = match &identity {
        Some(Caller::User(authed)) => Some(authed.as_ref()),
        _ => None,
    };
    let mut conn = ctx.app.db.acquire().await?;
    on_connection(&ctx, &mut conn, caller).await
}

/// The public supplier on an existing read connection, using the actual restored HTTP identity.
pub(crate) async fn on_connection(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    caller: Option<&crate::middleware::authentication::Authed>,
) -> ApiResult<Meta> {
    let policy = system_policy(conn).await?;
    let defs: Vec<_> = [keys::SYSTEM_BANNER, keys::STORAGE_MAX_BLOB_BYTES]
        .into_iter()
        .filter_map(registry::get)
        .collect();
    let snapshot = Snapshot::load(conn, Place::System, Wanted::Keys(&defs))
        .await
        .map_err(ApiError::internal)?;
    let banner: Option<MetaBanner> = setting(&snapshot, keys::SYSTEM_BANNER)?.flatten();
    let max_upload: Option<i64> = setting(&snapshot, keys::STORAGE_MAX_BLOB_BYTES)?;
    let email_code_login =
        policy.mail() && identity::email_code_sign_in_set_somewhere(conn).await?;
    let needs_setup = identity::no_users_yet(conn).await?;
    let identity_providers = identity::enabled_identity_providers(conn)
        .await?
        .into_iter()
        .map(|(key, name, auto_register)| MetaIdentityProvider {
            key,
            name,
            auto_register,
        })
        .collect();
    // Whoever the request says is asking learns whether they may make a team org now; nobody
    // signed in may not, and a token that does not hold is nobody here, as anywhere public.
    let may_create_org = caller.is_some_and(|caller| may_make_org(policy.org_creation, caller));
    let build = ctx.app.build;
    Ok(Meta {
        name: ctx.app.config.system.name.clone(),
        version: build.version.to_owned(),
        build: build.build_number.to_owned(),
        public_url: ctx.app.config.system.public_url.clone(),
        alternate_urls: ctx.app.config.system.alternate_urls.clone(),
        needs_setup,
        registration_mode: policy.registration,
        org_creation: policy.org_creation,
        may_create_org,
        mail: policy.mail(),
        local_login: policy.local_login,
        email_code_login,
        identity_providers,
        locales: vec![Locale::En, Locale::ZhCn],
        // A notice that has ended is not shown, though its row stays.
        banner: banner.filter(|banner| banner.until.is_none_or(|until| until.get() > ctx.now)),
        // The public half of the key the worker signs push with, composed from the same key
        // file; none where the deployment has no Web Push.
        push_public_key: crate::routes::notifications::push_public_key(),
        link_endpoints: ctx
            .app
            .link
            .certificates()
            .gateway_discovery()
            .gateways
            .into_iter()
            .flat_map(|gateway| gateway.endpoints)
            .map(|endpoint| MetaLinkEndpoint {
                url: endpoint.url,
                trust: trust_of(endpoint.trust),
            })
            .collect(),
        limits: MetaLimits {
            max_input_chars: i64::from(INPUT_TEXT_MAX_CHARS),
            max_image_bytes: IMAGE_UPLOAD_MAX_BYTES,
            max_upload_bytes: max_upload.unwrap_or(i64::MAX),
        },
    })
}
orch_api_base::__api_bind_f9d963c9b2!();

pub(crate) fn setup_settled() -> ApiError {
    ApiError::new(ErrorCode::ConflictExists, "the system is set up already")
}
orch_api_base::__api_bind_f02731996c!();

/// Makes the first system administrator, while there is no user, and signs
/// them in.
async fn setup(ctx: Ctx, (): (), body: SetupCreateBody) -> ApiResult<Answer<SetupCreateReply>> {
    {
        // Not worth hashing a password for, when it is too late.
        let mut conn = ctx.app.db.acquire().await?;
        if !identity::no_users_yet(&mut conn).await? {
            return Err(setup_settled());
        }
    }
    let clean = accounts::check(
        &Details {
            username: &body.username,
            display_name: &body.display_name,
            password: &body.password,
            email: body.email.as_deref(),
        },
        &ctx.app.config.auth,
        false,
    )?;
    let hash = passwords::hash(&body.password)
        .await
        .map_err(ApiError::internal)?;
    let mut tx = ctx.app.db.begin().await?;
    // Two requests that both found no user must not both make one.
    identity::lock_setup(&mut tx).await?;
    if !identity::no_users_yet(&mut tx).await? {
        return Err(setup_settled());
    }
    // The mail server, if there is one, is not up yet at the first start, so
    // the address is kept as given.
    let user = accounts::make(
        &ctx,
        &mut tx,
        &clean,
        &hash,
        &How {
            email_confirmed: false,
            is_system_admin: true,
            timezone: body.timezone.clone(),
        },
    )
    .await?;
    if crate::routes::notifications::push_public_key().is_some() {
        orch_worker_core::channel_setup::ensure_system_push(&mut tx, user.id, ctx.now).await?;
    }
    let how = json!({ "proofs": [SignInProof::Password], "setup": true });
    let opened = open_session(&ctx, &mut tx, &user, how).await?;
    orch_db::idempotency::creation::observe_account(&mut tx, user.id, "setup_account", 200).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let answer = opened.answer();
    Ok(Answer {
        body: SetupCreateReply::full(answer.body),
        status: answer.status,
        headers: answer.headers,
    })
}
