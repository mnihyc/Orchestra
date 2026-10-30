//! Signing in through an identity provider, end to end against the signed local provider: a good sign-in opens exactly one session, every answer that does not pass (another issuer, audience or nonce, an ended token, a key the provider does not publish, a stolen code with another PKCE verifier, another browser) opens none, an address joins nobody, an identity is linked and unlinked from a browser sign-in, and the provider's test says what it found.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use http::HeaderMap;
use orch_api_contract::admin::{
    AdminIdentityProviderCreate, AdminIdentityProviderTest, AdminIdentityProviderUpdate,
    IdentityProvider, IdentityProviderInput, IdentityProviderUpdateBody,
};
use orch_api_contract::auth::{
    AdminModeSetBody, AuthOidcCallback, AuthOidcCallbackQuery, AuthOidcStart, AuthOidcStartQuery,
    AuthRefresh, MeAdminModeSet, MeGet, MeIdentityDelete, MeIdentityLink, SensitivePurpose,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::users::TimezoneMode;
use orch_api_core::routes::auth::oidc::install_kind;
use orch_config::ServerConfig;
use orch_identity_oidc::OidcKind;
use orch_testkit::oidc::{
    CLIENT_ID, CLIENT_SECRET, Faults, OidcProvider, Person as Somebody, ProviderConfig,
};
use orch_types::{Id, new_id};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Once};
use support::{Signed, World, cookies, csrf};

const KEY: &str = "acme";

/// The world, the provider, and the system admin who added it.
struct Sso {
    world: World,
    provider: OidcProvider,
    admin: Signed,
    made: IdentityProvider,
}

async fn sso() -> Sso {
    sso_with(ServerConfig::default()).await
}

async fn sso_with(config: ServerConfig) -> Sso {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| install_kind(Arc::new(OidcKind::new().unwrap())));
    let world = World::with(config).await;
    // The client secret is sealed with the key the world signs in with.
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let callback = format!(
        "{}/api/v1/auth/oidc/{KEY}/callback",
        world.app.app.config.system.public_url.trim_end_matches('/')
    );
    let provider = OidcProvider::start(ProviderConfig::new(
        &callback,
        Arc::new(world.app.clock.clone()),
    ))
    .await
    .unwrap();
    let root = world.person("root").await;
    world.make_admin(&root).await;
    let admin = world.sign_in(&root).await;
    world
        .app
        .call(MeAdminModeSet, &[])
        .token(admin.token())
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let made = world
        .app
        .call(AdminIdentityProviderCreate, &[])
        .token(admin.token())
        .body(&IdentityProviderInput {
            key: KEY.to_owned(),
            name: "Acme".to_owned(),
            issuer: provider.issuer(),
            client_id: CLIENT_ID.to_owned(),
            client_secret: Some(CLIENT_SECRET.to_owned()),
            scopes: None,
            claim_subject: None,
            claim_email: None,
            claim_name: None,
            claim_username: None,
            allowed_email_domains: None,
            auto_register: Some(true),
            default_org_id: None,
            default_role: None,
            enabled: Some(true),
        })
        .send()
        .await
        .ok();
    assert_eq!(made.callback_url, callback);
    world.set("registration.mode", json!("open")).await;
    Sso {
        world,
        provider,
        admin,
        made,
    }
}

/// The cookies a browser holds, by name.
#[derive(Debug, Clone, Default)]
struct Browser {
    cookies: BTreeMap<String, String>,
}

impl Browser {
    fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn keep(&mut self, headers: &HeaderMap) {
        for cookie in cookies(headers) {
            if cookie.max_age() == Some(0) {
                self.cookies.remove(&cookie.name);
            } else {
                self.cookies.insert(cookie.name, cookie.value);
            }
        }
    }

    fn has(&self, name: &str) -> bool {
        self.cookies.contains_key(name)
    }
}

fn location(headers: &HeaderMap) -> String {
    headers["location"].to_str().unwrap().to_owned()
}

fn query_of(address: &str) -> HashMap<String, String> {
    url::Url::parse(address)
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

impl Sso {
    /// Begins a sign-in from the sign-in page's button in `browser`: the address of the provider.
    async fn begin(
        &self,
        browser: &mut Browser,
        redirect: Option<&str>,
        invitation: Option<&str>,
    ) -> String {
        self.begin_accepting(browser, redirect, invitation, false)
            .await
    }

    async fn begin_accepting(
        &self,
        browser: &mut Browser,
        redirect: Option<&str>,
        invitation: Option<&str>,
        accept_invitation: bool,
    ) -> String {
        let mut call = self
            .world
            .app
            .call(AuthOidcStart, &[&KEY])
            .query(&AuthOidcStartQuery {
                accept_invitation,
                redirect: redirect.map(str::to_owned),
                invitation: invitation.map(str::to_owned),
                ..AuthOidcStartQuery::default()
            });
        let held = browser.header();
        if !held.is_empty() {
            call = call.header("cookie", &held);
        }
        let reply = call.send_bytes().await;
        assert_eq!(reply.status, 302);
        browser.keep(&reply.headers);
        location(&reply.headers)
    }

    /// Comes back to the callback at the address the provider sent `browser` to: where the browser is
    /// sent next.
    async fn come_back(&self, browser: &mut Browser, back: &str) -> String {
        let query = query_of(back);
        let mut call =
            self.world
                .app
                .call(AuthOidcCallback, &[&KEY])
                .query(&AuthOidcCallbackQuery {
                    code: query.get("code").cloned(),
                    state: query.get("state").cloned(),
                    error: query.get("error").cloned(),
                    error_description: None,
                });
        let held = browser.header();
        if !held.is_empty() {
            call = call.header("cookie", &held);
        }
        let reply = call.send_bytes().await;
        assert_eq!(reply.status, 302);
        browser.keep(&reply.headers);
        location(&reply.headers)
    }

    /// A whole sign-in in `browser`: the button, the provider, the callback. Where the browser ends.
    async fn sign_in(&self, browser: &mut Browser, redirect: Option<&str>) -> String {
        let to_provider = self.begin(browser, redirect, None).await;
        let back = self.provider.authorize(&to_provider).await.unwrap();
        self.come_back(browser, &back).await
    }

    async fn count(&self, sql: &'static str, id: Id) -> i64 {
        sqlx::query_scalar(sql)
            .bind(id)
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }

    async fn user_named(&self, username: &str) -> Option<Id> {
        sqlx::query_scalar("SELECT id FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(&self.world.db.pool)
            .await
            .unwrap()
    }

    async fn sessions(&self, user: Id) -> i64 {
        self.count("SELECT count(*) FROM sessions WHERE user_id = $1", user)
            .await
    }

    async fn identities_at(&self, subject: &str) -> Vec<Id> {
        sqlx::query_scalar("SELECT user_id FROM user_identities WHERE subject = $1")
            .bind(subject)
            .fetch_all(&self.world.db.pool)
            .await
            .unwrap()
    }

    async fn all_sessions(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(&self.world.db.pool)
            .await
            .unwrap()
    }

    /// Starts linking the provider for a signed-in person, from their Security page: the address of
    /// the provider, with the browser given its mark.
    async fn begin_link(&self, signed: &Signed, browser: &mut Browser) -> String {
        let reply = self
            .world
            .app
            .call(MeIdentityLink, &[&KEY])
            .token(signed.token())
            .send()
            .await;
        browser.keep(&reply.headers);
        reply.ok().redirect_url
    }
}

fn dana() -> Somebody {
    Somebody::new("sub-dana")
        .with_email("dana@acme.example", true)
        .named("Dana Whit")
        .with_username("dana")
}

#[tokio::test]
async fn identity_starts_share_the_sign_in_address_limit_and_forwarded_headers_do_not_evade_it() {
    let mut config = ServerConfig::default();
    config.auth.address_failure_limit = 2;
    let window = config.auth.address_failure_window_secs;
    let sso = sso_with(config).await;
    for forwarded in ["203.0.113.10", "203.0.113.11"] {
        let reply = sso
            .world
            .app
            .call(AuthOidcStart, &[&KEY])
            .peer("198.51.100.7:5000".parse().unwrap())
            .header("x-forwarded-for", forwarded)
            .send_bytes()
            .await;
        assert_eq!(reply.status, 302);
        assert!(location(&reply.headers).starts_with(&sso.provider.issuer()));
    }
    let refused = sso
        .world
        .app
        .call(AuthOidcStart, &[&KEY])
        .peer("198.51.100.7:5001".parse().unwrap())
        .header("x-forwarded-for", "203.0.113.12")
        .send_bytes()
        .await;
    assert_eq!(refused.status, 429);
    assert_eq!(refused.problem().code, ErrorCode::RateLimited);
    assert_eq!(
        refused.headers["retry-after"].to_str().unwrap(),
        window.to_string()
    );
    assert!(!refused.headers.contains_key("set-cookie"));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_states")
        .fetch_one(&sso.world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "a refused start makes no state");

    let elsewhere = sso
        .world
        .app
        .call(AuthOidcStart, &[&KEY])
        .peer("198.51.100.8:5000".parse().unwrap())
        .send_bytes()
        .await;
    assert_eq!(
        elsewhere.status, 302,
        "another real address has its own limit"
    );
    sso.world
        .advance(Duration::seconds(i64::try_from(window).unwrap()));
    let later = sso
        .world
        .app
        .call(AuthOidcStart, &[&KEY])
        .peer("198.51.100.7:5000".parse().unwrap())
        .send_bytes()
        .await;
    assert_eq!(
        later.status, 302,
        "only the configured window ends the limit"
    );
}

#[tokio::test]
async fn a_link_and_fresh_proof_share_one_account_limit_across_sessions_and_addresses() {
    let mut config = ServerConfig::default();
    config.auth.login_failure_limit = 2;
    let window = config.auth.login_failure_window_secs;
    let sso = sso_with(config).await;
    let ada = sso.world.person("ada").await;
    let first = sso.world.sign_in(&ada).await;
    let second = sso.world.sign_in(&ada).await;
    sso.provider
        .sign_in_as(Somebody::new("sub-ada").with_email(&ada.email, true));
    let mut browser = Browser::default();
    let to_provider = sso.begin_link(&first, &mut browser).await;
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    assert_eq!(
        sso.come_back(&mut browser, &back).await,
        "/settings/security"
    );
    let proof = sso
        .world
        .app
        .call(AuthOidcStart, &[&KEY])
        .query(&AuthOidcStartQuery {
            accept_invitation: false,
            sensitive_purpose: Some(SensitivePurpose::TokenCreate),
            ..AuthOidcStartQuery::default()
        })
        .header("cookie", &first.browser())
        .peer("198.51.100.10:5000".parse().unwrap())
        .send_bytes()
        .await;
    assert_eq!(proof.status, 302);
    assert!(location(&proof.headers).starts_with(&sso.provider.issuer()));
    let refused = sso
        .world
        .app
        .call(MeIdentityLink, &[&KEY])
        .token(second.token())
        .peer("198.51.100.11:5000".parse().unwrap())
        .send()
        .await;
    assert_eq!(refused.status, 429);
    assert_eq!(
        refused.headers["retry-after"].to_str().unwrap(),
        window.to_string()
    );
    assert_eq!(refused.problem().code, ErrorCode::RateLimited);
    let bo = sso.world.person("bob").await;
    let bo = sso.world.sign_in(&bo).await;
    assert_eq!(
        sso.world
            .app
            .call(MeIdentityLink, &[&KEY])
            .token(bo.token())
            .send()
            .await
            .status,
        200,
        "another person is unaffected"
    );

    sso.world
        .advance(Duration::seconds(i64::try_from(window).unwrap()));
    let renewed = sso
        .world
        .app
        .call(AuthRefresh, &[])
        .header(csrf().0, csrf().1)
        .header("cookie", &second.browser())
        .send()
        .await
        .ok();
    assert_eq!(
        sso.world
            .app
            .call(MeIdentityLink, &[&KEY])
            .token(&renewed.access_token)
            .send()
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn anonymous_sign_ins_count_the_verified_account_across_addresses_before_opening_a_session() {
    let mut config = ServerConfig::default();
    config.auth.login_failure_limit = 2;
    let sso = sso_with(config).await;
    sso.provider.sign_in_as(dana());
    sso.sign_in(&mut Browser::default(), None).await;
    let user = sso.user_named("dana").await.unwrap();
    for (offset, expected) in [
        (1, "/"),
        (2, "/login?error=idp_error"),
        (3, "/login?error=idp_error"),
    ] {
        let peer = format!("198.51.100.{}:5000", 20 + offset).parse().unwrap();
        let start = sso
            .world
            .app
            .call(AuthOidcStart, &[&KEY])
            .peer(peer)
            .send_bytes()
            .await;
        assert_eq!(start.status, 302);
        let mut browser = Browser::default();
        browser.keep(&start.headers);
        let back = sso
            .provider
            .authorize(&location(&start.headers))
            .await
            .unwrap();
        let query = query_of(&back);
        let reply = sso
            .world
            .app
            .call(AuthOidcCallback, &[&KEY])
            .peer(peer)
            .header("cookie", &browser.header())
            .query(&AuthOidcCallbackQuery {
                code: query.get("code").cloned(),
                state: query.get("state").cloned(),
                ..AuthOidcCallbackQuery::default()
            })
            .send_bytes()
            .await;
        assert_eq!(reply.status, 302);
        assert_eq!(location(&reply.headers), expected);
        if offset >= 2 {
            assert!(reply.headers.contains_key("retry-after"));
            assert!(
                !reply.headers.contains_key("set-cookie"),
                "no session for a refused account"
            );
        }
    }
    assert_eq!(
        sso.sessions(user).await,
        2,
        "registration is the first of the account's two accepted sign-ins"
    );
}

#[tokio::test]
async fn a_registration_losing_the_username_race_redirects_to_sign_in_and_links_nobody() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let mut browser = Browser::default();
    let to_provider = sso.begin(&mut browser, None, None).await;
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    let query = query_of(&back);
    let before = sso.all_sessions().await;
    let winner = new_id();
    let mut competing = sso.world.db.pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *competing)
        .await
        .unwrap();
    orch_db::queries::identity::testing::insert_user(&mut competing, winner, "dana")
        .await
        .unwrap();
    // The uncommitted winner is invisible to the name search, but holds the
    // unique index entry. Commit only once the callback is waiting on it.
    let running = sso
        .world
        .app
        .call(AuthOidcCallback, &[&KEY])
        .header("cookie", &browser.header())
        .query(&AuthOidcCallbackQuery {
            code: query.get("code").cloned(),
            state: query.get("state").cloned(),
            ..AuthOidcCallbackQuery::default()
        })
        .send_bytes();
    let release = async {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                     WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))",
                )
                .bind(blocker)
                .fetch_one(&sso.world.db.pool)
                .await
                .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the callback reached the conflicting name insert");
        competing.commit().await.unwrap();
    };
    let (reply, ()) = tokio::join!(running, release);
    assert_eq!(reply.status, 302, "a browser gets a redirect, not JSON 409");
    assert_eq!(location(&reply.headers), "/login?error=idp_error");
    assert!(!reply.headers.contains_key("set-cookie"));
    assert_eq!(sso.all_sessions().await, before);
    assert_eq!(sso.user_named("dana").await, Some(winner));
    assert!(
        sso.identities_at("sub-dana").await.is_empty(),
        "the winner is never silently linked"
    );
    assert_eq!(sso.user_named("dana2").await, None);
}

#[tokio::test]
async fn a_first_sign_in_makes_one_person_with_one_session_and_the_next_signs_them_in_again() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let mut browser = Browser::default();
    let landed = sso.sign_in(&mut browser, Some("/projects")).await;
    assert_eq!(landed, "/projects");
    assert!(browser.has("orch_session") && browser.has("orch_device"));
    let dana = sso.user_named("dana").await.expect("a person was made");
    assert_eq!(sso.sessions(dana).await, 1, "exactly one session");
    assert_eq!(sso.identities_at("sub-dana").await, [dana]);
    let (email, confirmed, password): (Option<String>, bool, bool) = sqlx::query_as(
        "SELECT email, email_verified_at IS NOT NULL, password_hash IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(dana)
    .fetch_one(&sso.world.db.pool)
    .await
    .unwrap();
    assert_eq!(email.as_deref(), Some("dana@acme.example"));
    assert!(confirmed, "the provider confirmed the address");
    assert!(
        !password,
        "a person made through a provider has no password"
    );
    let audit = sso.world.audit().await;
    for action in ["user.registered", "auth.identity_linked", "auth.login"] {
        assert!(
            audit.iter().any(|each| each == action),
            "{action}: {audit:?}"
        );
    }

    // The session is the browser's: it refreshes as any other does.
    let refreshed = sso
        .world
        .app
        .call(AuthRefresh, &[])
        .header(csrf().0, csrf().1)
        .header("cookie", &browser.header())
        .send()
        .await
        .ok();
    assert_eq!(refreshed.user.user.username, "dana");
    assert_eq!(refreshed.user.timezone_mode, TimezoneMode::Device);
    assert_eq!(
        refreshed.user.timezone, "UTC",
        "the first browser may report its zone after provisioning"
    );

    let mut other = Browser::default();
    assert_eq!(sso.sign_in(&mut other, None).await, "/");
    assert_eq!(sso.user_named("dana2").await, None, "nobody else is made");
    assert_eq!(sso.sessions(dana).await, 2);
}

#[tokio::test]
async fn an_answer_that_does_not_pass_opens_no_session_and_makes_nobody() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let faults = [
        Faults {
            issuer: Some("http://127.0.0.1:9".to_owned()),
            ..Faults::default()
        },
        Faults {
            audience: Some(json!("another-client")),
            ..Faults::default()
        },
        Faults {
            nonce: Some("a-nonce-of-another-sign-in".to_owned()),
            ..Faults::default()
        },
        Faults {
            lifetime: Some(-300),
            ..Faults::default()
        },
        Faults {
            unpublished_key: true,
            ..Faults::default()
        },
        Faults {
            unsigned: true,
            ..Faults::default()
        },
    ];
    let before = sso.all_sessions().await;
    for fault in faults {
        sso.provider.set_faults(fault.clone());
        let mut browser = Browser::default();
        let landed = sso.sign_in(&mut browser, None).await;
        assert_eq!(landed, "/login?error=idp_error", "{fault:?}");
        assert!(!browser.has("orch_session"), "{fault:?}");
    }
    assert_eq!(sso.all_sessions().await, before);
    assert_eq!(sso.user_named("dana").await, None);
    assert!(sso.identities_at("sub-dana").await.is_empty());
}

#[tokio::test]
async fn a_code_exchanged_with_another_pkce_verifier_opens_nothing() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let mut browser = Browser::default();
    let to_provider = sso.begin(&mut browser, None, None).await;
    let state = query_of(&to_provider)["state"].clone();
    // Somebody who took the code on its way has a verifier of their own, never this sign-in's.
    sqlx::query("UPDATE oidc_states SET pkce_verifier = $2 WHERE state = $1")
        .bind(&state)
        .bind("a-verifier-of-somebody-who-took-the-code-on-its-way")
        .execute(&sso.world.db.pool)
        .await
        .unwrap();
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    assert_eq!(
        sso.come_back(&mut browser, &back).await,
        "/login?error=idp_error"
    );
    assert_eq!(sso.user_named("dana").await, None);
    assert!(!browser.has("orch_session"));
}

#[tokio::test]
async fn a_sign_in_ends_only_in_the_browser_that_began_it_and_only_once() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let mut mine = Browser::default();
    let to_provider = sso.begin(&mut mine, None, None).await;
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    // The address of the callback, sent to somebody else's browser.
    let mut theirs = Browser::default();
    assert_eq!(
        sso.come_back(&mut theirs, &back).await,
        "/login?error=state_expired"
    );
    assert!(!theirs.has("orch_session"));
    assert_eq!(
        sso.come_back(&mut mine, &back).await,
        "/login?error=state_expired",
        "the sign-in served once"
    );
    assert_eq!(sso.user_named("dana").await, None);
}

#[tokio::test]
async fn an_address_that_a_person_has_joins_nobody_until_that_person_links_the_identity() {
    let sso = sso().await;
    let ada = sso.world.person("ada").await;
    sso.provider.sign_in_as(
        Somebody::new("sub-ada")
            .with_email(&ada.email, true)
            .named("Ada"),
    );
    let mut stranger = Browser::default();
    assert_eq!(
        sso.sign_in(&mut stranger, None).await,
        "/login?error=link_required"
    );
    assert_eq!(sso.sessions(ada.id).await, 0, "no silent take-over");
    assert!(sso.identities_at("sub-ada").await.is_empty());
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'human'")
        .fetch_one(&sso.world.db.pool)
        .await
        .unwrap();
    assert_eq!(made, 2, "root and ada, nobody else");

    // Ada signs in the old way and links it from her Security page.
    let signed = sso.world.sign_in(&ada).await;
    let mut hers = Browser::default();
    let to_provider = sso.begin_link(&signed, &mut hers).await;
    assert!(hers.has("orch_oidc"));
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    assert_eq!(sso.come_back(&mut hers, &back).await, "/settings/security");
    assert_eq!(sso.identities_at("sub-ada").await, [ada.id]);
    let me = sso
        .world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .ok();
    assert_eq!(me.identities.len(), 1);
    assert_eq!(me.identities[0].idp_key, KEY);
    // From then on the identity signs her in.
    let mut later = Browser::default();
    assert_eq!(sso.sign_in(&mut later, None).await, "/");
    assert_eq!(
        sso.sessions(ada.id).await,
        2,
        "her password session and this one"
    );
}

#[tokio::test]
async fn an_identity_of_another_person_is_not_linked_and_the_last_way_in_is_not_unlinked() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    let mut first = Browser::default();
    sso.sign_in(&mut first, None).await;
    let dana = sso.user_named("dana").await.unwrap();

    // Bo links, and gets back to his Security page told why not: the identity is Dana's.
    let bo = sso.world.person("bob").await;
    let signed = sso.world.sign_in(&bo).await;
    let mut his = Browser::default();
    let to_provider = sso.begin_link(&signed, &mut his).await;
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    assert_eq!(
        sso.come_back(&mut his, &back).await,
        "/settings/security#identity_link_error=denied"
    );
    assert_eq!(sso.identities_at("sub-dana").await, [dana]);

    // Bo links an identity of his own, and unlinks it: he has a password.
    sso.provider
        .sign_in_as(Somebody::new("sub-bo").with_email("bo@acme.example", true));
    let to_provider = sso.begin_link(&signed, &mut his).await;
    let back = sso.provider.authorize(&to_provider).await.unwrap();
    assert_eq!(sso.come_back(&mut his, &back).await, "/settings/security");
    assert_eq!(sso.identities_at("sub-bo").await, [bo.id]);
    let unlinked = sso
        .world
        .app
        .call(MeIdentityDelete, &[&KEY])
        .token(signed.token())
        .send()
        .await;
    assert_eq!(unlinked.status, 204);
    assert!(sso.identities_at("sub-bo").await.is_empty());

    // Dana has no password and no other identity: hers is the last way in.
    sqlx::query("UPDATE users SET password_hash = NULL WHERE id = $1")
        .bind(dana)
        .execute(&sso.world.db.pool)
        .await
        .unwrap();
    let refreshed = sso
        .world
        .app
        .call(AuthRefresh, &[])
        .header(csrf().0, csrf().1)
        .header("cookie", &first.header())
        .send()
        .await
        .ok();
    let refused = sso
        .world
        .app
        .call(MeIdentityDelete, &[&KEY])
        .token(&refreshed.access_token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ResourceInUse);
    assert_eq!(sso.identities_at("sub-dana").await, [dana]);
}

#[tokio::test]
async fn a_provider_or_a_system_that_registers_nobody_makes_nobody_and_a_refusal_is_denied() {
    let sso = sso().await;
    sso.provider.sign_in_as(dana());
    sso.world.set("registration.mode", json!("closed")).await;
    let mut browser = Browser::default();
    assert_eq!(
        sso.sign_in(&mut browser, None).await,
        "/login?error=registration_closed"
    );
    sso.world.set("registration.mode", json!("invite")).await;
    assert_eq!(
        sso.sign_in(&mut browser, None).await,
        "/login?error=invite_required"
    );
    sso.world.set("registration.mode", json!("open")).await;
    let narrowed = sso
        .world
        .app
        .call(AdminIdentityProviderUpdate, &[&sso.made.id])
        .token(sso.admin.token())
        .body(&IdentityProviderUpdateBody {
            allowed_email_domains: Some(vec!["corp.example".to_owned()]),
            expected_revision: sso.made.revision,
            ..IdentityProviderUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        sso.sign_in(&mut browser, None).await,
        "/login?error=domain_not_allowed"
    );
    sso.world
        .app
        .call(AdminIdentityProviderUpdate, &[&sso.made.id])
        .token(sso.admin.token())
        .body(&IdentityProviderUpdateBody {
            allowed_email_domains: Some(Vec::new()),
            auto_register: Some(false),
            expected_revision: narrowed.revision,
            ..IdentityProviderUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(
        sso.sign_in(&mut browser, None).await,
        "/login?error=registration_closed"
    );
    sso.provider.refuse_next();
    assert_eq!(sso.sign_in(&mut browser, None).await, "/login?error=denied");
    assert_eq!(sso.user_named("dana").await, None);
    assert!(!browser.has("orch_session"));
}

#[tokio::test]
async fn the_test_of_a_provider_says_what_it_found_or_what_went_wrong() {
    let sso = sso().await;
    let found = sso
        .world
        .app
        .call(AdminIdentityProviderTest, &[&sso.made.id])
        .token(sso.admin.token())
        .send()
        .await
        .ok();
    assert!(found.ok, "{:?}", found.error);
    let metadata = found.issuer_metadata.unwrap();
    assert_eq!(metadata["issuer"], json!(sso.provider.issuer()));
    assert_eq!(metadata["signing_keys"], json!(1));

    let moved = sso
        .world
        .app
        .call(AdminIdentityProviderUpdate, &[&sso.made.id])
        .token(sso.admin.token())
        .body(&IdentityProviderUpdateBody {
            issuer: Some(format!("{}/realms/elsewhere", sso.provider.issuer())),
            expected_revision: sso.made.revision,
            ..IdentityProviderUpdateBody::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(moved.revision, sso.made.revision + 1);
    let failed = sso
        .world
        .app
        .call(AdminIdentityProviderTest, &[&sso.made.id])
        .token(sso.admin.token())
        .send()
        .await
        .ok();
    assert!(!failed.ok);
    assert!(failed.issuer_metadata.is_none());
    assert!(
        failed.error.as_deref().is_some_and(
            |error| error.starts_with("Could not read the issuer's discovery document")
        ),
        "{:?}",
        failed.error
    );

    let nobody = sso.world.person("nobody").await;
    let signed = sso.world.sign_in(&nobody).await;
    let refused = sso
        .world
        .app
        .call(AdminIdentityProviderTest, &[&sso.made.id])
        .token(signed.token())
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 403);
}

#[tokio::test]
async fn oidc_keeps_system_admission_and_explicit_team_acceptance_separate_from_default_enrollment()
{
    use orch_api_contract::nodes::{OrgCreate, OrgCreateBody};
    use orch_api_contract::users::{InvitationCreate, InvitationCreateBody, InvitationKind};
    for (accept_team, personal) in [(false, true), (true, true), (false, false), (true, false)] {
        let sso = sso().await;
        let mut teams = Vec::new();
        for (name, slug) in [
            ("Default team", "default-team"),
            ("Invited team", "invited-team"),
        ] {
            teams.push(
                sso.world
                    .app
                    .call(OrgCreate, &[])
                    .token(sso.admin.token())
                    .body(&OrgCreateBody {
                        name: name.to_owned(),
                        slug: slug.to_owned(),
                    })
                    .send()
                    .await
                    .ok()
                    .base
                    .id,
            );
        }
        sso.world
            .app
            .call(AdminIdentityProviderUpdate, &[&sso.made.id])
            .token(sso.admin.token())
            .body(&IdentityProviderUpdateBody {
                default_org_id: Some(Some(teams[0])),
                expected_revision: sso.made.revision,
                ..IdentityProviderUpdateBody::default()
            })
            .send()
            .await
            .ok();
        sso.world.set("registration.mode", json!("invite")).await;
        sso.world
            .set("orgs.personal_enabled", json!(personal))
            .await;
        let offer = sso
            .world
            .app
            .call(InvitationCreate, &[])
            .token(sso.admin.token())
            .body(&InvitationCreateBody {
                kind: if accept_team {
                    InvitationKind::Org
                } else {
                    InvitationKind::System
                },
                org_id: accept_team.then_some(teams[1]),
                node_id: None,
                chat_id: None,
                role: None,
                permissions: None,
                email: None,
                max_uses: Some(1),
                expires_in_hours: None,
                note: None,
            })
            .send()
            .await
            .ok();
        sso.provider.sign_in_as(dana());
        let mut browser = Browser::default();
        let address = sso
            .begin_accepting(&mut browser, Some("/"), Some(&offer.token), accept_team)
            .await;
        let back = sso.provider.authorize(&address).await.unwrap();
        let landed = sso.come_back(&mut browser, &back).await;
        let destination = if accept_team {
            "/org/invited-team".to_owned()
        } else {
            "/".to_owned()
        };
        assert!(
            landed == destination,
            "the callback must complete its requested workflow"
        );
        let user = sso
            .user_named("dana")
            .await
            .expect("the invited identity is registered");
        let home: Option<Id> = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
            .bind(user)
            .fetch_one(&sso.world.db.pool)
            .await
            .unwrap();
        assert_eq!(home.is_some(), personal);
        let domains:Vec<Id>=sqlx::query_scalar(
            "SELECT m.org_id FROM org_members m JOIN orgs o ON o.node_id=m.org_id WHERE m.user_id=$1 AND o.org_kind='team' ORDER BY m.org_id",
        ).bind(user).fetch_all(&sso.world.db.pool).await.unwrap();
        assert_eq!(domains, if accept_team { vec![teams[1]] } else { vec![] });
        let (uses,receipts):(i32,i64)=sqlx::query_as(
            "SELECT uses,(SELECT count(*) FROM invitation_uses WHERE invitation_id=$1 AND user_id=$2) FROM invitations WHERE id=$1",
        ).bind(offer.invitation.id).bind(user).fetch_one(&sso.world.db.pool).await.unwrap();
        assert_eq!((uses, receipts), (1, 1));
        let mut another = Browser::default();
        assert_eq!(sso.sign_in(&mut another, None).await, "/");
        assert_eq!(sso.user_named("dana2").await, None);
        let after:Vec<Id>=sqlx::query_scalar(
            "SELECT m.org_id FROM org_members m JOIN orgs o ON o.node_id=m.org_id WHERE m.user_id=$1 AND o.org_kind='team' ORDER BY m.org_id",
        ).bind(user).fetch_all(&sso.world.db.pool).await.unwrap();
        assert_eq!(
            after, domains,
            "ordinary later sign-in neither joins nor creates another identity"
        );
    }
}
