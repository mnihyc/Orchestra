//! Getting an identity and getting it back: what a client may know, the first setup, registration, the confirmation of an address, recovery, refreshing and ending a session.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::{DateTime, Duration, Utc};
use orch_api_contract::auth::{
    AuthEmailVerify, AuthEmailVerifyBody, AuthEmailVerifyResend, AuthEmailVerifyResendBody,
    AuthLogout, AuthRecoveryConfirm, AuthRecoveryConfirmBody, AuthRecoveryStart,
    AuthRecoveryStartBody, AuthRefresh, AuthRegister, AuthRegisterBody, AuthRegisterReply,
    AuthResult, BannerLevel, LinkTrust, MeGet, MetaGet, RegistrationMode, SetupCreate,
    SetupCreateBody, VerificationRequired,
};
use orch_api_contract::common::{CSRF_HEADER, ErrorCode};
use orch_api_contract::nodes::{OrgCreate, OrgCreateBody};
use orch_api_contract::users::{Locale, TimezoneMode};
use orch_api_core::testing::Reply;
use orch_db::queries::identity::{self, NewEmailToken};
use orch_types::Id;
use serde_json::json;
use sha2::{Digest, Sha256};
use support::{PASSWORD, STRANGER, Statements, World, cookie, told};

// Reproduce the default Tokio worker stack, even when the test runner has
// RUST_MIN_STACK set higher for other debug programs.
const SIGN_IN_STACK_BYTES: usize = 2 * 1024 * 1024;

fn hash(text: &str) -> Vec<u8> {
    Sha256::digest(text.as_bytes()).to_vec()
}

async fn register(
    world: &World,
    username: &str,
    email: Option<&str>,
    invitation: Option<&str>,
    ip: &str,
) -> Reply<AuthRegisterReply> {
    world
        .app
        .call(AuthRegister, &[])
        .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0))
        .body(&AuthRegisterBody {
            accept_invitation: false,
            username: username.to_owned(),
            display_name: "Ada Lovelace".to_owned(),
            password: PASSWORD.to_owned(),
            email: email.map(str::to_owned),
            invitation: invitation.map(str::to_owned),
            timezone: None,
        })
        .send()
        .await
}

fn registered(reply: Reply<AuthRegisterReply>) -> AuthResult {
    match reply.ok() {
        AuthRegisterReply::Registered(orch_api_contract::auth::AuthRegisterResult {
            auth: result,
            ..
        }) => result,
        other => panic!("not signed in: {other:?}"),
    }
}

async fn setup(world: &World, username: &str) -> Reply<AuthResult> {
    world
        .app
        .call(SetupCreate, &[])
        .body(&SetupCreateBody {
            username: username.to_owned(),
            display_name: "Root".to_owned(),
            password: PASSWORD.to_owned(),
            email: None,
            timezone: None,
        })
        .send()
        .await
}

/// An invitation to the system, for whoever holds the token.
async fn invitation(
    world: &World,
    token: &str,
    email: Option<&str>,
    ends: DateTime<Utc>,
    uses: i32,
    revoked: bool,
) {
    let maker = world
        .person(&format!(
            "m{}",
            &orch_types::new_id().simple().to_string()[20..]
        ))
        .await;
    world.make_admin(&maker).await;
    sqlx::query(
        "INSERT INTO invitations (id, token_hash, kind, email, max_uses, uses, expires_at, revoked_at, created_by)
         VALUES ($1, $2, 'system', $3, 1, $4, $5, $6, $7)",
    )
    .bind(orch_types::new_id())
    .bind(hash(token))
    .bind(email)
    .bind(uses)
    .bind(ends)
    .bind(revoked.then(support::start))
    .bind(maker.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
}

async fn refresh(world: &World, cookies: &str) -> Reply<AuthResult> {
    world
        .app
        .call(AuthRefresh, &[])
        .header("cookie", cookies)
        .header(CSRF_HEADER, "1")
        .send()
        .await
}

async fn recover(world: &World, email: &str, ip: &str) -> Reply<()> {
    world
        .app
        .call(AuthRecoveryStart, &[])
        .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0))
        .body(&AuthRecoveryStartBody {
            email: email.to_owned(),
        })
        .send()
        .await
}

async fn reset(world: &World, token: &str, password: &str) -> Reply<()> {
    world
        .app
        .call(AuthRecoveryConfirm, &[])
        .body(&AuthRecoveryConfirmBody {
            token: token.to_owned(),
            password: password.to_owned(),
        })
        .send()
        .await
}

async fn verify(world: &World, token: &str) -> Reply<()> {
    world
        .app
        .call(AuthEmailVerify, &[])
        .body(&AuthEmailVerifyBody {
            token: token.to_owned(),
        })
        .send()
        .await
}

async fn resend(world: &World, login: &str, ip: &str) -> Reply<()> {
    world
        .app
        .call(AuthEmailVerifyResend, &[])
        .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0))
        .body(&AuthEmailVerifyResendBody {
            login: login.to_owned(),
        })
        .send()
        .await
}

async fn token_of(world: &World, purpose: &str, to: &str) -> String {
    world
        .mails(purpose, to)
        .await
        .last()
        .map(|row| row["data"]["token"].as_str().unwrap().to_owned())
        .expect("a link was mailed")
}

#[tokio::test]
async fn the_public_metadata_tells_what_a_client_may_know_before_anybody_has_signed_in() {
    let world = World::new().await;
    let meta = world.app.call(MetaGet, &[]).send().await.ok();
    assert_eq!(
        (
            meta.name.as_str(),
            meta.version.as_str(),
            meta.build.as_str()
        ),
        ("Orch", "0.0.0-test", "1")
    );
    assert!(meta.needs_setup && meta.local_login && !meta.mail && !meta.email_code_login);
    assert_eq!(meta.registration_mode, RegistrationMode::Invite);
    assert_eq!(meta.locales, vec![Locale::En, Locale::ZhCn]);
    assert!(
        meta.banner.is_none()
            && meta.push_public_key.is_none()
            && meta.identity_providers.is_empty()
    );
    assert_eq!(
        meta.link_endpoints.len(),
        1,
        "the front is accepted but not advertised by default"
    );
    assert_eq!(meta.link_endpoints[0].trust, LinkTrust::Pinned);
    assert_eq!(meta.link_endpoints[0].url, "wss://gateway.test:9443");
    assert_eq!(meta.limits.max_input_chars, 100_000);
    assert_eq!(meta.limits.max_image_bytes, 20 * 1024 * 1024);
    assert_eq!(meta.limits.max_upload_bytes, 8 * 1024 * 1024 * 1024);

    // Once there is a person, a mail server, a way in by a code, a notice and a provider.
    world.mail_server().await;
    world.set("registration.mode", json!("open")).await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    let until = world.now() + Duration::hours(1);
    world
        .set(
            "system.banner",
            json!({ "text": "Maintenance tonight", "level": "warning", "until": until }),
        )
        .await;
    sqlx::query(
        "INSERT INTO identity_providers (id, key, name, issuer, client_id) VALUES ($1, 'corp', 'Corp', 'https://idp.example', 'orch')",
    )
    .bind(orch_types::new_id())
    .execute(&world.db.pool)
    .await
    .unwrap();
    let meta = world.app.call(MetaGet, &[]).send().await.ok();
    assert!(!meta.needs_setup && meta.mail && meta.email_code_login);
    assert_eq!(meta.registration_mode, RegistrationMode::Open);
    assert_eq!(meta.identity_providers[0].key, "corp");
    assert!(!meta.identity_providers[0].auto_register);
    sqlx::query("UPDATE identity_providers SET auto_register=true WHERE key='corp'")
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert!(
        world
            .app
            .call(MetaGet, &[])
            .send()
            .await
            .ok()
            .identity_providers[0]
            .auto_register
    );
    let banner = meta.banner.unwrap();
    assert_eq!(
        (banner.text.as_str(), banner.level),
        ("Maintenance tonight", BannerLevel::Warning)
    );
    // A notice that has ended is not shown.
    world.advance(Duration::hours(2));
    assert!(
        world
            .app
            .call(MetaGet, &[])
            .send()
            .await
            .ok()
            .banner
            .is_none()
    );
    world.set("auth.local_login", json!(false)).await;
    assert!(!world.app.call(MetaGet, &[]).send().await.ok().local_login);
}

#[tokio::test]
async fn the_first_setup_makes_the_first_administrator_with_a_session_and_works_only_once() {
    let world = World::new().await;
    let reply = setup(&world, "root").await;
    assert_eq!(reply.status, 200);
    assert!(cookie(&reply.headers, "__Host-orch_session").is_some());
    let result = reply.ok();
    assert!(result.user.is_system_admin);
    assert_eq!(result.user.user.username, "root");
    assert_eq!(result.user.timezone_mode, TimezoneMode::Device);
    assert_eq!(result.user.timezone, "UTC");
    assert!(!world.app.call(MetaGet, &[]).send().await.ok().needs_setup);
    let me = world
        .app
        .call(MeGet, &[])
        .token(&result.access_token)
        .send()
        .await
        .ok();
    assert_eq!(me.personal_org_id, result.user.personal_org_id);

    let again = setup(&world, "second").await.problem();
    assert_eq!((again.status, again.code), (409, ErrorCode::ConflictExists));
    // Nothing was made by the second.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn two_setups_at_once_make_one_administrator() {
    let world = World::new().await;
    let (first, second) = tokio::join!(setup(&world, "root"), setup(&world, "toor"));
    let mut statuses = [first.status.as_u16(), second.status.as_u16()];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 409]);
    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE is_system_admin")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(admins, 1);
}

#[test]
fn the_first_password_sign_in_after_setup_works_on_a_two_mib_stack() {
    std::thread::Builder::new()
        .name("setup-then-login".to_owned())
        .stack_size(SIGN_IN_STACK_BYTES)
        .spawn(|| {
            // A current-thread runtime polls the real router on this exact
            // thread, rather than a worker with the runner's larger stack.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(Box::pin(async {
                let world = World::new().await;
                let setup = setup(&world, "root").await;
                assert_eq!(setup.status, 200);
                let setup_cookie = cookie(&setup.headers, "__Host-orch_session").unwrap();
                let administrator = setup.ok().user.user.id;

                // This is the first request after setup and carries no session.
                let signed_in = world.password_login("root", PASSWORD, STRANGER, "").await;
                assert_eq!(signed_in.status, 200);
                let session = cookie(&signed_in.headers, "__Host-orch_session").unwrap();
                assert_ne!(session.value, setup_cookie.value, "signing in opens a fresh session");
                let result = signed_in.ok();
                assert_eq!(result.user.user.id, administrator);
                assert!(result.user.is_system_admin);

                assert!(!world.app.call(orch_api_contract::auth::MeAdminModeGet, &[])
                    .token(&result.access_token).send().await.ok().as_admin);
                world.app.call(orch_api_contract::auth::MeAdminModeSet, &[]).token(&result.access_token)
                    .body(&orch_api_contract::auth::AdminModeSetBody { as_admin: true }).send().await.ok();

                // The first content write also renders its new org and creates General.
                // This used to carry the full thread-rendering and private-page futures inline.
                let org = world.app.call(OrgCreate, &[]).token(&result.access_token)
                    .body(&OrgCreateBody { name: "Acme".to_owned(), slug: "acme".to_owned() })
                    .send().await.ok();
                assert_eq!(org.base.name, "Acme");
                assert_eq!(org.base.owner.id, administrator);
                assert!(org.base.access.permissions.iter().any(|permission| permission == "org.manage"));
                let general: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1 AND kind = 'project' AND name = 'General'")
                    .bind(org.base.id).fetch_one(&world.db.pool).await.unwrap();
                assert_eq!(general, 1, "a successful org starts with its General project");

                let me = world.app.call(MeGet, &[]).token(&result.access_token)
                    .send().await.ok();
                assert_eq!(me.user.id, administrator);
                assert!(me.is_system_admin, "the new token authenticates the administrator");
                let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id = $1")
                    .bind(administrator).fetch_one(&world.db.pool).await.unwrap();
                assert_eq!(sessions, 2, "setup and the first sign-in each open one session");
            }));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test]
async fn a_setup_with_what_does_not_fit_is_refused_and_leaves_the_system_to_be_set_up() {
    let world = World::new().await;
    let refused = world
        .app
        .call(SetupCreate, &[])
        .body(&SetupCreateBody {
            username: "Root".to_owned(),
            display_name: String::new(),
            password: "short".to_owned(),
            email: Some("not an address".to_owned()),
            timezone: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let paths: Vec<_> = refused.extra["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fault| fault["path"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(paths, ["username", "display_name", "email", "password"]);
    assert!(world.app.call(MetaGet, &[]).send().await.ok().needs_setup);
}

#[tokio::test]
async fn registering_makes_a_person_with_an_org_of_their_own_and_signs_them_in() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let reply = register(&world, "ada", None, None, STRANGER).await;
    assert_eq!(reply.status, 201);
    let headers = reply.headers.clone();
    let result = registered(reply);
    assert!(
        cookie(&headers, "__Host-orch_session").is_some()
            && cookie(&headers, "__Host-orch_device").is_some()
    );
    let user = result.user.user.id;
    let org = result
        .user
        .personal_org_id
        .expect("personal organizations are enabled as shipped");
    assert_eq!(result.user.user.avatar.text, "A");
    assert!(!result.user.is_system_admin && result.user.has_password);
    assert_eq!(result.user.timezone_mode, TimezoneMode::Device);
    assert_eq!(result.user.timezone, "UTC");

    // The org: a root org whose short name is the username, closed and flat as the policies ship, empty.
    let (slug, kind, closed, flat, name): (String, String, bool, bool, String) = sqlx::query_as(
        "SELECT o.slug, o.org_kind, o.closed, o.flat, n.name FROM orgs o JOIN nodes n ON n.id = o.node_id WHERE o.node_id = $1",
    )
    .bind(org)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (slug.as_str(), kind.as_str(), closed, flat, name.as_str()),
        ("ada", "personal", true, true, "Ada Lovelace")
    );
    let (owner, root, parent): (Id, Id, Option<Id>) =
        sqlx::query_as("SELECT owner_id, org_id, parent_id FROM nodes WHERE id = $1")
            .bind(org)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!((owner, root, parent), (user, org, None));
    let children: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1")
        .bind(org)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(children, 0);
    // They are its one member, and its owner.
    let members: Vec<(Id, Id)> =
        sqlx::query_as("SELECT user_id, domain_id FROM org_members WHERE org_id = $1")
            .bind(org)
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(members, vec![(user, org)]);
    let owners: Vec<(String, Option<Id>)> = sqlx::query_as(
        "SELECT holder_kind, holder_id FROM permission_entries WHERE object_kind = 'node' AND object_id = $1 AND permission = 'access.own' AND removed_at IS NULL",
    )
    .bind(org)
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(owners, vec![("user".to_owned(), Some(user))]);
    // The password is kept as an Argon2id hash; the epoch moved; the registration is audited.
    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    let epoch: i64 = sqlx::query_scalar("SELECT epoch FROM authz_state")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(epoch >= 1);
    let audit = world.audit().await;
    assert!(
        audit.contains(&"user.registered".to_owned()) && audit.contains(&"auth.login".to_owned())
    );
}

#[tokio::test]
async fn a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let register_asking = |username: &str, languages: &str| {
        world
            .app
            .call(AuthRegister, &[])
            .header("accept-language", languages)
            .body(&AuthRegisterBody {
                accept_invitation: false,
                username: username.to_owned(),
                display_name: "Ada".to_owned(),
                password: PASSWORD.to_owned(),
                email: None,
                invitation: None,
                timezone: None,
            })
            .send()
    };
    let chinese = registered(register_asking("ada", "zh-CN,zh;q=0.9,en;q=0.8").await);
    assert_eq!(chinese.user.locale, Locale::ZhCn);
    let english = registered(register_asking("bob", "fr, en;q=0.5").await);
    assert_eq!(english.user.locale, Locale::En);
    let none = registered(register(&world, "carl", None, None, STRANGER).await);
    assert_eq!(none.user.locale, Locale::En);
}

#[tokio::test]
async fn the_org_of_a_new_person_is_made_as_the_systems_policies_say() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    world.set("orgs.personal_closed", json!(false)).await;
    world.set("orgs.personal_flat", json!(false)).await;
    let result = registered(register(&world, "ada", None, None, STRANGER).await);
    let (closed, flat): (bool, bool) =
        sqlx::query_as("SELECT closed, flat FROM orgs WHERE node_id = $1")
            .bind(result.user.personal_org_id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!((closed, flat), (false, false));
}

#[tokio::test]
async fn registration_follows_its_mode_and_an_invitation_that_is_still_good() {
    let world = World::new().await;
    let later = world.now() + Duration::days(7);
    invitation(&world, "oinv_good", None, later, 0, false).await;
    invitation(
        &world,
        "oinv_ended",
        None,
        world.now() - Duration::hours(1),
        0,
        false,
    )
    .await;
    invitation(&world, "oinv_taken_back", None, later, 0, true).await;
    invitation(&world, "oinv_used_up", None, later, 1, false).await;

    // As shipped, by invitation.
    for token in [
        None,
        Some("oinv_unknown"),
        Some("oinv_ended"),
        Some("oinv_taken_back"),
        Some("oinv_used_up"),
    ] {
        let refused = register(&world, "ada", None, token, STRANGER)
            .await
            .problem();
        assert_eq!(
            (refused.status, refused.code),
            (403, ErrorCode::RegistrationInviteRequired),
            "{token:?}"
        );
    }
    assert_eq!(
        register(&world, "ada", None, Some("oinv_good"), STRANGER)
            .await
            .status,
        201
    );

    // Closed, nobody registers: not with an invitation either.
    world.set("registration.mode", json!("closed")).await;
    let closed = register(&world, "bob", None, Some("oinv_good"), STRANGER)
        .await
        .problem();
    assert_eq!(
        (closed.status, closed.code),
        (403, ErrorCode::RegistrationClosed)
    );

    // Open, anybody does; an invitation that ended opens nothing, and is not needed.
    world.set("registration.mode", json!("open")).await;
    assert_eq!(
        register(&world, "bob", None, Some("oinv_ended"), STRANGER)
            .await
            .status,
        201
    );
    assert_eq!(
        register(&world, "carl", None, None, STRANGER).await.status,
        201
    );
}

#[tokio::test]
async fn a_personal_link_is_for_the_address_it_was_sent_to_and_that_address_counts_as_confirmed() {
    let world = World::new().await;
    let later = world.now() + Duration::days(7);
    invitation(
        &world,
        "oinv_for_ada",
        Some("ada@example.org"),
        later,
        0,
        false,
    )
    .await;
    let wrong = register(
        &world,
        "ada",
        Some("someone@example.org"),
        Some("oinv_for_ada"),
        STRANGER,
    )
    .await
    .problem();
    assert_eq!(
        (wrong.status, wrong.code),
        (403, ErrorCode::RegistrationInviteRequired)
    );
    // The address it was made for, in any case; and the form that asks for no other.
    let result = registered(
        register(
            &world,
            "ada",
            Some("Ada@Example.org"),
            Some("oinv_for_ada"),
            STRANGER,
        )
        .await,
    );
    assert!(result.user.email_verified);
    assert_eq!(result.user.timezone_mode, TimezoneMode::Device);
    invitation(
        &world,
        "oinv_for_bob",
        Some("bob@example.org"),
        later,
        0,
        false,
    )
    .await;
    let result = registered(register(&world, "bob", None, Some("oinv_for_bob"), STRANGER).await);
    assert_eq!(result.user.email.as_deref(), Some("bob@example.org"));
    assert!(result.user.email_verified);
    assert_eq!(result.user.timezone_mode, TimezoneMode::Device);
}

#[tokio::test]
async fn a_registration_that_must_be_confirmed_waits_for_the_link_which_then_signs_in() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("registration.mode", json!("open")).await;
    // Confirmation is asked for where there is a mail server, as shipped: no setting was written.
    let none = register(&world, "ada", None, None, STRANGER)
        .await
        .problem();
    assert_eq!(none.code, ErrorCode::RequestInvalid);
    let reply = register(&world, "ada", Some("ada@example.org"), None, STRANGER).await;
    assert_eq!(reply.status, 202);
    assert!(cookie(&reply.headers, "__Host-orch_session").is_none());
    assert_eq!(
        reply.ok(),
        AuthRegisterReply::Pending(VerificationRequired {
            verification_required: true
        })
    );
    // The link is a row of the outbox, made with the person.
    let mails = world.mails("verify_email", "ada@example.org").await;
    assert_eq!(mails.len(), 1);
    let token = token_of(&world, "verify_email", "ada@example.org").await;
    assert!(token.starts_with("oem_"));
    assert_eq!(
        mails[0]["data"]["url"],
        format!("https://orch.test/verify-email/{token}")
    );
    // Until it is used, nobody signs in.
    let unverified = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(
        (unverified.status, unverified.code),
        (403, ErrorCode::AuthEmailUnverified)
    );

    let unknown = verify(&world, "oem_nobody").await.problem();
    assert_eq!((unknown.status, unknown.code), (410, ErrorCode::Gone));
    let done = verify(&world, &token).await;
    assert_eq!(done.status, 204);
    let session =
        cookie(&done.headers, "__Host-orch_session").expect("the link signs the browser in");
    let device = cookie(&done.headers, "__Host-orch_device").unwrap();
    let signed = refresh(&world, &format!("__Host-orch_session={}", session.value))
        .await
        .ok();
    assert!(signed.user.email_verified);
    assert!(device.value.starts_with("odv_"));
    // One use.
    let twice = verify(&world, &token).await.problem();
    assert_eq!(twice.code, ErrorCode::Gone);
    assert_eq!(
        world
            .password_login("ada", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn a_link_to_confirm_an_address_ends_after_a_day() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("registration.mode", json!("open")).await;
    register(&world, "ada", Some("ada@example.org"), None, STRANGER).await;
    let token = token_of(&world, "verify_email", "ada@example.org").await;
    world.advance(Duration::hours(24) + Duration::seconds(1));
    assert_eq!(verify(&world, &token).await.problem().code, ErrorCode::Gone);
}

#[tokio::test]
async fn where_the_system_has_no_mail_server_nothing_waits_to_be_confirmed() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    world
        .set("registration.email_verification", json!("required"))
        .await;
    let result = registered(register(&world, "ada", Some("ada@example.org"), None, STRANGER).await);
    // It is stored as given, and not confirmed.
    assert_eq!(result.user.email.as_deref(), Some("ada@example.org"));
    assert!(!result.user.email_verified);
    assert!(world.outbox().await.is_empty());
}

#[tokio::test]
async fn a_confirmation_link_can_be_asked_for_again_and_answers_alike_for_every_name() {
    let world = World::new().await;
    world.mail_server().await;
    world.person_with("bob", PASSWORD, false).await; // not confirmed
    world.person("ada").await; // confirmed already
    let carl = world.person_with("carl", PASSWORD, false).await;
    world.suspend(&carl).await;
    resend(&world, "warm-up", "198.51.100.1").await;

    let mut answers = Vec::new();
    for (index, login) in ["ghost", "bob", "ada", "carl", "bob@example.org"]
        .into_iter()
        .enumerate()
    {
        let before = Statements::now();
        let reply = resend(&world, login, &format!("198.51.100.{}", 10 + index)).await;
        let used = Statements::now() - before;
        assert_eq!(
            (reply.status.as_u16(), reply.raw.is_empty()),
            (202, true),
            "{login}"
        );
        answers.push(used);
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );
    // Only the person whose address waits was mailed, and once in the minute.
    assert_eq!(
        world.mails("verify_email", "bob@example.org").await.len(),
        1
    );
    assert_eq!(world.outbox().await.len(), 1);
    world.advance(Duration::seconds(61));
    resend(&world, "bob", "198.51.100.30").await;
    assert_eq!(
        world.mails("verify_email", "bob@example.org").await.len(),
        2
    );
    // Either link confirms, and once it has, the other is no more.
    let token = token_of(&world, "verify_email", "bob@example.org").await;
    assert_eq!(verify(&world, &token).await.status, 204);
    let first = world.mails("verify_email", "bob@example.org").await[0]["data"]["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(verify(&world, &first).await.problem().code, ErrorCode::Gone);
}

#[tokio::test]
async fn registration_refuses_what_does_not_fit_and_names_it_all_at_once() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let refused = world
        .app
        .call(AuthRegister, &[])
        .body(&AuthRegisterBody {
            accept_invitation: false,
            username: "Ada Lovelace".to_owned(),
            display_name: "  ".to_owned(),
            password: "ada".to_owned(),
            email: Some("nowhere".to_owned()),
            invitation: None,
            timezone: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"].as_array().unwrap().len(), 4);
    // Nothing was made.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn registration_and_a_reset_refuse_a_commonly_used_password_and_make_or_spend_nothing() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let refused = world
        .app
        .call(AuthRegister, &[])
        .body(&AuthRegisterBody {
            accept_invitation: false,
            username: "ada".to_owned(),
            display_name: "Ada Lovelace".to_owned(),
            password: "QwertyUiop".to_owned(),
            email: None,
            invitation: None,
            timezone: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        (refused.status, refused.code),
        (400, ErrorCode::AuthPasswordCommon)
    );
    assert_eq!(refused.extra["field"], "password");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "nothing was made");

    // A link that sets a new password is refused the common one, and stays good for another.
    world.mail_server().await;
    world.person("bob").await;
    recover(&world, "bob@example.org", "198.51.100.7").await;
    let token = token_of(&world, "reset_password", "bob@example.org").await;
    let common = reset(&world, &token, "password1234").await.problem();
    assert_eq!(
        (common.status, common.code),
        (400, ErrorCode::AuthPasswordCommon)
    );
    assert_eq!(
        reset(&world, &token, "a phrase of my own choosing")
            .await
            .status
            .as_u16(),
        204
    );
}

#[tokio::test]
async fn a_username_or_an_address_that_is_taken_is_a_conflict_and_makes_nothing() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    registered(register(&world, "ada", Some("ada@example.org"), None, STRANGER).await);
    let name = register(&world, "ada", Some("other@example.org"), None, STRANGER)
        .await
        .problem();
    assert_eq!((name.status, name.code), (409, ErrorCode::ConflictExists));
    let address = register(&world, "bob", Some("ADA@example.org"), None, STRANGER)
        .await
        .problem();
    assert_eq!(
        (address.status, address.code),
        (409, ErrorCode::ConflictExists)
    );
    // Usernames and the short names of root orgs are one namespace.
    let acme = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        let owner = identity::user_by_login(&mut conn, "ada")
            .await
            .unwrap()
            .unwrap()
            .id;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, acme, "org", None, "Acme", owner,
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE orgs SET slug = 'acme' WHERE node_id = $1")
        .bind(acme)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let org_name = register(&world, "acme", None, None, STRANGER)
        .await
        .problem();
    assert_eq!(
        (org_name.status, org_name.code),
        (409, ErrorCode::ConflictExists)
    );
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(users, 1);
    let nodes: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(nodes, 2);
}

#[tokio::test]
async fn ten_registrations_an_hour_may_be_asked_of_one_address() {
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    // Requests that fail at once cost nothing to make, and count all the same.
    let world = &world;
    let invalid = |ip: &'static str| async move {
        world
            .app
            .call(AuthRegister, &[])
            .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0))
            .body(&AuthRegisterBody {
                accept_invitation: false,
                username: "x".to_owned(),
                display_name: "X".to_owned(),
                password: "short".to_owned(),
                email: None,
                invitation: None,
                timezone: None,
            })
            .send()
            .await
    };
    for _ in 0..10 {
        assert_eq!(
            invalid(STRANGER).await.problem().code,
            ErrorCode::RequestInvalid
        );
    }
    let eleventh = invalid(STRANGER).await;
    assert_eq!(eleventh.status, 429);
    assert_eq!(eleventh.headers["retry-after"], "3600");
    assert_eq!(eleventh.problem().code, ErrorCode::RateLimited);
    // Another address is another count, and an hour is an hour.
    assert_eq!(
        invalid("203.0.113.9").await.problem().code,
        ErrorCode::RequestInvalid
    );
    world.advance(Duration::hours(1) + Duration::seconds(1));
    assert_eq!(
        invalid(STRANGER).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn recovery_mails_a_link_to_the_person_whose_confirmed_address_it_is_and_answers_alike_for_every_address()
 {
    let world = World::new().await;
    world.mail_server().await;
    world.person("ada").await;
    world.person_with("bob", PASSWORD, false).await; // an address that is not confirmed
    let carl = world.person("carl").await;
    world.suspend(&carl).await;
    recover(&world, "warm-up@example.org", "198.51.100.1").await;

    let mut answers = Vec::new();
    for (index, email) in [
        "ghost@example.org",
        "ada@example.org",
        "bob@example.org",
        "carl@example.org",
        "ADA@example.org",
    ]
    .into_iter()
    .enumerate()
    {
        let before = Statements::now();
        let reply = recover(&world, email, &format!("198.51.100.{}", 10 + index)).await;
        let used = Statements::now() - before;
        assert_eq!(
            (reply.status.as_u16(), reply.raw.is_empty()),
            (202, true),
            "{email}"
        );
        answers.push(used);
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );

    // One link, to the one confirmed address that is a person's and active; the address in
    // another case is the same address, and a minute had not passed.
    let outbox = world.outbox().await;
    assert_eq!(outbox.len(), 1, "{outbox:?}");
    assert_eq!(outbox[0]["purpose"], "reset_password");
    let token = token_of(&world, "reset_password", "ada@example.org").await;
    assert_eq!(
        outbox[0]["data"]["url"],
        format!("https://orch.test/reset/{token}")
    );
    // Only a hash of it is kept.
    let kept: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_tokens WHERE token_hash = $1 AND purpose = 'reset_password'",
    )
    .bind(hash(&token))
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn where_the_system_has_no_mail_server_recovery_answers_and_sends_nothing() {
    let world = World::new().await;
    world.person("ada").await;
    assert_eq!(
        recover(&world, "ada@example.org", STRANGER).await.status,
        202
    );
    assert!(world.outbox().await.is_empty());
}

#[tokio::test]
async fn a_reset_link_sets_a_new_password_ends_every_session_and_makes_the_browser_one_they_got_in_from()
 {
    let world = World::locking_after(3).await;
    world.mail_server().await;
    let ada = world.person("ada").await;
    let first = world.sign_in(&ada).await;
    let second = world.sign_in(&ada).await;
    // Their name is locked by strangers, and this browser is new to them.
    for index in 0..3 {
        let reply = world
            .password_login(
                "ada",
                "wrong password",
                &format!("198.51.100.{}", 10 + index),
                "",
            )
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.90", "")
            .await
            .status,
        423
    );

    recover(&world, "ada@example.org", STRANGER).await;
    let token = token_of(&world, "reset_password", "ada@example.org").await;
    let weak = reset(&world, &token, "short").await.problem();
    assert_eq!((weak.status, weak.code), (400, ErrorCode::RequestInvalid));
    assert_eq!(weak.extra["errors"][0]["path"], "password");
    // A refused password leaves the link to be used.
    let done = reset(&world, &token, "a brand new password").await;
    assert_eq!(done.status, 204);
    let mark = cookie(&done.headers, "__Host-orch_device").expect("the browser becomes known");
    assert!(cookie(&done.headers, "__Host-orch_session").is_none());

    // Every session ended at once.
    for signed in [&first, &second] {
        let reply = world
            .app
            .call(MeGet, &[])
            .token(signed.token())
            .send()
            .await
            .problem();
        assert_eq!(reply.code, ErrorCode::AuthInvalidToken);
        assert_eq!(refresh_status(&world, &signed.browser()).await, 401);
    }
    // The old password is no more, the new one is, on the browser that reset it, while the name
    // stays locked for strangers: nothing clears a count but time.
    let known = format!("__Host-orch_device={}", mark.value);
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.91", &known)
            .await
            .status,
        401
    );
    assert_eq!(
        world
            .password_login("ada", "a brand new password", "198.51.100.92", &known)
            .await
            .status,
        200
    );
    let stranger = world
        .password_login("ada", "a brand new password", "198.51.100.93", "")
        .await;
    assert_eq!(stranger.problem().code, ErrorCode::AuthLocked);
    // One use.
    let again = reset(&world, &token, "another new password")
        .await
        .problem();
    assert_eq!((again.status, again.code), (410, ErrorCode::Gone));
    let audit = world.audit().await;
    assert!(audit.contains(&"auth.password_reset".to_owned()));
}

async fn refresh_status(world: &World, cookies: &str) -> u16 {
    refresh(world, cookies).await.status.as_u16()
}

#[tokio::test]
async fn a_reset_link_ends_after_half_an_hour_and_an_unknown_one_is_gone_like_a_used_one() {
    let world = World::new().await;
    world.mail_server().await;
    world.person("ada").await;
    recover(&world, "ada@example.org", STRANGER).await;
    let token = token_of(&world, "reset_password", "ada@example.org").await;
    world.advance(Duration::minutes(30) + Duration::seconds(1));
    let late = reset(&world, &token, "a brand new password")
        .await
        .problem();
    assert_eq!((late.status, late.code), (410, ErrorCode::Gone));
    let unknown = reset(&world, "oem_nobody", "a brand new password")
        .await
        .problem();
    assert_eq!(told(&unknown), told(&late));
}

#[tokio::test]
async fn a_reset_link_a_system_admin_made_for_a_lost_second_step_switches_it_off_too() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["second_step"])).await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world.authenticator(&signed).await;
    {
        let mut conn = world.conn().await;
        identity::set_email_code_second_step(&mut conn, ada.id, Some(world.now()), world.now())
            .await
            .unwrap();
        identity::create_email_token(
            &mut conn,
            &NewEmailToken {
                id: orch_types::new_id(),
                user_id: ada.id,
                purpose: "reset_password",
                token_hash: &hash("oem_made_by_an_admin"),
                email: "ada@example.org",
                clear_second_step: true,
                at: world.now(),
                expires_at: world.now() + Duration::minutes(30),
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(
        reset(&world, "oem_made_by_an_admin", "a brand new password")
            .await
            .status,
        204
    );
    let me = world
        .password_login("ada", "a brand new password", STRANGER, "")
        .await
        .ok();
    assert!(!me.user.totp_enabled && !me.user.email_code_second_step);
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.second_step_cleared".to_owned())
    );
}

#[tokio::test]
async fn a_new_access_token_is_asked_for_with_the_session_cookie_and_the_header_that_cannot_be_sent_across_sites()
 {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    // An access token lasts fifteen minutes.
    world.advance(Duration::minutes(16));
    let expired = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .problem();
    assert_eq!(
        (expired.status, expired.code),
        (401, ErrorCode::AuthInvalidToken)
    );

    let no_header = world
        .app
        .call(AuthRefresh, &[])
        .header("cookie", &signed.browser())
        .send()
        .await
        .problem();
    assert_eq!(
        (no_header.status, no_header.code),
        (403, ErrorCode::AuthCsrf)
    );
    let no_cookie = refresh(&world, "").await.problem();
    assert_eq!(
        (no_cookie.status, no_cookie.code),
        (401, ErrorCode::AuthRequired)
    );
    let unknown = refresh(&world, "__Host-orch_session=ors_nobody")
        .await
        .problem();
    assert_eq!(
        (unknown.status, unknown.code),
        (401, ErrorCode::AuthInvalidToken)
    );

    let reply = refresh(&world, &signed.browser()).await;
    assert_eq!(reply.status, 200);
    let renewed =
        cookie(&reply.headers, "__Host-orch_session").expect("its life slides with the session's");
    assert_eq!(renewed.value, signed.session);
    assert_eq!(renewed.max_age(), Some(30 * 86_400));
    assert!(cookie(&reply.headers, "__Host-orch_device").is_none());
    let fresh = reply.ok();
    assert_eq!(
        fresh.access_expires_at.get(),
        world.now() + Duration::minutes(15)
    );
    assert_ne!(fresh.access_token, signed.result.access_token);
    let me = world
        .app
        .call(MeGet, &[])
        .token(&fresh.access_token)
        .send()
        .await
        .ok();
    assert_eq!(me.user.username, "ada");
}

#[tokio::test]
async fn a_parent_domain_cookie_cannot_replace_the_host_bound_session() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let attacker = world.person("attacker").await;
    let ada = world.sign_in(&ada).await;
    let attacker = world.sign_in(&attacker).await;

    // An untrusted sibling can set the old unprefixed name for a common
    // parent domain; it must not be used even if it appears first.
    let cookies = format!(
        "orch_session={}; __Host-orch_session={}",
        attacker.session, ada.session
    );
    let refreshed = refresh(&world, &cookies).await.ok();
    assert_eq!(refreshed.user.user.username, "ada");
    assert_eq!(
        refresh(&world, &format!("orch_session={}", attacker.session))
            .await
            .problem()
            .code,
        ErrorCode::AuthRequired
    );
}

#[tokio::test]
async fn a_session_lasts_thirty_days_from_its_last_use() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    for _ in 0..3 {
        world.advance(Duration::days(29));
        assert_eq!(refresh_status(&world, &signed.browser()).await, 200);
    }
    world.advance(Duration::days(31));
    let ended = refresh(&world, &signed.browser()).await.problem();
    assert_eq!(ended.code, ErrorCode::AuthInvalidToken);
}

#[tokio::test]
async fn a_suspended_person_is_not_given_a_new_access_token() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world.suspend(&ada).await;
    let refused = refresh(&world, &signed.browser()).await.problem();
    assert_eq!(
        (refused.status, refused.code),
        (401, ErrorCode::AuthSuspended)
    );
    let by_token = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .problem();
    assert_eq!(by_token.code, ErrorCode::AuthSuspended);
}

#[tokio::test]
async fn signing_out_revokes_the_session_clears_its_cookie_and_leaves_the_mark_of_the_browser() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let reply = world
        .app
        .call(AuthLogout, &[])
        .header("cookie", &signed.browser())
        .header(CSRF_HEADER, "1")
        .send()
        .await;
    assert_eq!(reply.status, 204);
    let cleared = cookie(&reply.headers, "__Host-orch_session").unwrap();
    assert_eq!(cleared.value, "");
    assert_eq!(cleared.max_age(), Some(0));
    assert_eq!(cleared.path(), Some("/api/v1/auth"));
    assert!(cookie(&reply.headers, "__Host-orch_device").is_none());

    let token = world
        .app
        .call(MeGet, &[])
        .token(signed.token())
        .send()
        .await
        .problem();
    assert_eq!(token.code, ErrorCode::AuthInvalidToken);
    assert_eq!(refresh_status(&world, &signed.browser()).await, 401);
    // The browser is still one their person got in from.
    let known: i64 = sqlx::query_scalar("SELECT count(*) FROM known_devices WHERE mark_hash = $1")
        .bind(hash(&signed.device))
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(known, 1);
    assert!(world.audit().await.contains(&"auth.logout".to_owned()));

    // Signing out twice, or with nothing to sign out of, is as good as once.
    for cookies in [signed.browser(), String::new()] {
        let again = world
            .app
            .call(AuthLogout, &[])
            .header("cookie", &cookies)
            .header(CSRF_HEADER, "1")
            .send()
            .await;
        assert_eq!(again.status, 204);
    }
    let no_header = world.app.call(AuthLogout, &[]).send().await.problem();
    assert_eq!(no_header.code, ErrorCode::AuthCsrf);
}

#[tokio::test]
async fn setup_and_administrator_creation_support_accounts_without_personal_organizations() {
    use orch_api_contract::admin::{AdminUserCreate, AdminUserCreateBody, OrgLimitsChange};
    use orch_api_contract::auth::{AdminModeSetBody, MeAdminModeSet};
    let world = World::new().await;
    world.set("orgs.personal_enabled", json!(false)).await;
    let root = setup(&world, "operator").await.ok();
    assert_eq!(root.user.personal_org_id, None);
    world
        .app
        .call(MeAdminModeSet, &[])
        .token(&root.access_token)
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let request = AdminUserCreateBody {
        username: "managed-zero".to_owned(),
        display_name: "Managed account".to_owned(),
        email: None,
        is_system_admin: None,
        personal_org: None,
    };
    assert_eq!(
        world
            .app
            .call(AdminUserCreate, &[])
            .token(&root.access_token)
            .body(&request)
            .send()
            .await
            .status,
        200
    );
    let home: Option<Id> =
        sqlx::query_scalar("SELECT personal_org_id FROM users WHERE username='managed-zero'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(home, None);
    let mut impossible = request;
    impossible.username = "override-zero".to_owned();
    impossible.personal_org = Some(OrgLimitsChange {
        closed: Some(false),
        flat: None,
    });
    assert_eq!(
        world
            .app
            .call(AdminUserCreate, &[])
            .token(&root.access_token)
            .body(&impossible)
            .send()
            .await
            .status,
        400
    );
    let absent: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE username='override-zero'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(
        absent, 0,
        "irrelevant home overrides are refused without partial creation"
    );
}
