//! Actual production routing must separate session and account-key credentials before effects.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test failures panic"
)]

mod support;

use axum::body::Body;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::KeyScope;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::registry::Registry;
use orch_db::queries::identity;
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use support::World;
use tower::ServiceExt;

fn production(world: &World) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    orch_api_core::server::router(world.app.app.clone(), registry)
}

async fn request(
    router: &axum::Router,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie).header("x-orch-csrf", "1");
    }
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = router
        .clone()
        .oneshot(
            request
                .body(body.map_or_else(Body::empty, |value| {
                    Body::from(serde_json::to_vec(&value).unwrap())
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn key(world: &World, owner: Id, modern: bool) -> String {
    let secret = new_token(API_TOKEN_PREFIX);
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"account","id":owner,"reach":"self"}, "resources":["account"],
        "allow":[{"operation":"MeGet","actions":["read"]},{"operation":"MeRecoveryCodesGet","actions":["read"]},{"operation":"MeUpdate.profile","actions":["write","read"]}]
    }]})).unwrap();
    let scope = modern.then(|| serde_json::to_value(scope).unwrap());
    let scopes = if modern { vec![] } else { vec!["*".to_owned()] };
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Surface control",
            token_hash: &token_hash(&secret),
            scopes: &scopes,
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    secret
}

#[tokio::test]
async fn surface_browser_refuses_both_key_kinds_before_an_ordinary_profile_effect() {
    let world = World::new().await;
    let person = world.person("surface-person").await;
    let signed = world.sign_in(&person).await;
    let router = production(&world);
    let initial = request(
        &router,
        "GET",
        "/api/v1/me",
        Some(signed.token()),
        None,
        None,
    )
    .await;
    assert_eq!(initial.0, StatusCode::OK);
    for modern in [false, true] {
        let token = key(&world, person.id, modern).await;
        let denied = request(
            &router,
            "PATCH",
            "/api/v1/me",
            Some(&token),
            None,
            Some(json!({"display_name":"foreign credential wrote this"})),
        )
        .await;
        let after = request(
            &router,
            "GET",
            "/api/v1/me",
            Some(signed.token()),
            None,
            None,
        )
        .await;
        assert_eq!(
            after.1["display_name"], initial.1["display_name"],
            "foreign key reached the ordinary mutation handler"
        );
        assert_eq!(
            denied.0,
            StatusCode::UNAUTHORIZED,
            "browser surface accepted a key kind"
        );
    }
}

#[tokio::test]
async fn surface_key_accepts_its_account_credential_and_refuses_browser_credentials() {
    let world = World::new().await;
    let person = world.person("key-plane-person").await;
    let signed = world.sign_in(&person).await;
    let router = production(&world);
    for modern in [false, true] {
        let token = key(&world, person.id, modern).await;
        let read = request(&router, "GET", "/openapi/v1/me", Some(&token), None, None).await;
        assert_eq!(read.0, StatusCode::OK);
        assert_eq!(read.1["id"], person.id.to_string());
    }
    let denied = request(
        &router,
        "PATCH",
        "/openapi/v1/me",
        Some(signed.token()),
        None,
        Some(json!({"display_name":"session crossed into keys"})),
    )
    .await;
    assert_eq!(denied.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        request(
            &router,
            "GET",
            "/openapi/v1/me",
            None,
            Some(&signed.browser()),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &router,
            "GET",
            "/api/v1/me",
            Some(signed.token()),
            None,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn surface_foreign_key_cannot_pass_public_or_cookie_shortcuts() {
    let world = World::new().await;
    let person = world.person("cookie-person").await;
    let signed = world.sign_in(&person).await;
    let token = key(&world, person.id, false).await;
    let router = production(&world);
    assert_eq!(
        request(&router, "GET", "/api/v1/meta", None, None, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&router, "GET", "/api/v1/meta", Some(&token), None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let denied = request(
        &router,
        "POST",
        "/api/v1/auth/logout",
        Some(&token),
        Some(&signed.browser()),
        None,
    )
    .await;
    assert_eq!(
        request(
            &router,
            "GET",
            "/api/v1/me",
            Some(signed.token()),
            None,
            None
        )
        .await
        .0,
        StatusCode::OK,
        "foreign key logged out a valid browser"
    );
    assert_eq!(denied.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/v1/auth/logout",
            None,
            Some(&signed.browser()),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
}

#[test]
fn surface_public_openapi_contains_only_shared_business_operations_and_key_security() {
    use orch_api_contract::operations::{Delegation, for_route};
    let routes = orch_api_contract::routes::all();
    let document = orch_api_core::openapi::document(&routes, "surface-control");
    let wanted: BTreeSet<_> = routes
        .iter()
        .filter(|route| for_route(route).unwrap().delegation == Delegation::Business)
        .map(|route| route.name.to_owned())
        .collect();
    let actual: BTreeSet<_> = document["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|path| path.as_object().unwrap().values())
        .map(|operation| operation["operationId"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        actual, wanted,
        "public docs expose browser/authentication routes or omit shared key business"
    );
    assert_eq!(document["servers"][0]["url"], "/openapi/v1");
    for shared in ["ClientFrame", "ServerFrame"] {
        assert!(
            document["components"]["schemas"][shared].is_object(),
            "public key API lost the existing shared realtime schema {shared}"
        );
    }
    let security = document["components"]["securitySchemes"]
        .as_object()
        .unwrap();
    assert_eq!(security.len(), 1);
    let description = security.values().next().unwrap().to_string().to_lowercase();
    assert!(description.contains("bearer"));
    assert!(!description.contains("browser") && !description.contains("session"));
    let internal = include_str!("../../../../frontend/src/generated/api-types.ts");
    for route in ["/auth/login", "/auth/refresh", "/me/admin-mode"] {
        assert!(
            internal.contains(route),
            "frontend internal contract lost {route}"
        );
    }
}

#[tokio::test]
async fn surface_public_and_internal_downloads_keep_the_same_non_person_behavior() {
    let world = World::new().await;
    let browser = production(&world);
    let internal =
        orch_api_core::server::internal_router(world.app.app.clone(), axum::Router::new());
    let path = "/api/v1/downloads/connector/not-an-os/not-an-arch";
    let outside = request(&browser, "GET", path, None, None, None).await;
    let inside = request(&internal, "GET", path, None, None, None).await;
    assert_eq!(
        (outside.0, &outside.1["code"]),
        (inside.0, &inside.1["code"]),
        "internal download registry lost its explicit public route placement"
    );
    assert!(!matches!(
        inside.0,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::INTERNAL_SERVER_ERROR
    ));
    let root_health = request(&browser, "GET", "/health", None, None, None)
        .await
        .0;
    let api_health = request(&browser, "GET", "/api/v1/health", None, None, None)
        .await
        .0;
    assert_eq!(root_health, api_health);
    assert!(
        matches!(
            root_health,
            StatusCode::OK | StatusCode::SERVICE_UNAVAILABLE
        ),
        "worker readiness is independent from person authentication"
    );
    assert_eq!(
        request(&browser, "GET", "/api/v1/meta", None, None, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&browser, "GET", "/openapi/v1/auth/login", None, None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn surface_proxy_preserves_distinct_http_and_websocket_paths() {
    let proxy = include_str!("../../../../deploy/nginx.conf");
    for location in [
        "location /api/",
        "location /api/v1/ws",
        "location /openapi/",
        "location /openapi/v1/ws",
        "location /link/",
        "location /internal/",
    ] {
        assert!(proxy.contains(location), "missing explicit {location}");
    }
}

#[tokio::test]
async fn surface_key_retains_session_annotated_business_and_service_identity_without_elevation() {
    let world = World::new().await;
    let person = world.person("surface-owner").await;
    let token = key(&world, person.id, true).await;
    let router = production(&world);
    let metadata = request(
        &router,
        "GET",
        "/openapi/v1/me/recovery-codes",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(
        metadata.0,
        StatusCode::OK,
        "Access::Session incorrectly hid shared Business metadata"
    );
    assert_eq!(
        request(
            &router,
            "PUT",
            "/openapi/v1/me/admin-mode",
            Some(&token),
            None,
            Some(json!({"enabled":true}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let service = new_id();
    identity::create_service_account(
        &mut *world.conn().await,
        service,
        person.personal_org,
        "surface-service",
        "Surface service",
        person.id,
        world.now(),
    )
    .await
    .unwrap();
    let service_key = key(&world, service, true).await;
    let read = request(
        &router,
        "GET",
        "/openapi/v1/me",
        Some(&service_key),
        None,
        None,
    )
    .await;
    assert_eq!(read.0, StatusCode::OK);
    assert_eq!(read.1["id"], service.to_string());
    assert_eq!(
        request(&router, "GET", "/api/v1/me", Some(&service_key), None, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn surface_test_client_uses_real_prefixes_and_never_overrides_an_explicit_plane() {
    use orch_api_contract::auth::MeGet;
    use orch_api_contract::surface::HttpSurface;
    let world = World::new().await;
    let person = world.person("helper-person").await;
    let signed = world.sign_in(&person).await;
    let token = key(&world, person.id, true).await;
    assert_eq!(
        world.app.call(MeGet, &[]).token(&token).send().await.status,
        StatusCode::OK
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .surface(HttpSurface::Browser)
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .surface(HttpSurface::Key)
            .token(signed.token())
            .send()
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}
