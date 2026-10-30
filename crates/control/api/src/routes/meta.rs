//! Health, setup and public metadata.
//!
//! Health is answered here; so is what a client may know before anybody has
//! signed in, and the making of the first administrator while the system has
//! no user.

use crate::middleware::authentication::{self, Caller};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx, Registry};
use crate::routes::nodes::make::may_make_org;
use crate::sign_in::accounts::{self, Details, How};
use crate::sign_in::{open_session, passwords, setting, system_policy};
use crate::state::App;
use axum::body::Body;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use orch_api_contract::auth::{
    AuthResult, HealthGet, LinkTrust, Meta, MetaBanner, MetaGet, MetaIdentityProvider, MetaLimits,
    MetaLinkEndpoint, SetupCreate, SetupCreateBody, SignInProof,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{IMAGE_UPLOAD_MAX_BYTES, INPUT_TEXT_MAX_CHARS};
use orch_api_contract::users::Locale;
use orch_db::queries::identity;
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_raw(HealthGet, health)
        .on(MetaGet, meta)
        .on(SetupCreate, setup);
}

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

/// The address a connector reaches the server at, as the configuration
/// names its trust.
fn trust_of(word: &str) -> LinkTrust {
    if word == "public" {
        LinkTrust::Public
    } else {
        LinkTrust::Pinned
    }
}

/// What a client may know before anybody has signed in.
pub(crate) async fn meta(ctx: Ctx, (): (), (): ()) -> ApiResult<Meta> {
    let mut conn = ctx.app.db.acquire().await?;
    let policy = system_policy(&mut conn).await?;
    let defs: Vec<_> = [keys::SYSTEM_BANNER, keys::STORAGE_MAX_BLOB_BYTES]
        .into_iter()
        .filter_map(registry::get)
        .collect();
    let snapshot = Snapshot::load(&mut conn, Place::System, Wanted::Keys(&defs))
        .await
        .map_err(ApiError::internal)?;
    let banner: Option<MetaBanner> = setting(&snapshot, keys::SYSTEM_BANNER)?.flatten();
    let max_upload: Option<i64> = setting(&snapshot, keys::STORAGE_MAX_BLOB_BYTES)?;
    let email_code_login =
        policy.mail() && identity::email_code_sign_in_set_somewhere(&mut conn).await?;
    let needs_setup = identity::no_users_yet(&mut conn).await?;
    let identity_providers = identity::enabled_identity_providers(&mut conn)
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
    let may_create_org = match authentication::authenticate(&ctx.app, &ctx.headers, ctx.now).await {
        Ok(Caller::User(authed)) => may_make_org(policy.org_creation, &authed),
        _ => false,
    };
    let build = ctx.app.build;
    Ok(Meta {
        name: ctx.app.config.system.name.clone(),
        version: build.version.to_owned(),
        build: build.build_number.to_owned(),
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
            .config
            .gateway
            .endpoints
            .iter()
            .map(|endpoint| MetaLinkEndpoint {
                url: endpoint.url.clone(),
                trust: trust_of(&endpoint.trust),
            })
            .collect(),
        limits: MetaLimits {
            max_input_chars: i64::from(INPUT_TEXT_MAX_CHARS),
            max_image_bytes: IMAGE_UPLOAD_MAX_BYTES,
            max_upload_bytes: max_upload.unwrap_or(i64::MAX),
        },
    })
}

/// Makes the first system administrator, while there is no user, and signs
/// them in.
async fn setup(ctx: Ctx, (): (), body: SetupCreateBody) -> ApiResult<Answer<AuthResult>> {
    let settled = || ApiError::new(ErrorCode::ConflictExists, "the system is set up already");
    {
        // Not worth hashing a password for, when it is too late.
        let mut conn = ctx.app.db.acquire().await?;
        if !identity::no_users_yet(&mut conn).await? {
            return Err(settled());
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
        return Err(settled());
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
    let how = json!({ "proofs": [SignInProof::Password], "setup": true });
    let opened = open_session(&ctx, &mut tx, &user, how).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(opened.answer())
}
