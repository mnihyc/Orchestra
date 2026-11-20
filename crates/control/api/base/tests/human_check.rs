//! The human check: where the system asks it, what a token that passes, fails, is old or is used twice comes to, a service that cannot be asked, and what each of these costs a name and an address. The vendors are the real backends speaking through a scripted transport, so what they send and how they read the answers is tried as well.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_contract::admin::{
    AdminHumanCheckGet, AdminHumanCheckSecretDelete, AdminHumanCheckSecretSet, AdminHumanCheckTest,
    AdminSettingSet, AdminSettingSetBody, HumanCheckEdgeState, HumanCheckEdgeVia,
    HumanCheckSecretBody, HumanCheckState, HumanCheckTestReply,
};
use orch_api_contract::auth::{
    AdminModeSetBody, AuthEmailCodeSend, AuthEmailCodeSendBody, AuthEmailVerify,
    AuthEmailVerifyBody, AuthEmailVerifyResend, AuthEmailVerifyResendBody, AuthLogin,
    AuthLoginBody, AuthNameAvailabilityGet, AuthRecoveryCheck, AuthRecoveryConfirm,
    AuthRecoveryConfirmBody, AuthRecoveryStart, AuthRecoveryStartBody, AuthRegister,
    MeAdminModeSet, MetaGet, SetupCreate,
};
use orch_api_contract::common::ErrorCode;
use orch_api_core::testing::Reply;
use orch_human_check::{EdgeReader, Reply as Said, Scripted, ScriptedEdge, Transport};
use orch_types::new_id;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Once};
use support::{PASSWORD, Signed, World};

/// The one vendor of this program, which answers what a test scripts, and the tests that script it one at a time.
static VENDOR: LazyLock<Arc<Scripted>> = LazyLock::new(|| Arc::new(Scripted::default()));
static ONE_AT_A_TIME: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
/// The one web edge of this program, which answers what a test scripts: nothing, where it does not.
static EDGE: LazyLock<Arc<ScriptedEdge>> = LazyLock::new(|| Arc::new(ScriptedEdge::new()));
/// Where the server looks at the stack's own web edge, and at the address people open.
const STACK_EDGE: &str = "http://frontend/";
const PUBLIC_EDGE: &str = "https://orch.test/";
/// The policy the edge sent before a human check was a choice.
const POLICY_BEFORE: &str = "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self'; worker-src 'self'; manifest-src 'self'";

const SECRET: &str = "the-secret-of-the-test";
const SITE_KEY: &str = "10000000-ffff-ffff-ffff-000000000001";
const FROM: &str = "198.51.100.10";

/// Prints what the server logs as an internal failure, so that a 500 says why.
fn show_internal_failures() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .without_time()
            .try_init();
    });
}

fn install() {
    show_internal_failures();
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let transport: Arc<dyn Transport> = Arc::clone(&VENDOR) as Arc<dyn Transport>;
        orch_api_core::sign_in::human_check::install_providers(orch_human_check::builtin(
            &transport,
        ));
        orch_api_core::sign_in::human_check::install_edge_reader(
            Arc::clone(&EDGE) as Arc<dyn EdgeReader>
        );
    });
}

/// Forgets what the vendor, the web edge and the server were told and found, for a test that starts again.
async fn forget() {
    VENDOR.clear();
    EDGE.clear();
    orch_api_core::sign_in::human_check::forget_looks().await;
}

/// The vendor says `body`.
fn says(body: &str) {
    VENDOR.then(Ok(Said {
        status: 200,
        body: body.as_bytes().to_vec(),
    }));
}

const PASSES: &str = r#"{"success":true,"hostname":"orch.test"}"#;
const REFUSES: &str = r#"{"success":false,"error-codes":["invalid-input-response"]}"#;
const OLD: &str = r#"{"success":false,"error-codes":["expired-input-response"]}"#;
const USED: &str = r#"{"success":false,"error-codes":["already-seen-response"]}"#;
const WRONG_SECRET: &str = r#"{"success":false,"error-codes":["invalid-input-secret"]}"#;

/// A world whose system asks the human check of hCaptcha, with its secret kept.
async fn asking() -> (World, tokio::sync::MutexGuard<'static, ()>) {
    install();
    let guard = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    world.set("human_check.provider", json!("hcaptcha")).await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    keep_secret(&world, SECRET).await;
    (world, guard)
}

async fn keep_secret(world: &World, secret: &str) {
    use orch_api_core::routes::resources::sealing::binding;
    use orch_api_core::sign_in::human_check::{SECRET_NAME, SECRET_OWNER, secret_owner_id};
    // The key the world signs in with.
    let sealed = orch_db::secrets::MasterKey::new([7; 32])
        .seal(
            secret.as_bytes(),
            &binding(SECRET_OWNER, secret_owner_id(), SECRET_NAME),
        )
        .unwrap();
    let mut conn = world.conn().await;
    orch_db::queries::settings::put_secret(
        &mut conn,
        SECRET_OWNER,
        secret_owner_id(),
        SECRET_NAME,
        &sealed,
        None,
        world.now(),
    )
    .await
    .unwrap();
}

fn peer(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse().unwrap(), 0)
}

fn sign_in_body(login: &str, password: &str) -> AuthLoginBody {
    AuthLoginBody {
        login: login.to_owned(),
        password: Some(password.to_owned()),
        email_code: None,
        totp: None,
        recovery_code: None,
    }
}

/// A sign-in from `ip`, with the token of a human check where one is given.
async fn sign_in(
    world: &World,
    body: AuthLoginBody,
    ip: &str,
    token: Option<&str>,
) -> Reply<orch_api_contract::auth::AuthResult> {
    let mut call = world
        .app
        .call(AuthLogin, &[])
        .peer(peer(ip))
        .header("user-agent", "tests/1.0");
    if let Some(token) = token {
        call = call.header("x-human-check", token);
    }
    call.body(&body).send().await
}

fn reason(problem: &orch_api_contract::common::Problem) -> Option<&str> {
    problem.extra.get("reason").and_then(Value::as_str)
}

async fn human_buckets(world: &World) -> Vec<String> {
    world
        .buckets()
        .await
        .into_iter()
        .filter(|bucket| bucket.starts_with("human|"))
        .collect()
}

async fn times(world: &World, bucket: &str) -> usize {
    let mut conn = world.conn().await;
    orch_db::queries::identity::throttle_times(&mut conn, bucket)
        .await
        .unwrap()
        .len()
}

// ---------------------------------------------------------------- off

#[tokio::test]
async fn with_the_check_off_every_route_answers_as_before() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    world.set("registration.mode", json!("open")).await;
    let ada = world.person("ada").await;
    // A person signs in, asks for a code and a link, and registers, with no token and no service asked.
    sign_in(&world, sign_in_body(&ada.username, PASSWORD), FROM, None)
        .await
        .ok();
    let sent = world
        .app
        .call(AuthEmailCodeSend, &[])
        .peer(peer(FROM))
        .body(&AuthEmailCodeSendBody {
            login: "ada".into(),
            password: None,
        })
        .send()
        .await;
    assert_eq!(sent.status, 202);
    let recovery = world
        .app
        .call(AuthRecoveryStart, &[])
        .peer(peer(FROM))
        .body(&AuthRecoveryStartBody {
            email: "ada@example.org".into(),
        })
        .send()
        .await;
    assert_eq!(recovery.status, 202);
    let registered = world
        .app
        .call(AuthRegister, &[])
        .peer(peer(FROM))
        .raw_body(&json!({"username":"newcomer","display_name":"New","password":PASSWORD}))
        .send_bytes()
        .await;
    assert_eq!(registered.status, 201);
    assert!(VENDOR.calls().is_empty(), "no service was asked");
    assert!(human_buckets(&world).await.is_empty());
}

// ---------------------------------------------------------------- the decision, through the routes

#[tokio::test]
async fn a_guarded_route_without_a_token_is_refused_before_the_name_is_looked_at() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    for login in [ada.username.as_str(), "nobody-has-this-name"] {
        let reply = sign_in(&world, sign_in_body(login, PASSWORD), FROM, None).await;
        let problem = reply.problem();
        assert_eq!(problem.code, ErrorCode::HumanCheckRequired, "{login}");
        assert_eq!(problem.status, 403);
    }
    assert!(
        VENDOR.calls().is_empty(),
        "nothing is asked of the service without a token"
    );
    // Neither name was looked at: no count of wrong tries was started for either.
    assert!(
        world.buckets().await.is_empty(),
        "{:?}",
        world.buckets().await
    );
}

#[tokio::test]
async fn a_passed_check_signs_in_and_gives_its_count_back() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    says(PASSES);
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("a-token"),
    )
    .await
    .ok();
    let calls = VENDOR.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].url, "https://api.hcaptcha.com/siteverify");
    let orch_human_check::Body::Form(form) = &calls[0].body else {
        panic!("hCaptcha is asked with a form");
    };
    let field = |name: &str| {
        form.iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(field("secret"), Some(SECRET));
    assert_eq!(field("response"), Some("a-token"));
    assert_eq!(field("sitekey"), Some(SITE_KEY));
    assert_eq!(field("remoteip"), Some(FROM));
    assert_eq!(
        times(&world, &format!("human|{FROM}")).await,
        0,
        "a pass gives its count back"
    );
}

#[tokio::test]
async fn a_failed_check_costs_the_name_no_wrong_try_and_the_address_one_count() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    says(REFUSES);
    let reply = sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("bad"),
    )
    .await;
    let problem = reply.problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckFailed);
    assert_eq!(reason(&problem), Some("invalid"));
    let buckets = world.buckets().await;
    assert_eq!(
        buckets,
        vec![format!("human|{FROM}")],
        "only the address was counted"
    );
    assert_eq!(times(&world, &format!("human|{FROM}")).await, 1);
}

#[tokio::test]
async fn a_failed_check_gives_a_name_nobody_has_the_same_answer_as_one_somebody_has() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    let mut answers = Vec::new();
    for login in [ada.username.as_str(), "nobody-has-this-name"] {
        says(REFUSES);
        let problem = sign_in(&world, sign_in_body(login, PASSWORD), FROM, Some("bad"))
            .await
            .problem();
        let why = reason(&problem).map(str::to_owned);
        answers.push((problem.code, problem.status, problem.detail, why));
    }
    assert_eq!(answers[0], answers[1]);
}

#[tokio::test]
async fn an_old_token_is_expired_and_a_used_one_is_refused_the_second_time() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    for (answer, why) in [(OLD, None), (USED, Some("replayed"))] {
        says(answer);
        let problem = sign_in(
            &world,
            sign_in_body(&ada.username, PASSWORD),
            FROM,
            Some("t"),
        )
        .await
        .problem();
        assert_eq!(problem.code, ErrorCode::HumanCheckExpired, "{answer}");
        if let Some(why) = why {
            assert_eq!(reason(&problem), Some(why));
        }
    }
}

#[tokio::test]
async fn every_call_of_a_sign_in_with_a_second_step_needs_its_own_token() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed: Signed = world.sign_in(&ada).await;
    let authenticator = world.authenticator(&signed).await;
    world.set("human_check.provider", json!("hcaptcha")).await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    keep_secret(&world, SECRET).await;
    // The first call, with the password alone, is answered that a second step is wanted.
    says(PASSES);
    let first = sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("one"),
    )
    .await;
    assert_eq!(first.problem().code, ErrorCode::AuthSecondStepRequired);
    // The second call carries the password again and the code, and is refused without a token of its own.
    world.advance(support::step());
    let mut second = sign_in_body(&ada.username, PASSWORD);
    second.totp = Some(world.totp_code(&authenticator));
    let refused = sign_in(&world, second.clone(), FROM, None).await;
    assert_eq!(refused.problem().code, ErrorCode::HumanCheckRequired);
    says(PASSES);
    sign_in(&world, second, FROM, Some("two")).await.ok();
    assert_eq!(VENDOR.calls().len(), 2);
}

// ---------------------------------------------------------------- the other acts

#[tokio::test]
async fn registration_recovery_a_code_and_the_confirmation_link_again_are_guarded_and_the_links_themselves_are_not()
 {
    let (world, _one) = asking().await;
    world.set("registration.mode", json!("open")).await;
    let ada = world.person("ada").await;
    let registered = world
        .app
        .call(AuthRegister, &[])
        .peer(peer(FROM))
        .raw_body(&json!({"username":"newcomer","display_name":"New","password":PASSWORD}))
        .send_bytes()
        .await;
    assert_eq!(registered.status, 403);
    assert_eq!(
        serde_json::from_slice::<Value>(&registered.raw).unwrap()["code"],
        "human_check.required"
    );
    let recovery = world
        .app
        .call(AuthRecoveryStart, &[])
        .peer(peer(FROM))
        .body(&AuthRecoveryStartBody {
            email: "ada@example.org".into(),
        })
        .send()
        .await;
    assert_eq!(recovery.problem().code, ErrorCode::HumanCheckRequired);
    let code = world
        .app
        .call(AuthEmailCodeSend, &[])
        .peer(peer(FROM))
        .body(&AuthEmailCodeSendBody {
            login: ada.username.clone(),
            password: None,
        })
        .send()
        .await;
    assert_eq!(code.problem().code, ErrorCode::HumanCheckRequired);
    let again = world
        .app
        .call(AuthEmailVerifyResend, &[])
        .peer(peer(FROM))
        .body(&AuthEmailVerifyResendBody {
            login: ada.username.clone(),
        })
        .send()
        .await;
    assert_eq!(again.problem().code, ErrorCode::HumanCheckRequired);
    // The links that carry their own proof are not asked for a check.
    let check = world
        .app
        .call(AuthRecoveryCheck, &[&"a-link-nobody-made"])
        .peer(peer(FROM))
        .send()
        .await;
    assert_ne!(check.problem().code, ErrorCode::HumanCheckRequired);
    let confirm = world
        .app
        .call(AuthRecoveryConfirm, &[])
        .peer(peer(FROM))
        .body(&AuthRecoveryConfirmBody {
            token: "a-link-nobody-made".into(),
            password: PASSWORD.into(),
        })
        .send()
        .await;
    assert_ne!(confirm.problem().code, ErrorCode::HumanCheckRequired);
    let verify = world
        .app
        .call(AuthEmailVerify, &[])
        .peer(peer(FROM))
        .body(&AuthEmailVerifyBody {
            token: "a-link-nobody-made".into(),
        })
        .send()
        .await;
    assert_ne!(verify.problem().code, ErrorCode::HumanCheckRequired);
    assert!(VENDOR.calls().is_empty());
}

#[tokio::test]
async fn a_signed_in_person_asks_for_their_own_confirmation_link_again_with_no_check_and_for_nothing_else()
 {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world.set("human_check.provider", json!("hcaptcha")).await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    keep_secret(&world, SECRET).await;
    let again = world
        .app
        .call(AuthEmailVerifyResend, &[])
        .peer(peer(FROM))
        .token(signed.token())
        .body(&AuthEmailVerifyResendBody {
            login: ada.username.clone(),
        })
        .send()
        .await;
    assert_eq!(again.status, 202);
    // Every other act asks, with a token of an account in hand or not.
    let code = world
        .app
        .call(AuthEmailCodeSend, &[])
        .peer(peer(FROM))
        .token(signed.token())
        .body(&AuthEmailCodeSendBody {
            login: ada.username.clone(),
            password: None,
        })
        .send()
        .await;
    assert_eq!(code.problem().code, ErrorCode::HumanCheckRequired);
    let login = world
        .app
        .call(AuthLogin, &[])
        .peer(peer(FROM))
        .token(signed.token())
        .body(&sign_in_body(&ada.username, PASSWORD))
        .send()
        .await;
    assert_eq!(login.problem().code, ErrorCode::HumanCheckRequired);
    // A token that is not good is not a session: the act is refused, as it is for anybody.
    let stranger = world
        .app
        .call(AuthEmailVerifyResend, &[])
        .peer(peer(FROM))
        .token("a-token-nobody-issued")
        .body(&AuthEmailVerifyResendBody {
            login: ada.username.clone(),
        })
        .send()
        .await;
    assert!(stranger.body.is_err(), "{}", stranger.status);
    assert!(VENDOR.calls().is_empty());
}

#[tokio::test]
async fn an_act_that_is_not_among_those_asked_is_not_asked() {
    let (world, _one) = asking().await;
    world.set("human_check.acts", json!(["sign_in"])).await;
    world.person("ada").await;
    let recovery = world
        .app
        .call(AuthRecoveryStart, &[])
        .peer(peer(FROM))
        .body(&AuthRecoveryStartBody {
            email: "ada@example.org".into(),
        })
        .send()
        .await;
    assert_eq!(recovery.status, 202);
    assert!(VENDOR.calls().is_empty());
}

#[tokio::test]
async fn the_first_administrator_the_names_and_a_link_to_look_at_never_ask_for_a_token() {
    let (world, _one) = asking().await;
    let free = world
        .app
        .call(AuthNameAvailabilityGet, &[])
        .peer(peer(FROM))
        .query(&json!({"name": "somebody"}))
        .send()
        .await;
    assert_eq!(free.status, 200);
    let first = world
        .app
        .call(SetupCreate, &[])
        .peer(peer(FROM))
        .raw_body(&json!({"username":"root","display_name":"Root","password":PASSWORD}))
        .send_bytes()
        .await;
    assert_eq!(first.status, 200, "{}", String::from_utf8_lossy(&first.raw));
    assert!(VENDOR.calls().is_empty());
}

#[tokio::test]
async fn a_retried_registration_whose_answer_was_lost_needs_no_second_token() {
    let (world, _one) = asking().await;
    world.set("registration.mode", json!("open")).await;
    let key = new_id().to_string();
    let input = json!({"username":"registered-once","display_name":"Once","password":PASSWORD});
    says(PASSES);
    let first = world
        .app
        .call(AuthRegister, &[])
        .peer(peer(FROM))
        .header("Idempotency-Key", &key)
        .header("x-human-check", "one-token")
        .raw_body(&input)
        .send_bytes()
        .await;
    assert_eq!(first.status, 201);
    // The same request again, as a client retries when the answer did not reach it: the creation is proved by its
    // password and its key, and the spent token is not asked for again.
    let again = world
        .app
        .call(AuthRegister, &[])
        .peer(peer(FROM))
        .header("Idempotency-Key", &key)
        .raw_body(&input)
        .send_bytes()
        .await;
    assert_eq!(again.status, 201, "{}", String::from_utf8_lossy(&again.raw));
    assert_eq!(VENDOR.calls().len(), 1);
}

// ---------------------------------------------------------------- the service cannot be asked

#[tokio::test]
async fn an_unreachable_service_admits_and_records_or_refuses_as_the_setting_says() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    // Its default: the person is let in, and the audit says so once in the hour.
    says(WRONG_SECRET);
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("t"),
    )
    .await
    .ok();
    says(WRONG_SECRET);
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        "198.51.100.11",
        Some("t"),
    )
    .await
    .ok();
    let admitted: Vec<String> = world
        .audit()
        .await
        .into_iter()
        .filter(|action| action == "auth.human_check_admitted")
        .collect();
    assert_eq!(admitted.len(), 1, "one row an hour for an act");
    // Chosen to refuse, it turns the person away and says why on the screen's word.
    world
        .set("human_check.when_unreachable", json!("refuse"))
        .await;
    says(WRONG_SECRET);
    let problem = sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        "198.51.100.12",
        Some("t"),
    )
    .await
    .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckUnavailable);
    assert_eq!(problem.status, 503);
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.human_check_refused".to_owned())
    );
    // The row says what the administrator needs to read: the act, the word for what went wrong, and no secret.
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action LIKE 'auth.human_check_%' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert!(rows.len() >= 2);
    assert_eq!(rows[0]["act"], "sign_in");
    assert_eq!(rows[0]["why"], "secret");
    assert!(!rows.iter().any(|row| row.to_string().contains(SECRET)));
}

#[tokio::test]
async fn a_service_without_a_secret_is_unavailable_and_not_a_failure_of_the_person() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    world.set("human_check.provider", json!("hcaptcha")).await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    let ada = world.person("ada").await;
    // No secret is kept: the service is not asked, and the default lets the person in.
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("t"),
    )
    .await
    .ok();
    assert!(VENDOR.calls().is_empty());
}

#[tokio::test]
async fn the_hourly_audit_line_counts_admitted_checks() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    let ask = |ip: &'static str| {
        let world = &world;
        let name = ada.username.clone();
        async move {
            says(WRONG_SECRET);
            let body = AuthEmailCodeSendBody {
                login: name,
                password: None,
            };
            world
                .app
                .call(AuthEmailCodeSend, &[])
                .peer(peer(ip))
                .header("x-human-check", "t")
                .body(&body)
                .send()
                .await
        }
    };
    ask("198.51.100.20").await;
    ask("198.51.100.21").await;
    ask("198.51.100.22").await;
    world.advance(chrono::Duration::minutes(61));
    ask("198.51.100.23").await;
    let counts: Vec<i64> = sqlx::query_scalar(
        "SELECT (details->>'count')::bigint FROM audit_log WHERE action='auth.human_check_admitted' AND details->>'act'='email_code' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        counts,
        vec![1, 3],
        "the first row, and the three that followed it in the hour after"
    );
}

// ---------------------------------------------------------------- reCAPTCHA

async fn asking_google(verification: &str) -> (World, tokio::sync::MutexGuard<'static, ()>) {
    install();
    let guard = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    world.set("human_check.provider", json!("recaptcha")).await;
    world
        .set("human_check.verification", json!(verification))
        .await;
    world
        .set("human_check.site_key", json!("6Lc-the-site-key"))
        .await;
    world
        .set("human_check.project_id", json!("a-project"))
        .await;
    keep_secret(&world, "the-secret-key").await;
    (world, guard)
}

fn siteverify(action: &str, hostname: &str, score: f64) -> String {
    json!({"success":true,"challenge_ts":"2026-11-13T10:00:00Z","hostname":hostname,"score":score,"action":action}).to_string()
}

#[tokio::test]
async fn a_token_made_for_another_act_or_on_another_site_or_scored_low_is_refused() {
    let (world, _one) = asking_google("siteverify").await;
    let ada = world.person("ada").await;
    for (answer, why) in [
        (siteverify("orch_register", "orch.test", 0.9), "wrong_act"),
        (
            siteverify("orch_sign_in", "evil.example", 0.9),
            "wrong_site",
        ),
        (siteverify("orch_sign_in", "orch.test", 0.3), "low_score"),
    ] {
        says(&answer);
        let problem = sign_in(
            &world,
            sign_in_body(&ada.username, PASSWORD),
            FROM,
            Some("t"),
        )
        .await
        .problem();
        assert_eq!(problem.code, ErrorCode::HumanCheckFailed, "{why}");
        assert_eq!(reason(&problem), Some(why));
    }
    // A score at the setting passes; a setting of its own moves the line.
    says(&siteverify("orch_sign_in", "orch.test", 0.5));
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("t"),
    )
    .await
    .ok();
    world.set("human_check.min_score", json!(20)).await;
    says(&siteverify("orch_sign_in", "orch.test", 0.3));
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        "198.51.100.30",
        Some("t"),
    )
    .await
    .ok();
    let calls = VENDOR.calls();
    assert_eq!(
        calls[0].url,
        "https://www.google.com/recaptcha/api/siteverify"
    );
}

#[tokio::test]
async fn the_assessments_way_asks_the_cloud_project_with_the_key_in_the_query() {
    let (world, _one) = asking_google("assessments").await;
    let ada = world.person("ada").await;
    says(
        r#"{"tokenProperties":{"valid":true,"hostname":"orch.test","action":"orch_sign_in"},"riskAnalysis":{"score":0.9,"reasons":[]}}"#,
    );
    sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("t"),
    )
    .await
    .ok();
    let calls = VENDOR.calls();
    assert_eq!(
        calls[0].url,
        "https://recaptchaenterprise.googleapis.com/v1/projects/a-project/assessments?key=the-secret-key"
    );
}

// ---------------------------------------------------------------- bounds

#[tokio::test]
async fn an_address_that_asks_too_often_is_refused_without_asking_the_service() {
    let (world, _one) = asking().await;
    world.set("human_check.address_limit", json!(2)).await;
    let ada = world.person("ada").await;
    for _ in 0..2 {
        says(REFUSES);
        let reply = sign_in(
            &world,
            sign_in_body(&ada.username, PASSWORD),
            FROM,
            Some("bad"),
        )
        .await;
        assert_eq!(reply.problem().code, ErrorCode::HumanCheckFailed);
    }
    says(REFUSES);
    let third = sign_in(
        &world,
        sign_in_body(&ada.username, PASSWORD),
        FROM,
        Some("bad"),
    )
    .await;
    assert_eq!(third.problem().code, ErrorCode::RateLimited);
    assert_eq!(
        VENDOR.calls().len(),
        2,
        "the third was not asked of the service"
    );
}

// ---------------------------------------------------------------- what the pages are told, and the secret

#[tokio::test]
async fn meta_names_the_provider_and_site_key_and_never_the_secret() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    let meta = |world: &World| {
        let app = world.app.clone();
        async move { app.call(MetaGet, &[]).send().await.ok() }
    };
    assert!(
        meta(&world).await.human_check.is_none(),
        "off, nothing is said"
    );
    world.set("human_check.provider", json!("hcaptcha")).await;
    assert!(
        meta(&world).await.human_check.is_none(),
        "a choice with no site key asks nobody"
    );
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    keep_secret(&world, SECRET).await;
    let shown = meta(&world).await;
    let text = serde_json::to_string(&shown).unwrap();
    let check = shown.human_check.unwrap();
    assert_eq!(check.site_key, SITE_KEY);
    assert_eq!(
        serde_json::to_value(&check).unwrap()["provider"],
        "hcaptcha"
    );
    assert_eq!(serde_json::to_value(&check).unwrap()["mode"], "checkbox");
    assert_eq!(serde_json::to_value(&check).unwrap()["wait_secs"], 20);
    assert!(!text.contains(SECRET));
    // reCAPTCHA runs unseen whatever the setting of the box says, and says which way it is asked.
    world.set("human_check.provider", json!("recaptcha")).await;
    let google = serde_json::to_value(meta(&world).await.human_check.unwrap()).unwrap();
    assert_eq!(google["mode"], "invisible");
    assert_eq!(google["verification"], "siteverify");
    assert_eq!(google["alternate_host"], false);
}

#[tokio::test]
async fn a_mode_kept_from_an_earlier_choice_is_applied_to_hcaptcha_and_to_recaptcha_never() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    let shown = |world: &World| {
        let app = world.app.clone();
        async move {
            let meta = app.call(MetaGet, &[]).send().await.ok();
            serde_json::to_value(meta.human_check.unwrap()).unwrap()
        }
    };
    // hCaptcha takes the mode that is kept for it, whichever it is.
    world.set("human_check.provider", json!("hcaptcha")).await;
    for kept in ["invisible", "checkbox"] {
        world.set("human_check.mode", json!(kept)).await;
        assert_eq!(shown(&world).await["mode"], kept);
    }
    // Choosing reCAPTCHA does not apply it, nor does it clear it: the page is told it runs unseen.
    world.set("human_check.provider", json!("recaptcha")).await;
    for kept in ["checkbox", "invisible"] {
        world.set("human_check.mode", json!(kept)).await;
        assert_eq!(shown(&world).await["mode"], "invisible", "kept {kept}");
    }
    // Choosing hCaptcha again finds the mode as it was kept.
    world.set("human_check.mode", json!("checkbox")).await;
    world.set("human_check.provider", json!("hcaptcha")).await;
    assert_eq!(shown(&world).await["mode"], "checkbox");
    world.set("human_check.provider", json!("recaptcha")).await;
    assert_eq!(shown(&world).await["mode"], "invisible");
}

#[tokio::test]
async fn the_secret_is_written_once_and_read_back_never() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
    let root = world.person("root").await;
    world.make_admin(&root).await;
    let admin = world.sign_in(&root).await;
    let ada = world.person("ada").await;
    let signed = world.sign_in(&ada).await;
    world
        .app
        .call(MeAdminModeSet, &[])
        .token(admin.token())
        .body(&AdminModeSetBody { as_admin: true })
        .send()
        .await
        .ok();
    let state: HumanCheckState = world
        .app
        .call(AdminHumanCheckGet, &[])
        .token(admin.token())
        .send()
        .await
        .ok();
    assert!(!state.secret_set);
    let set = world
        .app
        .call(AdminHumanCheckSecretSet, &[])
        .token(admin.token())
        .body(&HumanCheckSecretBody {
            secret: format!("  {SECRET}\n"),
        })
        .send()
        .await;
    let kept = set.ok();
    assert!(kept.secret_set);
    assert!(!serde_json::to_string(&kept).unwrap().contains(SECRET));
    // Sealed: what is stored is not the secret, and the audit names the act and never the value.
    let stored: Vec<u8> = sqlx::query_scalar(
        "SELECT sealed FROM secrets WHERE owner_kind='setting' AND name='HUMAN_CHECK_SECRET'",
    )
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert!(!String::from_utf8_lossy(&stored).contains(SECRET));
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.human_check_secret_changed".to_owned())
    );
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action='auth.human_check_secret_changed'",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert!(!rows.iter().any(|row| row.to_string().contains(SECRET)));
    // The test of the service asks it with a token that is not one, and says what it found.
    world.set("human_check.provider", json!("hcaptcha")).await;
    world.set("human_check.site_key", json!(SITE_KEY)).await;
    says(REFUSES);
    let tested: HumanCheckTestReply = world
        .app
        .call(AdminHumanCheckTest, &[])
        .token(admin.token())
        .send()
        .await
        .ok();
    assert!(tested.ok, "{tested:?}");
    let orch_human_check::Body::Form(form) = &VENDOR.calls()[0].body else {
        panic!("a form");
    };
    assert!(
        form.iter()
            .any(|(name, value)| name == "secret" && value == SECRET),
        "trimmed"
    );
    says(WRONG_SECRET);
    let bad = world
        .app
        .call(AdminHumanCheckTest, &[])
        .token(admin.token())
        .send()
        .await
        .ok();
    assert!(!bad.ok);
    assert!(bad.error.unwrap().contains("secret"));
    // Taken away, nothing is kept.
    let gone = world
        .app
        .call(AdminHumanCheckSecretDelete, &[])
        .token(admin.token())
        .send()
        .await
        .ok();
    assert!(!gone.secret_set);
    // Somebody who is not a system admin may do none of it.
    let refused = world
        .app
        .call(AdminHumanCheckSecretSet, &[])
        .token(signed.token())
        .body(&HumanCheckSecretBody { secret: "x".into() })
        .send()
        .await;
    assert!(refused.body.is_err());
}

#[tokio::test]
async fn a_setting_that_is_not_a_choice_or_a_number_in_range_is_refused_when_it_is_saved() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    let world = World::new().await;
    for (key, value) in [
        ("human_check.mode", json!("a-tick-box")),
        ("human_check.min_score", json!(101)),
        ("human_check.min_score", json!(-1)),
        ("human_check.acts", json!(["sign_in", "teleport"])),
        ("human_check.verification", json!("magic")),
    ] {
        let mut conn = world.conn().await;
        let written = orch_settings::write::write(
            &mut conn,
            &orch_settings::write::Write {
                actor: orch_db::audit::Actor::System,
                target: orch_settings::Target::System,
                key,
                value,
                locked: None,
                expected_revision: None,
                may_lock: false,
                origin: &orch_db::audit::Origin::default(),
            },
            world.now(),
        )
        .await;
        assert!(written.is_err(), "{key}");
    }
}

// ---------------------------------------------------------------- the web edge, and a widget that did not load

/// The policy an edge sends with room for hCaptcha, as the entrypoint writes it.
fn room_for_hcaptcha() -> String {
    orch_human_check::policy_of("hcaptcha", false).unwrap()
}

/// A sign-in of `ada` from `ip` with no token, or with the word a page puts there when its widget did not load.
async fn sign_in_as(
    world: &World,
    ada: &support::Person,
    ip: &str,
    token: Option<&str>,
) -> Reply<orch_api_contract::auth::AuthResult> {
    sign_in(world, sign_in_body(&ada.username, PASSWORD), ip, token).await
}

#[tokio::test]
async fn a_page_that_could_not_load_its_widget_is_let_in_only_where_the_service_cannot_be_asked() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    // The service answers, refusing the token that is not one: nothing is wrong with it, so the word of a page is no
    // reason to let anybody in, and the person is asked for a check, as with no token at all.
    says(REFUSES);
    let problem = sign_in_as(&world, &ada, FROM, Some("unloaded"))
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckRequired);
    assert_eq!(VENDOR.calls().len(), 1, "the service was asked once, with a token of the server's own");
    // What was found is remembered: the next claim does not ask the service again (a second call would find it
    // unreachable, and let the person in).
    let again = sign_in_as(&world, &ada, "198.51.100.11", Some("unloaded"))
        .await
        .problem();
    assert_eq!(again.code, ErrorCode::HumanCheckRequired);
    assert_eq!(VENDOR.calls().len(), 1);
    // The service cannot be asked: no widget could have given a token, the page's word is true, and the setting
    // decides, as for any unreachable service.
    forget().await;
    VENDOR.then(Err(orch_human_check::Unreachable::new(
        orch_human_check::Why::Connect,
        "down",
    )));
    sign_in_as(&world, &ada, "198.51.100.12", Some("unloaded"))
        .await
        .ok();
    assert!(
        world
            .audit()
            .await
            .contains(&"auth.human_check_admitted".to_owned())
    );
    forget().await;
    world
        .set("human_check.when_unreachable", json!("refuse"))
        .await;
    VENDOR.then(Err(orch_human_check::Unreachable::new(
        orch_human_check::Why::Connect,
        "down",
    )));
    let refused = sign_in_as(&world, &ada, "198.51.100.13", Some("unloaded"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::HumanCheckUnavailable);
}

#[tokio::test]
async fn an_edge_whose_policy_has_no_room_for_the_service_lets_people_in_or_turns_them_away_as_the_setting_says()
 {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    // The edge of a release before the check: its policy lets a page load nothing from outside.
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    // Nobody can have a token to give, so the service is not asked, and the default lets people in: with no token, with
    // the word of a page whose widget did not load, and with a token that nothing could have made.
    for (ip, token) in [
        ("198.51.100.30", None),
        ("198.51.100.31", Some("unloaded")),
        ("198.51.100.32", Some("a-token")),
    ] {
        sign_in_as(&world, &ada, ip, token).await.ok();
    }
    assert!(VENDOR.calls().is_empty(), "the service was not asked");
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action = 'auth.human_check_admitted' ORDER BY at, id",
    )
    .fetch_all(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "one row an hour for an act");
    assert_eq!(rows[0]["why"], "edge");
    assert_eq!(rows[0]["act"], "sign_in");
    // Chosen to refuse, it turns them away with the word of a service that cannot be reached.
    world
        .set("human_check.when_unreachable", json!("refuse"))
        .await;
    forget().await;
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    let problem = sign_in_as(&world, &ada, "198.51.100.33", None)
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckUnavailable);
    // Once the edge has room, the check is asked as it always was.
    forget().await;
    EDGE.serving(STACK_EDGE, &[&room_for_hcaptcha()]);
    let problem = sign_in_as(&world, &ada, "198.51.100.34", None)
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckRequired);
    says(PASSES);
    sign_in_as(&world, &ada, "198.51.100.35", Some("t"))
        .await
        .ok();
    // An edge that cannot be read says nothing, and nothing changes.
    forget().await;
    let problem = sign_in_as(&world, &ada, "198.51.100.36", None)
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckRequired);
}

#[tokio::test]
async fn the_policy_a_browser_is_given_through_a_front_decides_over_the_one_inside_the_stack() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    // A front of the operator's own that sets the policy: the stack's edge has room, what people are given has not.
    EDGE.serving(STACK_EDGE, &[&room_for_hcaptcha()]);
    let room = room_for_hcaptcha();
    EDGE.serving(PUBLIC_EDGE, &[room.as_str(), "default-src 'self'"]);
    sign_in_as(&world, &ada, "198.51.100.40", None).await.ok();
    assert!(VENDOR.calls().is_empty());
    // And a front that gives room where the stack's own edge gives none is what a browser gets.
    forget().await;
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    EDGE.serving(PUBLIC_EDGE, &[&room_for_hcaptcha()]);
    let problem = sign_in_as(&world, &ada, "198.51.100.41", None)
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckRequired);
}

#[tokio::test]
async fn the_check_is_not_switched_on_while_the_edge_has_no_room_for_it() {
    install();
    let _one = ONE_AT_A_TIME.lock().await;
    forget().await;
    let world = World::new().await;
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
    let put = |key: &'static str, value: Value| {
        let (world, token) = (&world, admin.token().to_owned());
        async move {
            world
                .app
                .call(AdminSettingSet, &[&key])
                .token(&token)
                .body(&AdminSettingSetBody {
                    value,
                    locked: None,
                    expected_revision: None,
                })
                .send()
                .await
        }
    };
    let state = || async {
        let state: HumanCheckState = world
            .app
            .call(AdminHumanCheckGet, &[])
            .token(admin.token())
            .send()
            .await
            .ok();
        state
    };
    // The edge of a release before the check: switching hCaptcha on is refused, with what the policy leaves out, and
    // the setting stays where it was.
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    let refused = put("human_check.provider", json!("hcaptcha"))
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::HumanCheckEdgeBlocked);
    assert_eq!(refused.status, 409);
    assert_eq!(refused.extra["via"], "stack");
    let lacking = refused.extra["lacking"].as_array().unwrap();
    assert!(
        lacking
            .iter()
            .any(|one| one == "https://hcaptcha.com in script-src"),
        "{lacking:?}"
    );
    assert!(state().await.edge.is_none(), "no service is chosen");
    // Where the policy cannot be read, nothing is known, and nothing is refused.
    forget().await;
    put("human_check.provider", json!("hcaptcha")).await.ok();
    put("human_check.provider", json!("off")).await.ok();
    // Where it has room, the service is chosen, and the state says what the policy allows.
    EDGE.serving(STACK_EDGE, &[&room_for_hcaptcha()]);
    put("human_check.provider", json!("hcaptcha")).await.ok();
    put("human_check.site_key", json!(SITE_KEY)).await.ok();
    let edge = state().await.edge.unwrap();
    assert_eq!(edge.state, HumanCheckEdgeState::Allows);
    assert_eq!(edge.via, Some(HumanCheckEdgeVia::Stack));
    assert!(edge.lacking.is_empty());
    // Changing what a service asks of the edge is looked at too: another service, and the alternate address.
    let other = put("human_check.provider", json!("recaptcha"))
        .await
        .problem();
    assert_eq!(other.code, ErrorCode::HumanCheckEdgeBlocked);
    EDGE.serving(
        STACK_EDGE,
        &[&orch_human_check::policy_of("recaptcha", false).unwrap()],
    );
    put("human_check.provider", json!("recaptcha")).await.ok();
    let alternate = put("human_check.alternate_host", json!(true))
        .await
        .problem();
    assert_eq!(alternate.code, ErrorCode::HumanCheckEdgeBlocked);
    EDGE.serving(
        STACK_EDGE,
        &[&orch_human_check::policy_of("recaptcha", true).unwrap()],
    );
    put("human_check.alternate_host", json!(true)).await.ok();
    // A choice that is on stays changeable when the edge is changed under it: only moving it is looked at, and
    // the state says what the policy leaves out.
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    put("human_check.min_score", json!(60)).await.ok();
    put("human_check.site_key", json!("another-key")).await.ok();
    let edge = state().await.edge.unwrap();
    assert_eq!(edge.state, HumanCheckEdgeState::Blocks);
    assert!(
        edge.lacking
            .iter()
            .any(|one| one == "https://www.recaptcha.net/recaptcha/ in script-src"),
        "{:?}",
        edge.lacking
    );
    // And off is always allowed.
    put("human_check.provider", json!("off")).await.ok();
}

#[tokio::test]
async fn what_was_found_of_the_edge_is_used_while_a_new_look_is_made_and_nobody_waits_for_it() {
    let (world, _one) = asking().await;
    let ada = world.person("ada").await;
    world.set("human_check.look_secs", json!(3)).await;
    EDGE.serving(STACK_EDGE, &[POLICY_BEFORE]);
    sign_in_as(&world, &ada, "198.51.100.50", None).await.ok();
    assert_eq!(EDGE.asked().len(), 2, "the address people open, and the edge inside, once");
    // Within the time, nothing is looked at again.
    sign_in_as(&world, &ada, "198.51.100.51", None).await.ok();
    assert_eq!(EDGE.asked().len(), 2);
    // The edge is set right. After the time, the first person is still told what was found, and a new look is made
    // behind them; the next is told what it found.
    EDGE.serving(STACK_EDGE, &[&room_for_hcaptcha()]);
    tokio::time::sleep(std::time::Duration::from_millis(3300)).await;
    sign_in_as(&world, &ada, "198.51.100.52", None).await.ok();
    for _ in 0..40 {
        if EDGE.asked().len() >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(EDGE.asked().len(), 4, "one new look, made once");
    let problem = sign_in_as(&world, &ada, "198.51.100.53", None)
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::HumanCheckRequired);
}
