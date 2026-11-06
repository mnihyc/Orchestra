//! Release channels stay silent until effective policy or an explicit refresh asks for them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use orch_api_contract::admin::{
    AdminHarnessChannelsGet, AdminHarnessChannelsRead, HarnessChannelSchedule,
};
use orch_db::queries::settings::{self as rows, Stored};
use orch_types::{Id, Time};
use serde_json::{Value, json};

use super::{World, cast, channel_build, keep, system_host};

/// A local policy read must return promptly; it has no vendor or node work to wait for.
const LOCAL_LOOK: Duration = Duration::from_secs(10);
const WITHOUT_CHECKSUM: &str = "0.0.1-without-checksum";

struct Mirror {
    url: String,
    calls: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Mirror {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mirror {
    async fn start() -> Self {
        let mut listener = None;
        for port in 41000..=41999 {
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.expect("a local test port");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let service = Router::new().fallback(reply).with_state(calls.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, service).await.unwrap();
        });
        Self { url, calls, task }
    }
    fn paths(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

async fn reply(State(calls): State<Arc<Mutex<Vec<String>>>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    calls.lock().unwrap().push(path.clone());
    if path.ends_with(&format!("/tags/rust-v{}", super::VERSION)) {
        return axum::Json(json!({"tag_name":format!("rust-v{}",super::VERSION),"draft":false,"prerelease":false,"assets":[{"name":"codex-package-x86_64-unknown-linux-musl.tar.gz","digest":format!("sha256:{}","c".repeat(64)),"browser_download_url":"https://github.com/openai/codex/exact.tar.gz"}]})).into_response();
    }
    if path.ends_with(&format!("/tags/rust-v{WITHOUT_CHECKSUM}")) {
        return axum::Json(json!({"tag_name":format!("rust-v{WITHOUT_CHECKSUM}"),"draft":false,"prerelease":false,"assets":[{"name":"codex-package-x86_64-unknown-linux-musl.tar.gz","digest":null,"browser_download_url":"https://github.com/openai/codex/no-checksum.tar.gz"}]})).into_response();
    }
    if path.starts_with("/api.github.com/") {
        return axum::Json(json!([])).into_response();
    }
    if path.ends_with("/latest") || path.ends_with("/stable") {
        return "2.1.309".into_response();
    }
    axum::Json(json!({"version":"2.1.309","platforms":{}})).into_response()
}

async fn setting(world: &World, node: Option<Id>, key: &str, value: Value, locked: bool) {
    let mut conn = world.conn().await;
    let written = rows::put(
        &mut conn,
        node,
        key,
        &Stored {
            value: &value,
            locked,
            by: None,
            at: world.now(),
        },
        None,
    )
    .await
    .unwrap();
    assert!(matches!(written, rows::Put::Written(_)));
}

async fn mirror_for(world: &World) -> Mirror {
    let mirror = Mirror::start().await;
    setting(
        world,
        None,
        "agents.harness_mirror",
        json!(mirror.url),
        false,
    )
    .await;
    mirror
}

async fn trust_mirror(world: &World) {
    setting(
        world,
        None,
        "agents.harness_trust_mirror",
        json!(true),
        false,
    )
    .await;
}

#[tokio::test]
async fn a_typed_exact_release_uses_its_own_manifest_checksum_and_background_native_check() {
    use orch_api_contract::admin::{
        HarnessBuildSource, HarnessBuildStatus, HarnessVersionFetch, HarnessVersionFetchBody,
    };
    let world = World::new().await;
    let people = cast(&world).await;
    let mirror = mirror_for(&world).await;
    trust_mirror(&world).await;
    let machine = system_host(&world).await;
    let answering = super::node_answering(&world, machine, &[]);
    world
        .app
        .call(HarnessVersionFetch, &[&"codex", &super::VERSION])
        .token(&people.root.token)
        .body(&HarnessVersionFetchBody::default())
        .send()
        .await
        .ok();
    assert_eq!(
        answering.await.unwrap(),
        [
            "node.tool.ensure",
            "node.tool.check",
            "node.tool.conformance"
        ]
    );
    let build = super::checked(&world, &people.root).await;
    assert_eq!(build.version, super::VERSION);
    assert_eq!(build.status, HarnessBuildStatus::Checked);
    assert_eq!(build.source, HarnessBuildSource::Mirror);
    let ensure: Value =
        sqlx::query_scalar("SELECT body FROM commands WHERE kind='node.tool.ensure'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(ensure["sha256"], "c".repeat(64));
    assert_eq!(
        mirror.paths(),
        [format!(
            "/api.github.com/repos/openai/codex/releases/tags/rust-v{}",
            super::VERSION
        )]
    );
    let refused = world
        .app
        .call(HarnessVersionFetch, &[&"codex", &WITHOUT_CHECKSUM])
        .token(&people.root.token)
        .body(&HarnessVersionFetchBody::default())
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    assert!(
        mirror
            .paths()
            .iter()
            .any(|path| path.ends_with(&format!("/tags/rust-v{WITHOUT_CHECKSUM}")))
    );
    let faults: Vec<orch_api_contract::common::FieldFault> =
        serde_json::from_value(refused.extra["errors"].clone()).unwrap();
    assert!(
        faults
            .iter()
            .any(|fault| fault.path == "version" && fault.message.contains("checksum")),
        "{faults:?}"
    );
}

#[tokio::test]
async fn shipped_defaults_contact_neither_a_vendor_nor_a_node_even_when_an_unchecked_build_is_kept()
{
    let world = World::new().await;
    let mirror = mirror_for(&world).await;
    keep(&world, &channel_build()).await;
    system_host(&world).await;
    let looked = tokio::time::timeout(
        LOCAL_LOOK,
        orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!looked);
    assert!(mirror.paths().is_empty());
    let commands: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM commands")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        commands, 0,
        "shipped policy does not fetch an unasked build for checking"
    );
    let state = orch_api_core::harness_channels::status(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(state.reason, HarnessChannelSchedule::Shipped);
    assert_eq!(state.last_read_at, None);
}

#[tokio::test]
async fn an_org_requests_only_its_vendor_and_an_ancestor_lock_can_take_that_request_back() {
    let world = World::new().await;
    let people = cast(&world).await;
    let mirror = mirror_for(&world).await;
    trust_mirror(&world).await;
    setting(
        &world,
        Some(people.acme),
        "agents.harness_versions",
        json!({"codex":{"use":"newest_checked"}}),
        false,
    )
    .await;
    assert!(
        orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    let paths = mirror.paths();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].starts_with("/api.github.com/"), "{paths:?}");
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert_eq!(
        mirror.paths(),
        paths,
        "the configured interval applies across scheduler looks"
    );
    setting(&world, None, "agents.harness_versions", json!({}), true).await;
    world.advance(chrono::Duration::hours(25));
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert_eq!(
        mirror.paths(),
        paths,
        "an ineffective old override is not consent to a channel request"
    );
    let state = orch_api_core::harness_channels::status(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(state.reason, HarnessChannelSchedule::Shipped);
}

#[tokio::test]
async fn zero_disables_the_schedule_but_an_admin_refresh_still_reads_and_keeps_its_time() {
    let world = World::new().await;
    let people = cast(&world).await;
    let mirror = mirror_for(&world).await;
    trust_mirror(&world).await;
    setting(
        &world,
        None,
        "agents.harness_versions",
        json!({"codex":{"use":"newest"}}),
        false,
    )
    .await;
    setting(
        &world,
        None,
        "agents.harness_channel_hours",
        json!(0),
        false,
    )
    .await;
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert!(mirror.paths().is_empty());
    let before = world
        .app
        .call(AdminHarnessChannelsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(before.reason, HarnessChannelSchedule::Disabled);
    assert!(
        mirror.paths().is_empty(),
        "reading the status itself contacts no vendor"
    );
    world
        .app
        .call(AdminHarnessChannelsRead, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    let paths = mirror.paths();
    assert!(
        paths
            .iter()
            .any(|path| path.starts_with("/api.github.com/"))
    );
    assert!(
        paths
            .iter()
            .any(|path| path.starts_with("/downloads.claude.ai/"))
    );
    let after = world
        .app
        .call(AdminHarnessChannelsGet, &[])
        .token(&people.root.token)
        .send()
        .await
        .ok();
    assert_eq!(after.reason, HarnessChannelSchedule::Disabled);
    assert_eq!(after.last_read_at, Some(Time::new(world.now())));
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert_eq!(mirror.paths(), paths);
    world
        .app
        .call(AdminHarnessChannelsGet, &[])
        .token(&people.boss.token)
        .send()
        .await
        .problem();
}

#[tokio::test]
async fn the_interval_has_no_artificial_upper_bound_and_an_exact_pin_needs_no_release_channel() {
    let key = "agents.harness_channel_hours";
    assert!(orch_settings_contract::registry::validate(key, &json!(0)).is_ok());
    assert!(orch_settings_contract::registry::validate(key, &json!(i64::MAX)).is_ok());
    assert!(orch_settings_contract::registry::validate(key, &json!(-1)).is_err());
    let world = World::new().await;
    let mirror = mirror_for(&world).await;
    setting(
        &world,
        None,
        "agents.harness_versions",
        json!({"codex":{"use":"newest"}}),
        false,
    )
    .await;
    setting(&world, None, key, json!(i64::MAX), false).await;
    orch_api_core::harness_channels::record_read(
        &mut *world.conn().await,
        world.now() - chrono::Duration::hours(1000),
        true,
    )
    .await
    .unwrap();
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert!(mirror.paths().is_empty());
    setting(
        &world,
        None,
        "agents.harness_versions",
        json!({"codex":{"use":"exact","version":"0.170.0"}}),
        false,
    )
    .await;
    let state = orch_api_core::harness_channels::status(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(state.reason, HarnessChannelSchedule::Pinned);
    assert!(
        !orch_api_core::harness_channels::scheduled_look(&world.app.app, &reqwest::Client::new())
            .await
            .unwrap()
    );
    assert!(mirror.paths().is_empty());
}
