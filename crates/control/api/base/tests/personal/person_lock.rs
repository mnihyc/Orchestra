//! A person's own busy browsers can read throughout a credential change.
use crate::support::{PASSWORD, World};
use futures::future::join_all;
use orch_api_contract::auth::{
    AdminModeSetBody, ApiTokenCreateBody, AuthRefresh, MeAdminModeSet, MeGet, MePasswordChange,
    MePasswordChangeBody, MeSessionDelete, MeTokenCreate, SensitiveProof,
};
use orch_api_contract::common::CSRF_HEADER;
use orch_api_core::middleware::authentication::{
    ACCESS_TOKEN_PREFIX, SESSION_PREFIX, new_token, token_hash,
};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::queries::{identity, transaction_guard};
use orch_types::{Id, new_id};
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::{Barrier, watch};

// Leave one ordinary 200 ms round trip in the owner's one-second interaction budget.
const SERVER_BUDGET: Duration = Duration::from_millis(800);
const CLIENT_RTT: Duration = Duration::from_millis(200);

async fn world_for_clients(clients: usize) -> World {
    let mut world = World::new().await;
    // A connection for each client, with room for an act and its nested reads, as on the installation.
    let capacity = clients + 4;
    let db = orch_db::Db::connect(&world.db.url, capacity.try_into().unwrap())
        .await
        .unwrap();
    let mut leases = Vec::new();
    for _ in 0..capacity {
        leases.push(db.acquire().await.unwrap());
    }
    drop(leases);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    world.app = TestApp::new(db, world.app.clock.clone(), registry);
    world
}

async fn browser(world: &World, user: Id) -> (Id, String) {
    let session = new_id();
    let bearer = new_token(ACCESS_TOKEN_PREFIX);
    let end = world.now() + chrono::Duration::hours(1);
    let mut tx = world.db.pool.begin().await.unwrap();
    identity::create_session(
        &mut tx,
        &identity::NewSession {
            id: session,
            user_id: user,
            refresh_hash: &token_hash(&new_token(SESSION_PREFIX)),
            user_agent: "person-lock fixture",
            ip: None,
            at: world.now(),
            expires_at: end,
        },
    )
    .await
    .unwrap();
    identity::add_access_token(&mut tx, &token_hash(&bearer), session, end)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (session, bearer)
}

async fn read_page(world: &World, bearer: &str) {
    let response = tokio::time::timeout(
        SERVER_BUDGET,
        world.app.call(MeGet, &[]).token(bearer).send_bytes(),
    )
    .await
    .expect("a reader finishes within its own server budget");
    assert!(
        response.status.is_success(),
        "an active reader received {}",
        response.status
    );
}

async fn under_load<T>(
    world: &World,
    clients: &[String],
    label: &str,
    work: impl Future<Output = T>,
) -> T {
    within(world, clients, label, SERVER_BUDGET, work).await
}

/// The Argon2id hash a password change makes beyond what sign-in and token creation make (they each
/// check one password; a change checks the current one and hashes the new one). Its parameters are
/// `MEMORY_KIB` 19 MiB, `PASSES` 2 and `LANES` 1 in sign_in/passwords.rs, the OWASP minimum: 0.29 to
/// 0.31 s in this unoptimized test build on the hosted runners (where a sign-in with its one hash took
/// 0.36 to 0.70 s and a password change 0.67 to 0.73 s), tens of milliseconds in a release build.
/// Measured here, the fastest of three, before any client reads, so that the bound follows the
/// machine the case runs on.
async fn second_hash() -> Duration {
    let mut fastest = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        orch_api_core::sign_in::passwords::hash("a measured long password")
            .await
            .unwrap();
        fastest = fastest.min(start.elapsed());
    }
    fastest
}

async fn within<T>(
    world: &World,
    clients: &[String],
    label: &str,
    budget: Duration,
    work: impl Future<Output = T>,
) -> T {
    let ready = Barrier::new(clients.len() + 1);
    let (finished, _) = watch::channel(false);
    let readers = join_all(clients.iter().map(|bearer| {
        let mut done = finished.subscribe();
        let ready = &ready;
        async move {
            read_page(world, bearer).await;
            ready.wait().await;
            loop {
                read_page(world, bearer).await;
                tokio::select! {
                    _ = done.changed() => break,
                    () = tokio::time::sleep(CLIENT_RTT) => {}
                }
            }
        }
    }));
    let act = async {
        ready.wait().await;
        let start = Instant::now();
        let result = tokio::time::timeout(budget, work).await;
        eprintln!(
            "personal act={label} clients={} elapsed={:?}",
            clients.len(),
            start.elapsed()
        );
        finished.send_replace(true);
        result
    };
    let ((), result) = tokio::join!(
        async {
            readers.await;
        },
        act
    );
    result.unwrap_or_else(|_| {
        panic!(
            "{label} exceeded the {budget:?} server budget with {} reading clients",
            clients.len()
        )
    })
}

async fn exercise(count: usize) {
    let world = world_for_clients(count).await;
    let person = world.person("reader").await;
    world.make_admin(&person).await;
    let signed = world.sign_in(&person).await;
    let (victim, revoked) = browser(&world, person.id).await;
    let mut clients = Vec::new();
    for _ in 0..count {
        clients.push(browser(&world, person.id).await.1);
    }
    // Keep the observed native identity lock continuously present. Ordinary page traffic can
    // otherwise leave a scheduling gap between readers, accidentally letting an exclusive lock pass.
    let mut reading = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_native_actor(&mut reading, person.id)
        .await
        .unwrap()
        .unwrap();
    under_load(
        &world,
        &clients,
        "sign in",
        world.password_login(&person.username, PASSWORD, "203.0.113.1", &signed.browser()),
    )
    .await
    .ok();
    under_load(
        &world,
        &clients,
        "refresh",
        world
            .app
            .call(AuthRefresh, &[])
            .header("cookie", &signed.browser())
            .header(CSRF_HEADER, "1")
            .send(),
    )
    .await
    .ok();
    let mode = under_load(
        &world,
        &clients,
        "admin mode",
        world
            .app
            .call(MeAdminModeSet, &[])
            .token(signed.token())
            .body(&AdminModeSetBody { as_admin: true })
            .send(),
    )
    .await
    .ok();
    assert!(mode.as_admin);
    under_load(
        &world,
        &clients,
        "create token",
        world
            .app
            .call(MeTokenCreate, &[])
            .token(signed.token())
            .body(&ApiTokenCreateBody {
                name: "personal check".into(),
                scopes: Some(vec!["*".into()]),
                expires_at: None,
                node_id: None,
                rules: None,
                proof: Some(SensitiveProof {
                    password: Some(PASSWORD.into()),
                    ..SensitiveProof::default()
                }),
            })
            .send(),
    )
    .await
    .ok();
    under_load(
        &world,
        &clients,
        "revoke session",
        world
            .app
            .call(MeSessionDelete, &[&victim])
            .token(signed.token())
            .send(),
    )
    .await
    .ok();
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&revoked)
            .send_bytes()
            .await
            .status,
        401
    );

    let at = world.now() + chrono::Duration::seconds(1);
    under_load(&world, &clients, "system writes", async {
        let mut tx = world.db.pool.begin().await.unwrap();
        identity::touch_last_seen(&mut tx, person.id, at)
            .await
            .unwrap();
        assert!(
            identity::add_inbox_row(
                &mut tx,
                &identity::NewInboxRow {
                    user_id: person.id,
                    kind: "system.message",
                    title: "Notice",
                    body: "",
                    data: &serde_json::json!({}),
                    dedupe_key: "person-lock/check",
                    at
                }
            )
            .await
            .unwrap()
            .is_some()
        );
        tx.commit().await.unwrap();
        let seen: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT last_seen_at FROM users WHERE id=$1")
                .bind(person.id)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(seen, at);
    })
    .await;

    // Changing the password deliberately ends the other sessions. The tabs of the requesting
    // session must keep reading, and the ended browsers must receive the intended refusal.
    // Its bound is the server budget of every act plus the one hash it makes beyond sign-in, and
    // half that hash again as the margin for the readers' share of the processors; a wait behind
    // the readers' locks (the fault this case holds: 1.5 to 15 s before the repair) still exceeds it.
    let hash = second_hash().await;
    let budget = SERVER_BUDGET + hash + hash / 2;
    eprintln!("personal act=change password second hash={hash:?} bound={budget:?}");
    let tabs = vec![signed.token().to_owned(); count];
    within(
        &world,
        &tabs,
        "change password",
        budget,
        world
            .app
            .call(MePasswordChange, &[])
            .token(signed.token())
            .body(&MePasswordChangeBody {
                current_password: Some(PASSWORD.into()),
                new_password: "a different long password".into(),
            })
            .send(),
    )
    .await
    .ok();
    reading.rollback().await.unwrap();
    read_page(&world, signed.token()).await;
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(&clients[0])
            .send_bytes()
            .await
            .status,
        401
    );
}

#[tokio::test]
async fn a_page_and_a_native_identity_read_can_finish_before_a_personal_writer_commits() {
    let world = World::new().await;
    let person = world.person("reader").await;
    let (_, bearer) = browser(&world, person.id).await;
    let writer = transaction_guard::request_scope(
        transaction_guard::Scope::Personal {
            user: Some(person.id),
        },
        world.app.app.db.begin(),
    )
    .await
    .unwrap();
    read_page(&world, &bearer).await;
    let mut reader = world.db.pool.begin().await.unwrap();
    tokio::time::timeout(
        SERVER_BUDGET,
        transaction_guard::lock_native_actor(&mut reader, person.id),
    )
    .await
    .expect("native identity reader can finish while the personal account write is open")
    .unwrap()
    .unwrap();
    reader.commit().await.unwrap();
    writer.rollback().await.unwrap();
}

#[tokio::test]
async fn five_clients_can_read_during_each_personal_act() {
    exercise(5).await;
}

#[tokio::test]
async fn twenty_clients_can_read_during_each_personal_act() {
    exercise(20).await;
}

#[tokio::test]
async fn ending_one_session_orders_its_admitted_action_while_another_session_keeps_working() {
    use transaction_guard::{Credential, lock_identity_for_write};

    let world = World::for_requests().await;
    let person = world.person("reader").await;
    let acting = world.sign_in(&person).await;
    let (target_id, target_bearer) = browser(&world, person.id).await;
    let (other_id, other_bearer) = browser(&world, person.id).await;
    let opened = async |session, bearer: &str| {
        let mut tx = world.db.pool.begin().await.unwrap();
        lock_identity_for_write(
            &mut tx,
            person.id,
            Credential::Session {
                id: session,
                hash: &token_hash(bearer),
            },
            false,
            Default::default(),
        )
        .await
        .unwrap()
        .expect("the action was admitted");
        tx
    };
    let mut target = opened(target_id, &target_bearer).await;
    let unrelated = opened(other_id, &other_bearer).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *target)
        .await
        .unwrap();
    let ids: [&dyn std::fmt::Display; 1] = [&target_id];
    let request = world
        .app
        .call(MeSessionDelete, &ids)
        .token(acting.token())
        .send();
    tokio::pin!(request);
    let blocked = async {
        loop {
            let waits:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(a.pid)))")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if waits {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        _ = &mut request => panic!("revocation finished ahead of an admitted action"),
        observation = tokio::time::timeout(Duration::from_secs(5),blocked) => observation.expect("observe the actual revocation waiting in PostgreSQL"),
    }
    target.commit().await.unwrap();
    tokio::time::timeout(SERVER_BUDGET, &mut request)
        .await
        .expect("the unrelated session is still open")
        .ok();
    let mut later = world.db.pool.begin().await.unwrap();
    assert!(
        lock_identity_for_write(
            &mut later,
            person.id,
            Credential::Session {
                id: target_id,
                hash: &token_hash(&target_bearer)
            },
            false,
            Default::default()
        )
        .await
        .unwrap()
        .is_none()
    );
    later.rollback().await.unwrap();
    unrelated.commit().await.unwrap();
    read_page(&world, &other_bearer).await;
}

#[tokio::test]
async fn a_browser_can_sign_out_while_its_page_and_four_other_sessions_are_reading() {
    use orch_api_contract::auth::{AuthLogout, MeBootstrapGet};
    use orch_api_core::testing::{Boundaries, follow};
    use std::sync::Arc;
    use tokio::sync::Notify;

    let world = world_for_clients(5).await;
    let person = world.person("reader").await;
    let signed = world.sign_in(&person).await;
    let mut others = Vec::new();
    for _ in 0..4 {
        others.push(browser(&world, person.id).await.1);
    }
    let log = Boundaries::new();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let arrived = entered.clone();
    let resume = release.clone();
    log.hold(move |route, phase| {
        let arrived = arrived.clone();
        let resume = resume.clone();
        Box::pin(async move {
            if route == "MeBootstrapGet" && phase == "page.project" {
                arrived.notify_one();
                resume.notified().await;
            }
        })
    });
    let page = follow(
        log,
        world
            .app
            .call(MeBootstrapGet, &[])
            .token(signed.token())
            .send_bytes(),
    );
    let logout = async {
        tokio::time::timeout(Duration::from_secs(10), entered.notified())
            .await
            .expect("the browser started rendering its page");
        under_load(
            &world,
            &others,
            "sign out with an open page",
            world
                .app
                .call(AuthLogout, &[])
                .header("cookie", &signed.browser())
                .header(CSRF_HEADER, "1")
                .send(),
        )
        .await
        .ok();
        release.notify_one();
    };
    let (reply, ()) = tokio::join!(page, logout);
    assert_eq!(
        reply.status, 401,
        "a page completed after sign-out must refuse the ended session"
    );
    for bearer in &others {
        read_page(&world, bearer).await;
    }
}

#[tokio::test]
async fn signing_out_ends_only_the_cookie_session_during_another_credential_change() {
    use orch_api_contract::auth::AuthLogout;

    let world = world_for_clients(2).await;
    let person = world.person("reader").await;
    let signed = world.sign_in(&person).await;
    let (_, other) = browser(&world, person.id).await;
    let mut changing = world.db.pool.begin().await.unwrap();
    transaction_guard::lock_credential_writer(&mut changing, person.id)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(
        SERVER_BUDGET,
        world
            .app
            .call(AuthLogout, &[])
            .header("cookie", &signed.browser())
            .header(CSRF_HEADER, "1")
            .send(),
    )
    .await
    .expect("ending this cookie does not need the account writer")
    .ok();
    changing.rollback().await.unwrap();
    assert_eq!(
        world
            .app
            .call(MeGet, &[])
            .token(signed.token())
            .send_bytes()
            .await
            .status,
        401
    );
    read_page(&world, &other).await;
}
