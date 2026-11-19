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
        let result = tokio::time::timeout(SERVER_BUDGET, work).await;
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
            "{label} exceeded the {SERVER_BUDGET:?} server budget with {} reading clients",
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
    let tabs = vec![signed.token().to_owned(); count];
    under_load(
        &world,
        &tabs,
        "change password",
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
