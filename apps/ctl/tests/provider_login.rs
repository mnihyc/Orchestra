//! A provider access token piped into the command-line tool signs in through a real API and database, without secret arguments, output or journal entries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "../../../crates/control/api/tests/places/mod.rs"]
mod places;
#[path = "../../../crates/control/api/tests/rig/mod.rs"]
mod rig;
#[path = "../../../crates/control/api/tests/support/mod.rs"]
mod support;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_api_core::registry::Registry;
use orch_api_core::routes::resources::sealing::{binding, install_master_key};
use orch_db::queries::resources::{access_fixtures, hosting, provider_runtime};
use orch_db::secrets::MasterKey;
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, Empty, ProviderRefreshResult, Secret, SecretOrText,
};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Json, Time, new_id};
use serde_json::json;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt as _;

#[tokio::test(flavor = "multi_thread")]
async fn a_piped_access_token_signs_in_and_is_in_no_argument_output_or_control_journal() {
    install_master_key(MasterKey::new([7; 32]));
    let s = rig::Scene::new().await;
    let now = s.rig.world.now();
    let mut declared = rig::descriptor();
    declared["native_provider"]["login_methods"] = json!([{"id":"access_token","title":"Access token","summary":"Bring one","kind":"import","fields":[{"name":"access_token","label":"Token","type":"secret","required":true,"help":null,"choices":null,"default":null,"max_bytes":4096}],"yields":"token","lifetime_secs":null,"requires":null,"recommended":false}]);
    let reported = declared.clone();
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: false,
                harness_version: "1.0.0".to_owned(),
                descriptor: serde_json::from_value(reported.clone()).unwrap(),
            },
        ))
    });
    {
        let mut conn = s.rig.db.acquire().await.unwrap();
        let host = access_fixtures::insert_resource(
            &mut conn,
            "host",
            Some(s.ada.id()),
            Some(s.tree.org.base.id),
            "CLI runtime host",
        )
        .await
        .unwrap();
        let template = access_fixtures::insert_resource(
            &mut conn,
            "template",
            Some(s.ada.id()),
            Some(s.tree.org.base.id),
            "CLI provider template",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE env_templates SET kind = 'provider',driver = 'docker' WHERE id = $1")
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
            .bind(s.on.agent)
            .bind(&declared)
            .execute(&mut *conn)
            .await
            .unwrap();
        let runtime = new_id();
        provider_runtime::insert_runtime(
            &mut conn,
            &provider_runtime::NewRuntime {
                id: runtime,
                host_id: host,
                unit_name: "orch-plan-cli-runtime",
                provider_id: s.on.provider,
                driver: "docker",
                template_id: template,
                spec_hash: &[0; 32],
                generation: 1,
                volumes: &json!({}),
            },
            now,
        )
        .await
        .unwrap();
        hosting::adopt_machine(&mut conn, s.rig.machine_id, runtime, now)
            .await
            .unwrap();
        sqlx::query("UPDATE runtimes SET state = 'running' WHERE id = $1")
            .bind(runtime)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let token = format!("{}.{}.ZmFrZQ",URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#),URL_SAFE_NO_PAD.encode(json!({"iat":now.timestamp(),"exp":(now+chrono::Duration::days(10)).timestamp(),"https://api.openai.com/auth":{"chatgpt_account_id":"test-account","chatgpt_plan_type":"plus"}}).to_string()));
    let received: Arc<Mutex<Option<String>>> = Arc::default();
    let got = received.clone();
    let db = s.rig.db.clone();
    let machine = Arc::downgrade(&s.rig.machine);
    let provider = s.on.provider;
    let runtime = tokio::runtime::Handle::current();
    s.rig.machine.handler.on(CommandKind::ProviderLoginStart,move |command| {
        let Command::ProviderLoginStart(start) = command else {unreachable!()};
        let SecretOrText::Secret(Secret::Ref(id)) = start.values.as_ref().unwrap().get("access_token").unwrap() else {panic!("the import remains sealed until the runtime takes it")};
        let id = *id;
        let plain = tokio::task::block_in_place(|| runtime.block_on(async {
            let mut conn = db.acquire().await.unwrap();
            let row = orch_db::queries::link::stored_secret(&mut conn,id).await.unwrap().unwrap();
            MasterKey::new([7;32]).open(&row.sealed,&binding(&row.owner_kind,row.owner_id,&row.name)).unwrap()
        }));
        *got.lock().unwrap() = Some(String::from_utf8(plain).unwrap());
        let machine = machine.upgrade().unwrap();
        runtime.spawn(async move {machine.publish(&Stream::Provider {provider_id:provider},&Event::ProviderLogin(serde_json::from_value(json!({"state":"completed","method":"access_token","shows":[],"asks":null,"expires_at":null,"error":null})).unwrap())).await;});
        Ok(CommandResult::ProviderLoginStart(Empty {}))
    });
    s.rig.machine.handler.on(CommandKind::ProviderRefresh,move |_| Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {source:None,status:"ready".into(),facts:serde_json::from_value(json!({"plan":"plus","windows":[],"credential":{"method":"access_token","yields":"token","issued_at":Time::new(now),"expires_at":Time::new(now+chrono::Duration::days(10))}})).unwrap()})));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = orch_api_core::server::router(s.rig.world.app.app.clone(), registry);
    let mut listener = None;
    for port in 41470..41480 {
        if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            listener = Some(bound);
            break;
        }
    }
    let listener = listener.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move {
        orch_api_core::server::serve_plain(listener, router, std::future::pending())
            .await
            .unwrap();
    });
    let api = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::API_TOKEN_PREFIX,
    );
    let hash = orch_api_core::middleware::authentication::token_hash(&api);
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::identity::testing::insert_api_token(&mut conn, s.ada.id(), &hash)
        .await
        .unwrap();
    // The command reads the provider's placement before starting its managed sign-in.
    // Token scope names are independent: management does not imply a view scope.
    sqlx::query("UPDATE api_tokens SET scopes = ARRAY['provider.view', 'provider.manage'] WHERE token_hash = $1")
        .bind(hash)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let arguments = vec![
        "provider".to_owned(),
        "login".to_owned(),
        provider.to_string(),
        "--method".to_owned(),
        "access_token".to_owned(),
        "--values-stdin".to_owned(),
    ];
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_orchctl"));
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("ORCH_URL", url)
        .env("ORCH_TOKEN", api)
        .args(&arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(
            serde_json::to_vec(&json!({"access_token":token}))
                .unwrap()
                .as_slice(),
        )
        .await
        .unwrap();
    input.shutdown().await.unwrap();
    drop(input);
    let output = child.wait_with_output().await.unwrap();
    serving.abort();
    assert!(
        output.status.success(),
        "the import command ended {} (stdout {} bytes, stderr {} bytes)",
        output.status,
        output.stdout.len(),
        output.stderr.len()
    );
    let standing = String::from_utf8_lossy(&output.stdout);
    assert!(matches!(standing.trim(), "starting" | "completed"));
    rig::eventually("the imported sign-in completes", || async {
        let state: Option<Json> =
            sqlx::query_scalar("SELECT login_state FROM providers WHERE id = $1")
                .bind(provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
        state.is_some_and(|state| state.get("state").and_then(Json::as_str) == Some("completed"))
    })
    .await;
    assert_eq!(received.lock().unwrap().as_deref(), Some(token.as_str()));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(&token));
    assert!(arguments.iter().all(|argument| !argument.contains(&token)));
    let journal: Vec<Json> = sqlx::query_scalar("SELECT body FROM commands")
        .fetch_all(s.rig.db.pool())
        .await
        .unwrap();
    assert!(!serde_json::to_string(&journal).unwrap().contains(&token));
    let audit: Vec<Json> = sqlx::query_scalar("SELECT details FROM audit_log")
        .fetch_all(s.rig.db.pool())
        .await
        .unwrap();
    assert!(!serde_json::to_string(&audit).unwrap().contains(&token));
}
