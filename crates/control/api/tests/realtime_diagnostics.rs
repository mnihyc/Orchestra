//! Resource changes, private provider progress and freshly masked diagnostic frames on transient feeds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_core::realtime::raw_traffic;
use orch_db::queries::access;
use orch_link::streams::{Received, StreamReceiver};
use orch_link_contract::events::{Event, RawFrame};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{Ev, Evb, EvbEntry, Stream};
use orch_realtime_contract::feeds::{Feed, ResourceKind};
use orch_types::new_id;
use realtime_support::{Client, Heard, Rig};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

async fn next_what(client: &mut Client, what: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == "resync" {
            // An authority change clears the former projection before the next event.
            continue;
        }
        assert_eq!(
            frame["op"],
            "event",
            "unexpected event frame: {}",
            frame_summary(&frame, None)
        );
        if frame["data"]["what"] == what {
            return frame["data"].clone();
        }
    }
}

// Only structural diagnostics: no receipt, token, certificate or native payload is printed.
fn frame_summary(frame: &Value, receipt: Option<&str>) -> Value {
    json!({
        "op": frame["op"].as_str(), "feed": frame["feed"].as_str(),
        "event_kind": frame["data"]["what"].as_str(),
        "receipt_matches": receipt.is_some_and(|receipt| frame["data"]["receipt"].as_str() == Some(receipt)),
        "status_present": frame["data"].get("status").is_some_and(|status| !status.is_null()),
    })
}

async fn enrollment_event(
    client: &mut Client,
    receipt: &str,
) -> orch_api_contract::resources::EnrollmentProgress {
    let mut seen = std::collections::VecDeque::new();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = client.frame().await;
            // The last sixteen structural frames bound a timeout diagnostic, not delivery.
            if seen.len() == 16 {
                let _ = seen.pop_front();
            }
            seen.push_back(frame_summary(&frame, Some(receipt)));
            if frame["op"] == "event"
                && frame["data"]["what"] == "enrollment"
                && frame["data"]["receipt"] == receipt
            {
                return serde_json::from_value(frame["data"]["status"].clone())
                    .expect("the matching enrollment must carry its native projected status");
            }
            assert!(
                matches!(frame["op"].as_str(), Some("sync" | "event" | "resync")),
                "unexpected issuer frame: {seen:?}"
            );
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("the committed enrollment reaches its issuer within ten seconds; structural frames: {seen:?}"))
}

async fn no_enrollment_event(client: &mut Client, wait: Duration) {
    let until = tokio::time::Instant::now() + wait;
    while let Some(remaining) = until.checked_duration_since(tokio::time::Instant::now()) {
        let Some(heard) = client.hear_within(remaining).await else {
            break;
        };
        let Heard::Frame(frame) = heard else {
            panic!("the peer's socket closed");
        };
        assert_ne!(
            frame["data"]["what"], "enrollment",
            "another person's user feed received an enrollment"
        );
    }
}

async fn enrollment_case(node: bool) {
    use orch_api_contract::resources::{EnrollmentProgress, EnrollmentToken};
    use orch_link_contract::enrollment::EnrollRequest;
    use orch_link_contract::session::{Arch, Os, Platform};
    let rig = Rig::with_time_held().await;
    let (_, peer_token) = rig.person("enrollment-peer").await;
    let mut issuer = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut peer = Client::signed_in(rig.server.addr, &peer_token).await;
    issuer.sub(1, "user", None).await;
    issuer.expect("subscribed").await;
    peer.sub(1, "user", None).await;
    peer.expect("subscribed").await;
    let http = reqwest::Client::new();
    let base = format!("http://{}/api/v1", rig.server.addr);
    let (path, body) = if node {
        ("hosts", json!({"home_id":rig.root,"access":"only_me"}))
    } else {
        ("machines", json!({"home_id":rig.root,"roles":["agent"]}))
    };
    let response = http
        .post(format!("{base}/{path}/enrollment-tokens"))
        .bearer_auth(&rig.token)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let issued: EnrollmentToken = response.json().await.unwrap();
    let receipt = issued
        .receipt
        .as_deref()
        .expect("new enrollment has a tracking receipt");
    let status_url = format!("{base}/machines/enrollment-status/{receipt}");
    let pending = http
        .get(&status_url)
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap();
    assert_eq!(pending.status(), reqwest::StatusCode::OK);
    assert_eq!(
        pending
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let pending: EnrollmentProgress = pending.json().await.unwrap();
    assert!(pending.machine.is_none() && pending.host.is_none());
    assert_eq!(
        http.get(&status_url)
            .bearer_auth(&peer_token)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::NOT_FOUND
    );
    assert_eq!(
        http.get(&status_url).send().await.unwrap().status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let certificate = orch_pki::enrollment::PendingEnrollment::start().unwrap();
    let enrolled = rig
        .server
        .app
        .link
        .certificates()
        .enroll(
            &EnrollRequest {
                machine_id: None,
                token: issued.token,
                csr_pem: certificate.csr_pem().to_owned(),
                name: "receipt-machine".to_owned(),
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                connector_version: "1.0.0".to_owned(),
                protocol_versions: vec![1],
                roles: vec![if node { Role::Node } else { Role::Agent }],
            },
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap();
    let progress = enrollment_event(&mut issuer, receipt).await;
    assert!(
        progress.receipt == receipt,
        "the projected enrollment must match the issuer's exact receipt"
    );
    let resource = if node {
        let host = progress
            .host
            .as_ref()
            .expect("node enrollment returns its host view");
        assert!(progress.machine.is_none());
        assert_ne!(host.base.id, enrolled.machine_id);
        assert_eq!(host.machine.id, enrolled.machine_id);
        assert!(!host.machine.online);
        assert!(
            host.access
                .permissions
                .iter()
                .any(|permission| permission == "host.view")
        );
        Feed::Resource {
            kind: ResourceKind::Host,
            id: host.base.id,
        }
    } else {
        let machine = progress
            .machine
            .as_ref()
            .expect("connector enrollment returns its machine view");
        assert!(progress.host.is_none());
        assert_eq!(machine.base.id, enrolled.machine_id);
        assert!(!machine.base.online);
        Feed::Resource {
            kind: ResourceKind::Machine,
            id: enrolled.machine_id,
        }
    };
    let recovery: EnrollmentProgress = http
        .get(&status_url)
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        recovery == progress,
        "recovery reads exactly the same admitted native projection"
    );
    no_enrollment_event(&mut peer, Duration::from_millis(300)).await;
    let mut stream = Client::signed_in(rig.server.addr, &rig.token).await;
    stream.sub(2, &resource.to_string(), None).await;
    stream.expect("subscribed").await;
    next_what(&mut stream, "changed").await;
    sqlx::query("UPDATE machines SET online=true,last_seen_at=$2 WHERE id=$1")
        .bind(enrolled.machine_id)
        .bind(rig.clock.now())
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let online = next_what(&mut stream, "changed").await;
    if node {
        assert_eq!(online["row"]["value"]["machine"]["online"], true);
    } else {
        assert_eq!(online["row"]["value"]["online"], true);
    }
    let mut foreign = Client::signed_in(rig.server.addr, &peer_token).await;
    foreign.sub(3, &resource.to_string(), None).await;
    let denied = foreign.expect("error").await;
    assert!(
        matches!(denied["code"].as_str(), Some("forbidden" | "not_found")),
        "the native feed must refuse an unreadable resource"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connector_enrollment_is_correlated_and_recovered_only_by_its_issuer_then_follows_online()
{
    enrollment_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_enrollment_delivers_its_host_identity_and_online_transition_only_to_its_issuer() {
    enrollment_case(true).await;
}

/// A native public view can change during sign-in; no private Login or Job event may reach this reader.
async fn only_public_provider_rows(client: &mut Client, wait: Duration, manages: bool) {
    let until = tokio::time::Instant::now() + wait;
    while let Some(remaining) = until.checked_duration_since(tokio::time::Instant::now()) {
        let Some(heard) = client.hear_within(remaining).await else {
            break;
        };
        let Heard::Frame(frame) = heard else {
            panic!("the provider reader's socket closed");
        };
        assert_eq!(frame["op"], "event", "{frame}");
        let data: orch_realtime_contract::feeds::ResourceEvent =
            serde_json::from_value(frame["data"].clone()).unwrap();
        let orch_realtime_contract::feeds::ResourceEvent::Changed {
            row: Some(orch_api_contract::everything::EverythingResource::Provider(row)),
            view_revoked: false,
        } = data
        else {
            panic!("a private provider step reached an ordinary resource reader: {frame}");
        };
        assert_eq!(
            row.access
                .permissions
                .iter()
                .any(|permission| permission == "provider.manage"),
            manages
        );
    }
}

async fn the_provider_manager_is_allowed(rig: &Rig) {
    let mut conn = rig.pool.acquire().await.unwrap();
    let page = access::PageKey::new("provider", rig.provider, "live");
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id: new_id(),
            page: &page,
            profile: "any",
            permission: "provider.manage",
            holder_kind: "user",
            holder_id: Some(rig.ada),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(rig.ada),
            at: rig.clock.now(),
        },
    )
    .await
    .unwrap();
    access::bump_epoch(&mut conn).await.unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(rig.ada),
        rig.clock.time(),
    )
    .await
    .unwrap();
    let object = orch_access_contract::decisions::ObjectRef {
        kind: orch_access_contract::permissions::ObjectKind::Provider,
        id: rig.provider,
    };
    let decision = orch_authz::resources::check(
        &mut conn,
        &principal,
        object,
        orch_access_contract::permissions::Permission::ProviderManage,
        orch_authz::Policy::default(),
    )
    .await;
    assert!(
        matches!(
            decision,
            Ok(orch_access_contract::decisions::Decision::Allowed { .. })
        ),
        "manager decision: {decision:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_error_reaches_the_provider_page_when_only_its_harness_report_changed() {
    let rig = Rig::with_time_held().await;
    the_provider_manager_is_allowed(&rig).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let feed = Feed::Resource {
        kind: ResourceKind::Provider,
        id: rig.provider,
    };
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    let initial = next_what(&mut client, "changed").await;
    assert!(
        initial["row"]["value"]["harnesses"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    realtime_support::coordinator_ready(&rig).await;
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::resources::thread_engine::set_harness_status(
        &mut tx,
        rig.provider,
        rig.agent,
        rig.machine_id,
        &orch_db::queries::resources::thread_engine::HarnessReport {
            status: "online",
            detail: None,
            version: Some("1.0.0"),
            sessions_open: 0,
        },
        rig.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE harnesses SET last_error_at=$4,last_error_detail=$5 WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(rig.provider).bind(rig.agent).bind(rig.machine_id).bind(rig.clock.now()).bind("native process final diagnostic")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let changed = next_what(&mut client, "changed").await;
    let reports = changed["row"]["value"]["harnesses"].as_array().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0]["status"], "online");
    assert_eq!(
        reports[0]["last_error"]["detail"],
        "native process final diagnostic"
    );
    assert_eq!(
        changed["row"]["value"]["access"],
        initial["row"]["value"]["access"]
    );
    assert!(
        client.hear_within(Duration::from_secs(2)).await.is_none(),
        "the unchanged program report is quiet"
    );
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_providers_native_view_and_models_arrive_without_reload_and_an_unchanged_epoch_is_quiet()
{
    let rig = Rig::with_time_held().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(
            1,
            &Feed::Resource {
                kind: ResourceKind::Provider,
                id: rig.provider,
            }
            .to_string(),
            None,
        )
        .await;
    client.expect("subscribed").await;
    let initial = next_what(&mut client, "changed").await;
    assert_eq!(initial["row"]["kind"], "provider");
    assert_eq!(initial["row"]["value"]["id"], rig.provider.to_string());
    assert!(initial["row"]["value"]["access"]["permissions"].is_array());
    realtime_support::coordinator_ready(&rig).await;
    sqlx::query("UPDATE providers SET name = 'Changed' WHERE id = $1")
        .bind(rig.provider)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let renamed = next_what(&mut client, "changed").await;
    // Only which members of the view differ from the first one, not what they hold.
    let differing: Vec<&String> = renamed["row"]["value"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, value)| initial["row"]["value"].get(key.as_str()) != Some(value))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        renamed["row"]["value"]["name"], "Changed",
        "the next view differs from the first in {differing:?}"
    );
    assert_eq!(
        renamed["row"]["value"]["access"],
        initial["row"]["value"]["access"]
    );
    sqlx::query("UPDATE provider_models SET enabled = false WHERE provider_id = $1")
        .bind(rig.provider)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let models = next_what(&mut client, "changed").await;
    assert_ne!(
        models["row"]["value"]["models"],
        renamed["row"]["value"]["models"]
    );
    let decoded: orch_realtime_contract::feeds::ResourceEvent =
        serde_json::from_value(models).unwrap();
    assert!(matches!(
        decoded,
        orch_realtime_contract::feeds::ResourceEvent::Changed {
            row: Some(_),
            view_revoked: false
        }
    ));
    let mut conn = rig.pool.acquire().await.unwrap();
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);
    // The delivered API view, rather than an epoch bump alone, is what changed for this reader.
    let unexpected = client.hear_within(Duration::from_secs(2)).await;
    let diagnostic = unexpected.as_ref().map(|heard| match heard {
        Heard::Frame(frame) => frame_summary(frame, None),
        Heard::Closed(code) => json!({"closed":code}),
    });
    assert!(
        unexpected.is_none(),
        "an unchanged native provider view must be completely quiet; first={diagnostic:?}"
    );
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_sign_in_step_reaches_managers_and_no_step_reaches_a_viewer() {
    let rig = Rig::new().await;
    the_provider_manager_is_allowed(&rig).await;
    let (viewer, token) = rig.person("provider-reader").await;
    let (keeper, keeper_token) = rig.person("provider-keeper").await;
    let page = access::PageKey::new("provider", rig.provider, "live");
    let mut conn = rig.pool.acquire().await.unwrap();
    for (holder, permission) in [(viewer, "provider.view"), (keeper, "provider.manage")] {
        access::insert_entry(
            &mut conn,
            &access::NewEntry {
                id: new_id(),
                page: &page,
                profile: "any",
                permission,
                holder_kind: "user",
                holder_id: Some(holder),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "",
                source: "page",
                created_by: Some(rig.ada),
                at: rig.clock.now(),
            },
        )
        .await
        .unwrap();
    }
    access::bump_epoch(&mut conn).await.unwrap();
    drop(conn);
    let feed = Feed::Resource {
        kind: ResourceKind::Provider,
        id: rig.provider,
    };
    let mut manager = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut reader = Client::signed_in(rig.server.addr, &token).await;
    for client in [&mut manager, &mut reader] {
        client.sub(1, &feed.to_string(), None).await;
        client.expect("subscribed").await;
        let current = next_what(client, "changed").await;
        assert_eq!(current["row"]["kind"], "provider");
        assert_eq!(current["row"]["value"]["id"], rig.provider.to_string());
    }
    // Any admitted right keeps its own feed: a keeper's private progress must not require a rich View grant.
    let mut keeper = Client::signed_in(rig.server.addr, &keeper_token).await;
    keeper.sub(1, &feed.to_string(), None).await;
    keeper.expect("subscribed").await;
    let current = next_what(&mut keeper, "changed").await;
    let native = reqwest::Client::new()
        .get(format!(
            "http://{}/api/v1/providers/{}",
            rig.server.addr, rig.provider
        ))
        .bearer_auth(&keeper_token)
        .send()
        .await
        .unwrap();
    if native.status().is_success() {
        let expected: Value = native.json().await.unwrap();
        assert_eq!(current["row"]["value"]["access"], expected["access"]);
        assert_eq!(current["row"]["value"]["id"], expected["id"]);
    } else {
        assert_eq!(native.status(), reqwest::StatusCode::FORBIDDEN);
        assert_eq!(current["view_revoked"], true);
        assert!(current.get("row").is_none());
    }
    realtime_support::coordinator_ready(&rig).await;
    let stream = Stream::Provider {
        provider_id: rig.provider,
    };
    let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Provider]);
    let steps = ["starting", "awaiting_user", "completed"];
    let events = steps.iter().enumerate().map(|(n, state)| EvbEntry {
        seq: i64::try_from(n).unwrap() + 1, at: rig.clock.time(), kind: "provider.login".to_owned(),
        body: json!({"state": state, "method": "login", "shows": [], "asks": null, "expires_at": null, "error": null}),
    }).collect();
    let Received::Apply(delivery) = receiver.on_evb(Evb {
        stream: stream.to_string(),
        events,
    }) else {
        panic!("a login delivery");
    };
    raw_traffic::after(&rig.pool, None, &delivery)
        .await
        .unwrap();
    for step in steps {
        assert_eq!(
            next_what(&mut manager, "login").await["state"]["state"],
            step
        );
        assert_eq!(
            next_what(&mut keeper, "login").await["state"]["state"],
            step
        );
    }
    only_public_provider_rows(&mut reader, Duration::from_millis(500), false).await;
    let mut conn = rig.pool.acquire().await.unwrap();
    orch_db::queries::resources::provider_runtime::set_login_state(
        &mut conn,
        rig.provider,
        &json!({"state": "completed", "method": "login", "shows": [], "asks": null,
            "expires_at": null, "error": null, "at": rig.clock.time()}),
        rig.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    // Reception time is not a new native sign-in state or resource view.
    only_public_provider_rows(&mut manager, Duration::from_secs(2), true).await;
    drop((manager, reader, keeper));
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_job_that_makes_a_providers_runtime_reports_its_progress() {
    let rig = Rig::new().await;
    the_provider_manager_is_allowed(&rig).await;
    rig.server.engine.stop();
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(
            1,
            &Feed::Resource {
                kind: ResourceKind::Provider,
                id: rig.provider,
            }
            .to_string(),
            None,
        )
        .await;
    client.expect("subscribed").await;
    let id = new_id();
    let progress = json!({"phase": "make", "phases": [{"name": "make", "weight": 100, "state": "running"}],
        "percent": 10, "files_done": 0, "files_total": null, "bytes_done": 0, "bytes_total": null,
        "rate_bps": null, "eta_seconds": null, "message": "Making the runtime"});
    sqlx::query("INSERT INTO jobs (id, kind, provider_id, state, plan, progress, created_by) VALUES ($1, 'provider_provision', $2, 'running', '{}', $3, $4)")
        .bind(id).bind(rig.provider).bind(&progress).bind(rig.ada).execute(rig.pool.pool()).await.unwrap();
    let data = next_what(&mut client, "job").await;
    assert_eq!(data["job"]["id"], id.to_string());
    assert_eq!(data["job"]["progress"]["percent"], 10);
    sqlx::query("UPDATE jobs SET progress = jsonb_set(progress, '{percent}', '80') WHERE id = $1")
        .bind(id)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        next_what(&mut client, "job").await["job"]["progress"]["percent"],
        80
    );
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_raw_frame_arrives_masked_and_a_later_subscriber_receives_no_old_frames() {
    let rig = Rig::new().await;
    let thread = rig.thread("Diagnostics").await;
    rig.send(thread, "start a session").await;
    let segment = rig.segment_of(thread).await;
    let feed = Feed::ThreadRaw(thread);
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .send(json!({"op":"sub","id":1,"feed":feed.to_string(),
        "raw_segment_id":segment,"projection_revision":1}))
        .await;
    client.expect("subscribed").await;
    let event = Event::RawFrame(RawFrame {
        n: 1,
        direction: "in".to_owned(),
        payload: json!({"Authorization": "must-never-arrive", "message": "hello"}),
    });
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Agent]);
    let Received::Apply(delivery) =
        receiver.on_ev(Ev::of(&stream, 1, rig.clock.time(), &event).unwrap())
    else {
        panic!("a raw delivery");
    };
    let mut tx = rig.pool.begin().await.unwrap();
    let boundary = raw_traffic::before(&mut tx, &delivery).await.unwrap();
    rig.server
        .engine
        .apply_events(&mut tx, rig.machine_id, &delivery)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    raw_traffic::after(&rig.pool, boundary, &delivery)
        .await
        .unwrap();
    let frame = client.expect("event").await;
    assert_eq!(
        frame["data"]["event"]["payload"]["Authorization"],
        "[masked]"
    );
    assert_eq!(frame["data"]["event"]["payload"]["message"], "hello");
    assert!(!frame.to_string().contains("must-never-arrive"));
    let mut newcomer = Client::signed_in(rig.server.addr, &rig.token).await;
    newcomer
        .send(json!({"op":"sub","id":1,"feed":feed.to_string(),
        "raw_segment_id":segment,"projection_revision":1}))
        .await;
    newcomer.expect("subscribed").await;
    assert!(newcomer.is_quiet_for(Duration::from_millis(300)).await);
    drop((client, newcomer));
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_environment_ends_its_transient_subscription_without_an_authorization_announcement()
 {
    async fn next_before(
        client: &mut Client,
        until: tokio::time::Instant,
        phase: &'static str,
    ) -> Value {
        let remaining = until
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|left| !left.is_zero())
            .unwrap_or_else(|| {
                panic!("deleted environment phase={phase}: the original overall patience elapsed")
            });
        match client.hear_within(remaining).await {
            Some(Heard::Frame(frame)) => frame,
            Some(Heard::Closed(code)) => {
                panic!("deleted environment phase={phase}: the sibling socket closed {code:?}")
            }
            None => {
                panic!("deleted environment phase={phase}: the original overall patience elapsed")
            }
        }
    }
    fn sibling_row(
        frame: &Value,
        rig: &Rig,
        feed: &str,
    ) -> Box<orch_api_contract::resources::Provider> {
        assert!(
            frame["id"] == 2 && frame["feed"] == feed && frame["op"] == "event",
            "only a correlated provider event can update the sibling: {}",
            frame_summary(frame, None)
        );
        let event = serde_json::from_value::<orch_realtime_contract::feeds::ResourceEvent>(
            frame["data"].clone(),
        )
        .unwrap_or_else(|_| {
            panic!(
                "the correlated sibling event must have its native resource shape: {}",
                frame_summary(frame, None)
            )
        });
        let orch_realtime_contract::feeds::ResourceEvent::Changed {
            row: Some(orch_api_contract::everything::EverythingResource::Provider(row)),
            view_revoked: false,
        } = event
        else {
            panic!(
                "the sibling retains its current native provider view: {}",
                frame_summary(frame, None)
            );
        };
        assert!(
            row.base.id == rig.provider && row.owner.id == rig.ada,
            "the sibling row must belong to this provider and its actual fixture owner"
        );
        row
    }
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    let sibling_feed = Feed::Resource {
        kind: ResourceKind::Provider,
        id: rig.provider,
    }
    .to_string();
    let environment_feed = Feed::EnvDisplays(rig.environment).to_string();
    // Both subscriptions share one transport, not one event ordering. Every
    // intervening provider row is validated and handled within this same bound.
    let until = tokio::time::Instant::now() + realtime_support::PATIENCE;
    client.sub(2, &sibling_feed, None).await;
    let ack = next_before(&mut client, until, "provider acknowledgement").await;
    assert!(
        ack["op"] == "subscribed" && ack["id"] == 2 && ack["feed"] == sibling_feed,
        "the provider requires its own actual acknowledgement: {}",
        frame_summary(&ack, None)
    );
    let initial = sibling_row(
        &next_before(&mut client, until, "initial provider view").await,
        &rig,
        &sibling_feed,
    );
    let original_name = initial.base.name;
    client.sub(1, &environment_feed, None).await;
    loop {
        let frame = next_before(&mut client, until, "environment acknowledgement").await;
        if frame["id"] == 2 {
            assert!(
                sibling_row(&frame, &rig, &sibling_feed).base.name == original_name,
                "before the explicit rename the correlated provider retains its name"
            );
        } else {
            assert!(
                frame["op"] == "subscribed"
                    && frame["id"] == 1
                    && frame["feed"] == environment_feed,
                "the environment requires its own actual acknowledgement: {}",
                frame_summary(&frame, None)
            );
            break;
        }
    }
    sqlx::query("UPDATE environments SET deleted_at = $2 WHERE id = $1")
        .bind(rig.environment)
        .bind(rig.clock.now())
        .execute(rig.pool.pool())
        .await
        .unwrap();
    loop {
        let frame = next_before(&mut client, until, "environment deleted closure").await;
        if frame["id"] == 2 {
            assert!(
                sibling_row(&frame, &rig, &sibling_feed).base.name == original_name,
                "a correlated sibling update is handled without satisfying the deletion closure"
            );
            continue;
        }
        assert!(
            frame["id"] == 1 && frame["feed"] == environment_feed,
            "only the exact environment reader may end here: {}",
            frame_summary(&frame, None)
        );
        if frame["op"] == "closed" {
            assert_eq!(frame["reason"], "deleted");
            break;
        }
        // The scripted fixture may still be connecting when the subscription is answered.
        assert_eq!(frame["op"], "event");
        assert_eq!(frame["data"]["offline"], true);
    }
    client.send(json!({"op":"ping","n":47})).await;
    loop {
        let frame = next_before(&mut client, until, "correlated Pong after closure").await;
        if frame["id"] == 2 {
            assert!(
                sibling_row(&frame, &rig, &sibling_feed).base.name == original_name,
                "a current provider update cannot stand in for the Pong"
            );
        } else {
            assert!(
                frame["op"] == "pong" && frame["n"] == 47,
                "the same socket must answer the exact ping after deletion: {}",
                frame_summary(&frame, None)
            );
            break;
        }
    }
    sqlx::query("UPDATE providers SET name='Live sibling after environment deletion' WHERE id=$1")
        .bind(rig.provider)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    loop {
        let frame = next_before(&mut client, until, "live provider rename after closure").await;
        let sibling = sibling_row(&frame, &rig, &sibling_feed);
        if sibling.base.name == "Live sibling after environment deletion" {
            break;
        }
        assert!(
            sibling.base.name == original_name,
            "only a correlated earlier native row may precede the explicit provider rename"
        );
    }
    drop(client);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_how_many_raw_events_one_api_process_passes_on_in_a_second() {
    let rig = Rig::new().await;
    let thread = rig.thread("Fanout").await;
    rig.send(thread, "start a session").await;
    let segment = rig.segment_of(thread).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .send(
            json!({"op":"sub","id":1,"feed":Feed::ThreadRaw(thread).to_string(),
        "raw_segment_id":segment,"projection_revision":1}),
        )
        .await;
    client.expect("subscribed").await;
    // One full link window is a bounded burst, representative of frames already gathered by a harness.
    let count = orch_link_contract::streams::MAX_UNACKNOWLEDGED_EVENTS;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let events = (1..=count)
        .map(|seq| EvbEntry {
            seq,
            at: rig.clock.time(),
            kind: "raw.frame".to_owned(),
            body: json!({"n": seq, "direction": "in", "payload": {"text": "a diagnostic frame"}}),
        })
        .collect();
    let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Agent]);
    let Received::Apply(delivery) = receiver.on_evb(Evb {
        stream: stream.to_string(),
        events,
    }) else {
        panic!("a burst delivery");
    };
    let mut tx = rig.pool.begin().await.unwrap();
    let boundary = raw_traffic::before(&mut tx, &delivery).await.unwrap();
    rig.server
        .engine
        .apply_events(&mut tx, rig.machine_id, &delivery)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let began = std::time::Instant::now();
    raw_traffic::after(&rig.pool, boundary, &delivery)
        .await
        .unwrap();
    for n in 1..=count {
        assert_eq!(client.expect("event").await["data"]["event"]["n"], n);
    }
    println!(
        "events={} elapsed_ms={} api_events_per_second={:.2}",
        count,
        began.elapsed().as_millis(),
        count as f64 / began.elapsed().as_secs_f64()
    );
    drop(client);
    rig.server.stop().await;
}
