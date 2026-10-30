//! Signing in: the proofs, the second step, and the rules about wrong tries, locks and answers that tell nothing about a name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_api_contract::auth::{
    AuthEmailCodeSend, AuthEmailCodeSendBody, AuthEmailVerify, AuthLogin, AuthLoginBody,
    AuthRegister, MeGet, SetupCreate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::testing::Reply;
use orch_db::queries::identity::{self, testing as rows};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{PASSWORD, STRANGER, Signed, Statements, World, cookie, start, step, told};

fn only(login: &str) -> AuthLoginBody {
    AuthLoginBody {
        login: login.to_owned(),
        password: None,
        email_code: None,
        totp: None,
        recovery_code: None,
    }
}

fn with(
    login: &str,
    password: Option<&str>,
    code: Option<&str>,
    totp: Option<&str>,
) -> AuthLoginBody {
    AuthLoginBody {
        login: login.to_owned(),
        password: password.map(str::to_owned),
        email_code: code.map(str::to_owned),
        totp: totp.map(str::to_owned),
        recovery_code: None,
    }
}

/// A try from an address of its own, so that the tries of a name are never
/// held against the address.
fn address(index: usize) -> String {
    format!("198.51.100.{}", 10 + index)
}

async fn wrong_tries(world: &World, login: &str, how_many: usize) {
    for index in 0..how_many {
        let reply = world
            .password_login(login, "this is not the password", &address(index), "")
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
}

fn retry_after(reply: &Reply<impl std::fmt::Debug>) -> String {
    reply
        .headers
        .get("retry-after")
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

async fn ask_for_code(
    world: &World,
    login: &str,
    password: Option<&str>,
    ip: &str,
    cookies: &str,
) -> Reply<()> {
    let mut call = world
        .app
        .call(AuthEmailCodeSend, &[])
        .peer(std::net::SocketAddr::new(ip.parse().unwrap(), 0));
    if !cookies.is_empty() {
        call = call.header("cookie", cookies);
    }
    call.body(&AuthEmailCodeSendBody {
        login: login.to_owned(),
        password: password.map(str::to_owned),
    })
    .send()
    .await
}

async fn inbox_of(
    world: &World,
    user: orch_types::Id,
    expected: usize,
) -> Vec<(String, String, Value)> {
    let mut conn = world.conn().await;
    let inbox = rows::inbox(&mut conn, user).await.unwrap();
    assert_eq!(inbox.len(), expected, "{inbox:?}");
    inbox
}

async fn statements_of(world: &World, login: &str, password: &str, ip: &str) -> usize {
    let before = Statements::now();
    let _ = world.password_login(login, password, ip, "").await;
    Statements::now() - before
}

#[tokio::test]
async fn cross_site_forms_cannot_create_a_browser_session() {
    let world = World::new().await;
    // A text/plain form inserts '=' between its field name and value. Placed
    // inside a display name, that makes a valid setup or registration body.
    let setup =
        format!("{{\"username\":\"root\",\"display_name\":\"=\",\"password\":\"{PASSWORD}\"}}\r\n");
    let refused = world
        .app
        .call(SetupCreate, &[])
        .header("content-type", "text/plain")
        .bytes(setup.as_bytes())
        .send()
        .await;
    assert_eq!(refused.status, 400);
    assert!(refused.headers.get("set-cookie").is_none());
    let humans: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'human'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(humans, 0);

    world.person("ada").await;
    world.set("registration.mode", json!("open")).await;
    let register = format!(
        "{{\"username\":\"newuser\",\"display_name\":\"=\",\"password\":\"{PASSWORD}\"}}\r\n"
    );
    let refused = world
        .app
        .call(AuthRegister, &[])
        .header("content-type", "text/plain")
        .bytes(register.as_bytes())
        .send()
        .await;
    assert_eq!(refused.status, 400);
    assert!(refused.headers.get("set-cookie").is_none());
    let humans: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE kind = 'human'")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(humans, 1);

    // '=' can also go in an unused second-step field when the attacker has
    // no second step configured for their own account.
    let login = format!("{{\"login\":\"ada\",\"password\":\"{PASSWORD}\",\"totp\":\"=\"}}\r\n");
    let refused = world
        .app
        .call(AuthLogin, &[])
        .header("content-type", "text/plain")
        .bytes(login.as_bytes())
        .send()
        .await;
    assert_eq!(refused.status, 400);
    assert!(refused.headers.get("set-cookie").is_none());
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);

    let protected = world
        .app
        .call(AuthLogin, &[])
        .header("content-type", "text/plain")
        .header("x-orch-csrf", "1")
        .bytes(login.as_bytes())
        .send()
        .await;
    assert_eq!(protected.status, 200);
    assert!(protected.headers.get("set-cookie").is_some());

    let verify = world
        .app
        .call(AuthEmailVerify, &[])
        .header("content-type", "text/plain")
        .bytes(b"{\"token\":\"=\"}\r\n")
        .send()
        .await;
    assert_eq!(verify.status, 400);
    assert!(verify.headers.get("set-cookie").is_none());

    let accepted = world
        .app
        .call(AuthLogin, &[])
        .header("content-type", "Application/JSON; charset=UTF-8")
        .body(&with("ada", Some(PASSWORD), None, None))
        .send()
        .await;
    assert_eq!(accepted.status, 200);
    assert!(accepted.headers.get("set-cookie").is_some());
}

#[tokio::test]
async fn signing_in_gives_an_access_token_a_session_cookie_and_the_mark_of_the_browser() {
    let world = World::new().await;
    world.person("ada").await;
    let reply = world
        .password_login("ada", PASSWORD, "203.0.113.1", "")
        .await;
    assert_eq!(reply.status, 200);
    let headers = reply.headers.clone();
    // What holds a secret that is shown once is kept by no cache.
    assert_eq!(headers["cache-control"], "no-store");
    let result = reply.ok();
    assert!(result.access_token.starts_with("oat_"));
    assert_eq!(
        result.access_expires_at.get(),
        world.now() + Duration::minutes(15)
    );
    assert_eq!(result.user.user.username, "ada");

    // The attributes of the cookies, exactly: out of reach of scripts, only
    // over TLS, never sent along from another site, only to the sign-in's own
    // requests; the session for its thirty days, the mark for ninety.
    let session = cookie(&headers, "__Host-orch_session").unwrap();
    let device = cookie(&headers, "__Host-orch_device").unwrap();
    assert!(session.value.starts_with("ors_"));
    assert!(device.value.starts_with("odv_"));
    let same = ["Path=/", "HttpOnly", "Secure", "SameSite=Strict"];
    assert_eq!(
        session.attributes,
        [&["Max-Age=2592000"], &same[..]].concat()
    );
    assert_eq!(
        device.attributes,
        [&["Max-Age=7776000"], &same[..]].concat()
    );

    // The access token is who they are, and only hashes are kept of the secrets.
    let me = world
        .app
        .call(MeGet, &[])
        .token(&result.access_token)
        .send()
        .await
        .ok();
    assert_eq!(me.user.username, "ada");
    for (sql, secret) in [
        (
            "SELECT count(*) FROM sessions WHERE refresh_hash = $1",
            &session.value,
        ),
        (
            "SELECT count(*) FROM access_tokens WHERE token_hash = $1",
            &result.access_token,
        ),
        (
            "SELECT count(*) FROM known_devices WHERE mark_hash = $1",
            &device.value,
        ),
    ] {
        let found: i64 = sqlx::query_scalar(sql)
            .bind(Sha256::digest(secret.as_bytes()).to_vec())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(found, 1, "{sql}");
    }
    assert!(world.audit().await.contains(&"auth.login".to_owned()));
}

#[tokio::test]
async fn a_sign_in_takes_one_proof_or_two_and_never_an_authenticators_code_alone() {
    let world = World::new().await;
    world.person("ada").await;
    for (body, path) in [
        (only("ada"), ""),
        (with("ada", Some(""), Some(""), Some("")), ""),
        (with("ada", None, None, Some("123456")), "totp"),
        (
            with("ada", Some(PASSWORD), Some("ABCD-1234"), Some("123456")),
            "",
        ),
    ] {
        let problem = world.attempt(body, STRANGER, "").await.problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid);
        assert_eq!(problem.extra["errors"][0]["path"], path);
    }
    // Nothing was tried, so nothing was counted.
    assert!(world.buckets().await.is_empty());
}

#[tokio::test]
async fn a_wrong_password_and_a_name_nobody_has_are_answered_alike() {
    let world = World::new().await;
    world.person("ada").await;
    let mut answers = Vec::new();
    for (login, password) in [
        ("ada", "this is not the password"),
        ("nobody", PASSWORD),
        ("Ada@Example.org", "this is not the password"),
        ("nobody@example.org", PASSWORD),
    ] {
        let reply = world.password_login(login, password, STRANGER, "").await;
        let status = reply.status;
        let kind = reply.headers.get("content-type").cloned();
        let retry = retry_after(&reply);
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::AuthInvalidCredentials);
        answers.push((status, kind, retry, told(&problem)));
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );
    assert_eq!(answers[0].0, 401);
}

#[tokio::test]
async fn a_name_nobody_has_costs_the_same_statements_as_one_somebody_has() {
    let world = World::new().await;
    world.person("ada").await;
    // The first use of a statement is not what is measured.
    statements_of(&world, "ada", "wrong password", "198.51.100.1").await;
    statements_of(&world, "nobody", "wrong password", "198.51.100.2").await;

    let real = statements_of(&world, "ada", "wrong password", "198.51.100.3").await;
    let ghost = statements_of(&world, "nobody", "wrong password", "198.51.100.4").await;
    let ghost_address = statements_of(
        &world,
        "nobody@example.org",
        "wrong password",
        "198.51.100.5",
    )
    .await;
    assert_eq!(real, ghost);
    assert_eq!(real, ghost_address);
    assert!(
        real >= 4,
        "{real} statements for a try that is looked up, counted and audited"
    );
}

#[tokio::test]
async fn wrong_tries_are_counted_by_the_name_and_not_by_the_address_they_come_from() {
    let world = World::new().await;
    world.person("ada").await;
    // Eight tries, each from an address of its own: a thousand addresses give a guesser nothing.
    wrong_tries(&world, "ada", 8).await;

    // The ninth is refused unseen, right though it is, and from an address that never tried.
    let reply = world
        .password_login("ada", PASSWORD, "198.51.100.99", "")
        .await;
    assert_eq!(reply.status, 423);
    assert_eq!(retry_after(&reply), "10800");
    let problem = reply.problem();
    assert_eq!(problem.code, ErrorCode::AuthLocked);
    assert_eq!(problem.extra["way"], "password");
    // The person has one name for two spellings: their address is the same count.
    let by_address = world
        .password_login("ada@example.org", PASSWORD, "198.51.100.98", "")
        .await;
    assert_eq!(by_address.problem().code, ErrorCode::AuthLocked);
}

#[tokio::test]
async fn a_name_nobody_has_is_locked_by_the_same_tries_and_in_the_same_words() {
    let world = World::new().await;
    world.person("ada").await;
    let mut locks = Vec::new();
    for login in ["ada", "ghost"] {
        wrong_tries(&world, login, 8).await;
        let reply = world
            .password_login(login, PASSWORD, "198.51.100.99", "")
            .await;
        assert_eq!(reply.status, 423, "{login}");
        let retry = retry_after(&reply);
        locks.push((retry, told(&reply.problem())));
    }
    assert_eq!(locks[0], locks[1]);
}

#[tokio::test]
async fn each_proof_has_its_own_count() {
    let world = World::locking_after(3).await;
    world.person("ada").await;
    let bob = world.person("bob").await;
    let signed = world.sign_in(&bob).await;
    let secret = world.authenticator(&signed).await;
    world.advance(step());

    wrong_tries(&world, "ada", 3).await;
    // The password is locked at that name...
    let locked = world
        .password_login("ada", PASSWORD, "198.51.100.90", "")
        .await;
    assert_eq!(locked.problem().extra["way"], "password");
    // ...and a mailed code is not: it is tried, found wrong, and counted by itself.
    for index in 0..3 {
        let reply = world
            .attempt(
                with("ada", None, Some("ABCD-1234"), None),
                &address(index),
                "",
            )
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
    let locked = world
        .attempt(
            with("ada", None, Some("ABCD-1234"), None),
            "198.51.100.91",
            "",
        )
        .await;
    assert_eq!(locked.problem().extra["way"], "email_code");

    // An authenticator's code is a third count, looked at after a right first step.
    for index in 0..3 {
        let reply = world
            .attempt(
                with("bob", Some(PASSWORD), None, Some("000000")),
                &address(index),
                "",
            )
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthSecondStepInvalid);
    }
    let locked = world
        .attempt(
            with("bob", Some(PASSWORD), None, Some(&world.totp_code(&secret))),
            "198.51.100.92",
            "",
        )
        .await;
    assert_eq!(locked.problem().extra["way"], "totp");
    // The password at that name is not locked: it is asked for the second step.
    let asked = world
        .password_login("bob", PASSWORD, "198.51.100.93", "")
        .await;
    assert_eq!(asked.problem().code, ErrorCode::AuthSecondStepRequired);
}

#[tokio::test]
async fn a_browser_its_person_signed_in_from_is_counted_by_itself() {
    let world = World::locking_after(3).await;
    let ada = world.person("ada").await;
    let bob = world.person("bob").await;
    let known = world.sign_in(&ada).await;
    let bobs = world.sign_in(&bob).await;

    // Two wrong tries from the known browser, then three from strangers: the
    // strangers' count is their own, and fills at their third.
    for index in 0..2 {
        let reply = world
            .password_login("ada", "wrong password", &address(index), &known.mark_only())
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
    wrong_tries(&world, "ada", 3).await;
    let stranger = world
        .password_login("ada", PASSWORD, "198.51.100.90", "")
        .await;
    assert_eq!(stranger.problem().code, ErrorCode::AuthLocked);
    // Somebody else's browser is a stranger's at this name.
    let others = world
        .password_login("ada", PASSWORD, "198.51.100.91", &bobs.mark_only())
        .await;
    assert_eq!(others.problem().code, ErrorCode::AuthLocked);
    // Their own browser signs in as always.
    let own = world
        .password_login("ada", PASSWORD, "198.51.100.92", &known.mark_only())
        .await;
    assert_eq!(own.status, 200);

    // Its own count is as large as the limit says: the third wrong try here
    // fills it, and the next is refused.
    let reply = world
        .password_login("ada", "wrong password", "198.51.100.93", &known.mark_only())
        .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    let reply = world
        .password_login("ada", PASSWORD, "198.51.100.94", &known.mark_only())
        .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthLocked);

    // Two counts, one for the strangers and one for the browser.
    let buckets = world.buckets().await;
    let of_ada: Vec<_> = buckets
        .iter()
        .filter(|bucket| bucket.starts_with(&format!("password|u:{}", ada.id)))
        .collect();
    assert_eq!(of_ada.len(), 2, "{buckets:?}");
    assert!(of_ada.iter().any(|bucket| bucket.matches('|').count() == 2));
}

#[tokio::test]
async fn nothing_clears_a_count_but_time() {
    let world = World::locking_after(3).await;
    world.person("ada").await;
    wrong_tries(&world, "ada", 1).await;
    world.advance(Duration::hours(1));
    wrong_tries(&world, "ada", 1).await;
    // Getting in does not clear what was counted: the right password is let in...
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.90", "")
            .await
            .status,
        200
    );
    world.advance(Duration::hours(1));
    // ...and the next wrong try is the third of the three hours.
    wrong_tries(&world, "ada", 1).await;
    let locked = world
        .password_login("ada", PASSWORD, "198.51.100.91", "")
        .await;
    assert_eq!(locked.status, 423);
    // It stays locked until the oldest is three hours old: that was an hour ago for the
    // second, two hours ago for the first.
    assert_eq!(retry_after(&locked), "3600");
    world.advance(Duration::minutes(59));
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.92", "")
            .await
            .status,
        423
    );
    world.advance(Duration::minutes(1) + Duration::seconds(1));
    assert_eq!(
        world
            .password_login("ada", PASSWORD, "198.51.100.93", "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn a_try_that_is_refused_unseen_is_not_counted_against_the_address() {
    let mut config = orch_config::ServerConfig::default();
    config.auth.login_failure_limit = 2;
    config.auth.address_failure_limit = 3;
    let world = World::with(config).await;
    world.person("ada").await;
    // Two wrong tries from one address lock the name.
    for _ in 0..2 {
        let reply = world
            .password_login("ada", "wrong password", STRANGER, "")
            .await;
        assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
    }
    // Any number of tries at the locked name are refused unseen, and cost the address nothing.
    for _ in 0..5 {
        let reply = world.password_login("ada", PASSWORD, STRANGER, "").await;
        assert_eq!(reply.problem().code, ErrorCode::AuthLocked);
    }
    // The address has failed twice of its three: one more, at another name, and then it is full.
    let third = world
        .password_login("ghost", "wrong password", STRANGER, "")
        .await;
    assert_eq!(third.problem().code, ErrorCode::AuthInvalidCredentials);
    let refused = world
        .password_login("other", "wrong password", STRANGER, "")
        .await;
    assert_eq!(refused.status, 429);
    assert_eq!(retry_after(&refused), "900");
    assert_eq!(refused.problem().code, ErrorCode::RateLimited);
    // Whatever the names, and right or wrong: that machine has had its tries.
    let right = world.password_login("ada", PASSWORD, STRANGER, "").await;
    assert_eq!(right.status, 429);
    // Another address is another count, and a quarter of an hour is long enough.
    assert_eq!(
        world
            .password_login("other", "wrong password", "203.0.113.200", "")
            .await
            .status,
        401
    );
    world.advance(Duration::minutes(15) + Duration::seconds(1));
    assert_eq!(
        world
            .password_login("other", "wrong password", STRANGER, "")
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn a_right_password_gives_its_count_back_so_that_an_office_behind_one_address_does_not_lock_itself()
 {
    let mut config = orch_config::ServerConfig::default();
    config.auth.address_failure_limit = 2;
    let world = World::with(config).await;
    world.person("ada").await;
    for _ in 0..4 {
        assert_eq!(
            world
                .password_login("ada", PASSWORD, STRANGER, "")
                .await
                .status,
            200
        );
    }
    // Only wrong tries are held against the address.
    for _ in 0..2 {
        assert_eq!(
            world
                .password_login("ghost", "wrong password", STRANGER, "")
                .await
                .status,
            401
        );
    }
    assert_eq!(
        world
            .password_login("ghost", "wrong password", STRANGER, "")
            .await
            .status,
        429
    );
}

#[tokio::test]
async fn the_person_is_told_once_a_day_that_strangers_locked_their_name() {
    let world = World::locking_after(3).await;
    let ada = world.person("ada").await;
    let channel = orch_types::new_id();
    sqlx::query(
        "INSERT INTO notification_channels (id, user_id, kind, name, config, created_by)
        VALUES ($1, $2, 'webhook', 'Own hook', '{\"url\":\"https://hooks.example/security\"}', $2)",
    )
    .bind(channel)
    .bind(ada.id)
    .execute(&world.db.pool)
    .await
    .unwrap();
    // A route keeps a safe copy of what its channel is, as one made through its request does.
    sqlx::query(
        "INSERT INTO notification_routes (id, user_id, channel_id, channel_snapshot, enabled)
        SELECT $1, $2, c.id, jsonb_build_object('id', c.id, 'kind', c.kind, 'name', c.name, 'org_id', c.org_id), true
          FROM notification_channels c WHERE c.id = $3",
    )
    .bind(orch_types::new_id())
    .bind(ada.id)
    .bind(channel)
    .execute(&world.db.pool)
    .await
    .unwrap();
    wrong_tries(&world, "ada", 3).await;
    let inbox = inbox_of(&world, ada.id, 1).await;
    assert_eq!(inbox[0].0, "sign_in.attention");
    assert_eq!(inbox[0].2["way"], "password");
    let outward: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox o JOIN notification_routes r ON r.id = o.route_id
        WHERE r.channel_id = $1 AND o.payload -> 'notifications' -> 0 ->> 'kind' = 'sign_in.attention'")
        .bind(channel).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(
        outward, 1,
        "the ordinary enabled route carries sign-in attention too"
    );
    assert!(
        inbox[0].2["until"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-08T17:00:00")
    );

    // Locked again the same day: audited again, and the person is not told again.
    world.advance(Duration::hours(4));
    wrong_tries(&world, "ada", 3).await;
    inbox_of(&world, ada.id, 1).await;
    // The next day they are.
    world.advance(Duration::hours(20));
    wrong_tries(&world, "ada", 3).await;
    inbox_of(&world, ada.id, 2).await;
    let locked = world
        .audit()
        .await
        .iter()
        .filter(|each| *each == "auth.locked")
        .count();
    assert_eq!(locked, 3);
}

#[tokio::test]
async fn a_person_with_an_authenticator_is_asked_for_it_only_after_a_right_first_step() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let secret = world.authenticator(&signed).await;
    world.advance(step());

    // Right password, no second step given: it is asked for, by name.
    let asked = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(
        (asked.status, asked.code),
        (401, ErrorCode::AuthSecondStepRequired)
    );
    assert_eq!(asked.extra["methods"], json!(["totp", "recovery_code"]));
    // A wrong first step is told nothing more than any other name is.
    let wrong = world
        .password_login("ada", "wrong password", STRANGER, "")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    assert!(wrong.extra.get("methods").is_none());
    let ghost = world
        .attempt(
            with("ghost", Some(PASSWORD), None, Some("123456")),
            STRANGER,
            "",
        )
        .await
        .problem();
    assert_eq!(told(&ghost), told(&wrong));

    // A wrong second step is wrong, and is a try at the authenticator's count only.
    let bad = world
        .attempt(
            with("ada", Some(PASSWORD), None, Some("000000")),
            STRANGER,
            "",
        )
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::AuthSecondStepInvalid);
    // The right one gets in.
    let code = world.totp_code(&secret);
    let reply = world
        .attempt(with("ada", Some(PASSWORD), None, Some(&code)), STRANGER, "")
        .await;
    assert_eq!(reply.status, 200);
    assert!(reply.ok().user.totp_enabled);
}

#[tokio::test]
async fn an_authenticators_code_is_looked_at_only_when_the_first_step_was_right() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world.authenticator(&signed).await;
    world.advance(step());

    let wrong_first = world
        .attempt(
            with("ada", Some("wrong password"), None, Some("000000")),
            STRANGER,
            "",
        )
        .await
        .problem();
    assert_eq!(wrong_first.code, ErrorCode::AuthInvalidCredentials);
    let buckets = world.buckets().await;
    assert!(
        buckets.contains(&format!("password|u:{}", ada.id)),
        "{buckets:?}"
    );
    assert!(
        !buckets.contains(&format!("totp|u:{}", ada.id)),
        "{buckets:?}"
    );
}

#[tokio::test]
async fn a_code_of_the_authenticator_is_never_accepted_twice() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    let secret = world.authenticator(&signed).await;
    let login =
        |code: String| world.attempt(with("ada", Some(PASSWORD), None, Some(&code)), STRANGER, "");

    // The code that switched it on is spent: it is the code of this moment.
    let spent = login(world.totp_code(&secret)).await.problem();
    assert_eq!(spent.code, ErrorCode::AuthSecondStepInvalid);
    // The next moment's code is good once.
    world.advance(step());
    let fresh = world.totp_code(&secret);
    assert_eq!(login(fresh.clone()).await.status, 200);
    assert_eq!(
        login(fresh).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    // Nor is a code of an earlier step, though the clock would still take it.
    let earlier = world.totp_code_at(&secret, world.now() - step());
    assert_eq!(
        login(earlier).await.problem().code,
        ErrorCode::AuthSecondStepInvalid
    );
    // Next moment, next code.
    world.advance(step());
    assert_eq!(login(world.totp_code(&secret)).await.status, 200);
}

#[tokio::test]
async fn a_suspended_person_with_the_right_password_is_refused_and_with_a_wrong_one_told_nothing() {
    let world = World::new().await;
    let ada = world.person("ada").await;
    world.suspend(&ada).await;
    let wrong = world
        .password_login("ada", "wrong password", STRANGER, "")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    let right = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!((right.status, right.code), (401, ErrorCode::AuthSuspended));
}

#[tokio::test]
async fn with_signing_in_here_switched_off_only_system_admins_get_in() {
    let world = World::new().await;
    world.person("ada").await;
    let root = world.person("root").await;
    world.make_admin(&root).await;
    world.set("auth.local_login", json!(false)).await;

    let refused = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(
        (refused.status, refused.code),
        (403, ErrorCode::AuthLocalLoginDisabled)
    );
    // Not before the first step was right: a wrong password is wrong for everybody.
    let wrong = world
        .password_login("ada", "wrong password", STRANGER, "")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    assert_eq!(
        world
            .password_login("root", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn an_address_that_is_not_confirmed_signs_nobody_in_where_confirmation_is_required() {
    let world = World::new().await;
    world.mail_server().await;
    world
        .set("registration.email_verification", json!("required"))
        .await;
    world.person_with("ada", PASSWORD, false).await;
    let wrong = world
        .password_login("ada", "wrong password", STRANGER, "")
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::AuthInvalidCredentials);
    let unverified = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(
        (unverified.status, unverified.code),
        (403, ErrorCode::AuthEmailUnverified)
    );
    // A system without a mail server cannot ask for it, whatever the setting says.
    let bare = World::new().await;
    bare.set("registration.email_verification", json!("required"))
        .await;
    bare.person_with("bob", PASSWORD, false).await;
    assert_eq!(
        bare.password_login("bob", PASSWORD, STRANGER, "")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn a_mailed_code_gets_a_person_in_once_where_their_orgs_allow_it() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    world.person("ada").await;
    world.person("bob").await;

    assert_eq!(
        ask_for_code(&world, "ada", None, STRANGER, "").await.status,
        202
    );
    let code = world.mailed_code("ada@example.org").await;
    assert_eq!(code.len(), 9, "{code}");
    // Read without regard to case, hyphen or space.
    let typed = code.replace('-', " ").to_lowercase();
    let reply = world
        .attempt(with("ada", None, Some(&typed), None), STRANGER, "")
        .await;
    assert_eq!(reply.status, 200);
    assert!(cookie(&reply.headers, "__Host-orch_session").is_some());
    // One use.
    let again = world
        .attempt(with("ada", None, Some(&code), None), STRANGER, "")
        .await;
    assert_eq!(again.problem().code, ErrorCode::AuthInvalidCredentials);

    // A code is for the person it was mailed to.
    ask_for_code(&world, "bob", None, STRANGER, "").await;
    let bobs = world.mailed_code("bob@example.org").await;
    let wrong = world
        .attempt(with("ada", None, Some(&bobs), None), STRANGER, "")
        .await;
    assert_eq!(wrong.problem().code, ErrorCode::AuthInvalidCredentials);

    // Good for ten minutes.
    world.advance(Duration::minutes(2));
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    let late = world.mailed_code("ada@example.org").await;
    world.advance(Duration::minutes(10) + Duration::seconds(1));
    let reply = world
        .attempt(with("ada", None, Some(&late), None), STRANGER, "")
        .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
}

#[tokio::test]
async fn a_code_for_an_old_address_cannot_sign_in_after_the_address_changes() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    let ada = world.person("ada").await;
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    let old = world.mailed_code("ada@example.org").await;
    // Simulates an old-address issuance that raced a committed address replacement.
    sqlx::query("UPDATE users SET email='new@example.org', email_verified_at=$2 WHERE id=$1")
        .bind(ada.id)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let denied = world
        .attempt(with("ada", None, Some(&old), None), STRANGER, "")
        .await;
    assert_eq!(denied.problem().code, ErrorCode::AuthInvalidCredentials);
}

#[tokio::test]
async fn a_mailed_code_is_no_way_in_where_nothing_allows_it() {
    let world = World::new().await;
    world.mail_server().await;
    world.person("ada").await;
    // The system has not allowed it (as shipped): the request is answered, and nothing is mailed.
    assert_eq!(
        ask_for_code(&world, "ada", None, STRANGER, "").await.status,
        202
    );
    assert!(
        world
            .mails("sign_in_code", "ada@example.org")
            .await
            .is_empty()
    );

    // A code that exists but may not get this person in is none.
    world.set("auth.email_code", json!(["sign_in"])).await;
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    let code = world.mailed_code("ada@example.org").await;
    world.set("auth.email_code", json!([])).await;
    let reply = world
        .attempt(with("ada", None, Some(&code), None), STRANGER, "")
        .await;
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
}

#[tokio::test]
async fn asking_for_a_code_is_answered_the_same_to_every_name_after_the_same_work() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    world.person("ada").await; // may be sent a code
    world.person_with("bob", PASSWORD, false).await; // an address that is not confirmed
    let carl = world.person("carl").await; // suspended
    world.suspend(&carl).await;
    ask_for_code(&world, "warm-up", None, "198.51.100.1", "").await;

    let mut answers = Vec::new();
    for (index, login) in ["ghost", "ada", "bob", "carl", "ada@example.org"]
        .into_iter()
        .enumerate()
    {
        let before = Statements::now();
        let reply = ask_for_code(&world, login, None, &address(index), "").await;
        let used = Statements::now() - before;
        assert_eq!(
            (reply.status.as_u16(), reply.raw.is_empty()),
            (202, true),
            "{login}"
        );
        let mut headers: Vec<_> = reply
            .headers
            .keys()
            .map(|name| name.as_str().to_owned())
            .filter(|name| name != "x-request-id")
            .collect();
        headers.sort();
        answers.push((used, headers));
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );

    // Only the person who may be sent one was.
    let outbox = world.outbox().await;
    assert_eq!(outbox.len(), 1, "{outbox:?}");
    assert_eq!(outbox[0]["to"], "ada@example.org");

    // A password given counts as a password given to the sign-in, wrong or right.
    let wrong = ask_for_code(&world, "ada", Some("wrong password"), "198.51.100.40", "").await;
    assert_eq!(wrong.status, 202);
    let ghost = ask_for_code(&world, "ghost", Some("wrong password"), "198.51.100.41", "").await;
    assert_eq!(ghost.status, 202);
    assert!(
        world
            .buckets()
            .await
            .iter()
            .any(|bucket| bucket.starts_with("password|u:"))
    );
    assert!(
        world
            .buckets()
            .await
            .iter()
            .any(|bucket| bucket.starts_with("password|n:"))
    );
}

#[tokio::test]
async fn a_wrong_password_given_to_ask_for_a_code_counts_as_one_given_to_sign_in() {
    let world = World::locking_after(3).await;
    world.mail_server().await;
    world.person("ada").await;
    for index in 0..3 {
        let reply = ask_for_code(&world, "ada", Some("wrong password"), &address(index), "").await;
        assert_eq!(reply.status, 202);
    }
    // The three were the three of the password at that name.
    let locked = world
        .password_login("ada", PASSWORD, "198.51.100.90", "")
        .await;
    assert_eq!(locked.problem().extra["way"], "password");
    // Asking with a password that is refused unseen is still answered 202, and nothing is mailed.
    let refused = ask_for_code(&world, "ada", Some(PASSWORD), "198.51.100.91", "").await;
    assert_eq!(refused.status, 202);
    assert!(world.outbox().await.is_empty());
}

#[tokio::test]
async fn codes_are_mailed_one_a_minute_and_five_in_three_hours_to_a_name_and_to_a_known_browser() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    let ada = world.person("ada").await;
    let known: Signed = world.sign_in(&ada).await;
    let mails = || world.mails("sign_in_code", "ada@example.org");

    ask_for_code(&world, "ada", None, STRANGER, "").await;
    assert_eq!(mails().await.len(), 1);
    let first = world.mailed_code("ada@example.org").await;
    // Not within the minute.
    world.advance(Duration::seconds(30));
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    assert_eq!(mails().await.len(), 1);
    // A minute apart, until five have gone.
    for expected in 2..=5 {
        world.advance(Duration::seconds(61));
        ask_for_code(&world, "ada", None, STRANGER, "").await;
        assert_eq!(mails().await.len(), expected);
    }
    world.advance(Duration::seconds(61));
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    assert_eq!(mails().await.len(), 5);
    // The answer was 202 all the same, and a new code ended the one before.
    let old = world
        .attempt(with("ada", None, Some(&first), None), STRANGER, "")
        .await;
    assert_eq!(old.problem().code, ErrorCode::AuthInvalidCredentials);
    // Their own browser has a pace of its own.
    ask_for_code(&world, "ada", None, STRANGER, &known.mark_only()).await;
    assert_eq!(mails().await.len(), 6);
    // Three hours on, the five have aged, and one may go again.
    world.advance(Duration::hours(3));
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    assert_eq!(mails().await.len(), 7);
}

#[tokio::test]
async fn a_person_whose_only_second_step_is_the_mailed_code_and_who_began_with_one_is_asked_for_their_password()
 {
    let world = World::new().await;
    world.mail_server().await;
    world
        .set("auth.email_code", json!(["sign_in", "second_step"]))
        .await;
    let ada = world.person("ada").await;
    {
        let mut conn = world.conn().await;
        identity::set_email_code_second_step(&mut conn, ada.id, Some(start()), start())
            .await
            .unwrap();
    }
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    let code = world.mailed_code("ada@example.org").await;

    // A mailed code is one proof: with it alone, the password is what is missing.
    let first = world
        .attempt(with("ada", None, Some(&code), None), STRANGER, "")
        .await
        .problem();
    assert_eq!(first.code, ErrorCode::AuthSecondStepRequired);
    assert_eq!(first.extra["methods"], json!(["password"]));
    // With the password alone, the mailed code is.
    let second = world
        .password_login("ada", PASSWORD, STRANGER, "")
        .await
        .problem();
    assert_eq!(second.code, ErrorCode::AuthSecondStepRequired);
    assert_eq!(second.extra["methods"], json!(["email_code"]));
    // The code was not spent by the requests that did not complete a sign-in.
    let both = world
        .attempt(with("ada", Some(PASSWORD), Some(&code), None), STRANGER, "")
        .await;
    assert_eq!(both.status, 200);
    // And it is spent now.
    let again = world
        .attempt(with("ada", Some(PASSWORD), Some(&code), None), STRANGER, "")
        .await;
    assert_eq!(again.problem().code, ErrorCode::AuthSecondStepInvalid);
}

#[tokio::test]
async fn a_mailed_code_as_a_second_step_is_asked_for_by_asking_with_the_password() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["second_step"])).await;
    let ada = world.person("ada").await;
    {
        let mut conn = world.conn().await;
        identity::set_email_code_second_step(&mut conn, ada.id, Some(start()), start())
            .await
            .unwrap();
    }
    // A code is not a way in by itself here, so without the password none is mailed.
    ask_for_code(&world, "ada", None, STRANGER, "").await;
    assert!(
        world
            .mails("sign_in_code", "ada@example.org")
            .await
            .is_empty()
    );
    // With a wrong password none is mailed either, and with the right one it is.
    ask_for_code(&world, "ada", Some("wrong password"), "198.51.100.40", "").await;
    assert!(
        world
            .mails("sign_in_code", "ada@example.org")
            .await
            .is_empty()
    );
    ask_for_code(&world, "ada", Some(PASSWORD), "198.51.100.41", "").await;
    let code = world.mailed_code("ada@example.org").await;
    let reply = world
        .attempt(with("ada", Some(PASSWORD), Some(&code), None), STRANGER, "")
        .await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn a_mailed_code_that_an_org_does_not_allow_is_not_a_way_in_for_its_members() {
    let world = World::new().await;
    world.mail_server().await;
    world.set("auth.email_code", json!(["sign_in"])).await;
    world.person("ada").await;
    let bob = world.person("bob").await;
    // Bob belongs to an org that allows no mailed code; Ada's own org does not count.
    let acme = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, acme, "org", None, "Acme", bob.id,
        )
        .await
        .unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, acme, bob.id, acme)
            .await
            .unwrap();
    }
    world
        .set_at(
            orch_settings::Target::Node(acme),
            "auth.email_code",
            json!([]),
        )
        .await;
    ask_for_code(&world, "ada", None, "198.51.100.40", "").await;
    ask_for_code(&world, "bob", None, "198.51.100.41", "").await;
    assert_eq!(
        world.mails("sign_in_code", "ada@example.org").await.len(),
        1
    );
    assert!(
        world
            .mails("sign_in_code", "bob@example.org")
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_service_identity_signs_in_nowhere() {
    let world = World::new().await;
    let owner = world.person("owner").await;
    let service = orch_types::new_id();
    sqlx::query(
        "INSERT INTO users (id, kind, service_org_id, username, display_name, avatar_color, password_hash)
         VALUES ($1, 'service', $2, 'robot', 'Robot', 1, 'x')",
    )
    .bind(service)
    .bind(owner.personal_org)
    .execute(&world.db.pool)
    .await
    .unwrap();
    let reply = world.password_login("robot", PASSWORD, STRANGER, "").await;
    assert_eq!(reply.problem().code, ErrorCode::AuthInvalidCredentials);
}
