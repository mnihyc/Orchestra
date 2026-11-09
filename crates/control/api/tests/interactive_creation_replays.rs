//! Native reservations and completed reads keep one actual browser/display process per key.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native controls fail by panicking"
)]
mod places;
mod rig;
mod support;
mod thread_history_scope;
use orch_api_contract::{
    browsers::{BrowserPreset, EnvironmentBrowserCreate, EnvironmentBrowserCreateBody},
    displays::{EnvironmentDisplayCreate, EnvironmentDisplayCreateBody},
};
use orch_api_core::testing::RawReply;
use orch_link_contract::{
    browser::BrowserOpenResult,
    browser_window::DisplayOpenResult,
    commands::{Command, CommandError, CommandKind, CommandResult, ErrorCode},
};
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use thread_history_scope::{Proof, key, rule};

async fn scene() -> Proof<(rig::Scene, Arc<AtomicUsize>, Arc<AtomicUsize>)> {
    static LOGS: std::sync::Once = std::sync::Once::new();
    LOGS.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("error,orch_api::native_operations=debug")
            .with_test_writer()
            .try_init();
    });
    let s = rig::Scene::new().await;
    sqlx::query("UPDATE environments SET browser=true,vnc=true,status='running',placement='managed',facts=jsonb_set(jsonb_set(facts,'{browser}',$2),'{vnc}',$3) WHERE id=$1")
        .bind(s.on.environment)
        .bind(json!({"programs":[{"name":"Chromium","path":"/usr/bin/chromium","version":null}],"window":true,"playwright_cli":null,"sandbox":true}))
        .bind(json!({"servers":[{"name":"Xvnc","path":"/usr/bin/Xvnc","version":null}],"window_manager":null}))
        .execute(s.rig.db.pool()).await?;
    let browsers = Arc::new(AtomicUsize::new(0));
    let count = browsers.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::BrowserOpen, move |command| {
            let Command::BrowserOpen(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "expected native browser open",
                ));
            };
            count.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::BrowserOpen(BrowserOpenResult {
                service: format!("browser:{}", body.browser_id),
                endpoint: "http://127.0.0.1:9291".into(),
                program: "First native program".into(),
                started: true,
                running: None,
            }))
        });
    let displays = Arc::new(AtomicUsize::new(0));
    let count = displays.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::DisplayOpen, move |command| {
            let Command::DisplayOpen(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "expected native display open",
                ));
            };
            count.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::DisplayOpen(DisplayOpenResult {
                service: format!("rfb:{}", body.display_id),
                endpoint: "127.0.0.1:5901".into(),
                x_display: ":71".into(),
            }))
        });
    Ok((s, browsers, displays))
}
#[track_caller]
fn body(reply: &RawReply, status: u16) -> Value {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn scoped_creation_without_response_read(display: bool) -> Proof {
    let (s, browsers, displays) = scene().await?;
    let environment = s.on.environment;
    let operation = if display {
        "EnvironmentDisplayCreate"
    } else {
        "EnvironmentBrowserCreate"
    };
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        async move {
            let act = new_id().to_string();
            if display {
                app.call(EnvironmentDisplayCreate, &[&environment])
                    .token(&secret)
                    .header("Idempotency-Key", &act)
                    .body(&EnvironmentDisplayCreateBody {
                        name: Some("Native scoped display".into()),
                        geometry: None,
                    })
                    .send_bytes()
                    .await
            } else {
                app.call(EnvironmentBrowserCreate, &[&environment])
                    .token(&secret)
                    .header("Idempotency-Key", &act)
                    .body(&EnvironmentBrowserCreateBody {
                        profile: "native-scoped-profile".into(),
                        create: Some(false),
                        preset: Some(BrowserPreset::Headless),
                        url: None,
                    })
                    .send_bytes()
                    .await
            }
        }
    };
    for (target, action) in [(environment, "read"), (new_id(), "manage")] {
        let refused = key(
            &s.rig.world,
            s.ada.id(),
            vec![rule(operation, "environment", target, "self", action)],
        )
        .await?;
        let reply = send(refused.secret).await;
        assert_eq!(body(&reply, 403)["code"], "authz.denied");
        assert_eq!(browsers.load(Ordering::SeqCst), 0);
        assert_eq!(displays.load(Ordering::SeqCst), 0);
    }
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule(
            operation,
            "environment",
            environment,
            "self",
            "manage",
        )],
    )
    .await?;
    let reply = body(&send(writer.secret).await, 201);
    let id: Id =
        serde_json::from_value(reply[if display { "display_id" } else { "browser_id" }].clone())?;
    let expected = if display {
        json!({"projection":"receipt","display_id":id,"environment_id":environment})
    } else {
        json!({"projection":"receipt","browser_id":id,"environment_id":environment})
    };
    assert_eq!(reply, expected);
    assert_eq!(browsers.load(Ordering::SeqCst), usize::from(!display));
    assert_eq!(displays.load(Ordering::SeqCst), usize::from(display));
    let rows: i64 = sqlx::query_scalar(if display {
        "SELECT count(*) FROM display_sessions WHERE id=$1 AND environment_id=$2"
    } else {
        "SELECT count(*) FROM browser_sessions WHERE id=$1 AND environment_id=$2"
    })
    .bind(id)
    .bind(environment)
    .fetch_one(s.rig.db.pool())
    .await?;
    assert_eq!(rows, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_creation_needs_its_environment_manage_and_keeps_the_no_read_receipt() -> Proof {
    scoped_creation_without_response_read(false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn display_creation_needs_its_environment_manage_and_keeps_the_no_read_receipt() -> Proof {
    scoped_creation_without_response_read(true).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_creation_replays_current_native_content_or_a_receipt_without_another_open()
-> Proof {
    let (s, count, _) = scene().await?;
    let environment = s.on.environment;
    let op = "EnvironmentBrowserCreate";
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(op, "environment", environment, "self", "manage"),
            rule(op, "environment", environment, "self", "read"),
        ],
    )
    .await?;
    let header = new_id().to_string();
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let header = header.clone();
        async move {
            let call = app
                .call(EnvironmentBrowserCreate, &[&environment])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .body(&EnvironmentBrowserCreateBody {
                    profile: "one-native-profile".into(),
                    create: Some(false),
                    preset: Some(BrowserPreset::Headless),
                    url: None,
                });
            call.send_bytes().await
        }
    };
    let first = body(&send(writer.secret).await, 201);
    let id: Id = serde_json::from_value(first["id"].clone())?;
    sqlx::query("UPDATE browser_sessions SET program='Current native program' WHERE id=$1")
        .bind(id)
        .execute(s.rig.db.pool())
        .await?;
    for selected in [op, "BrowserClose"] {
        let reader = key(
            &s.rig.world,
            s.ada.id(),
            vec![rule(selected, "environment", environment, "self", "read")],
        )
        .await?;
        let again = body(&send(reader.secret).await, 201);
        if selected == op {
            assert_eq!(again["id"], json!(id));
            assert_eq!(again["program"], "Current native program");
        } else {
            assert_eq!(
                again,
                json!({"projection":"receipt","browser_id":id,"environment_id":environment})
            );
        }
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM browser_sessions WHERE environment_id=$1")
            .bind(environment)
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(rows, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn joining_an_existing_browser_records_its_native_200_status_and_real_identity() -> Proof {
    let (s, count, _) = scene().await?;
    let environment = s.on.environment;
    let input = EnvironmentBrowserCreateBody {
        profile: "one-joined-profile".into(),
        create: Some(false),
        preset: Some(BrowserPreset::Headless),
        url: None,
    };
    let call = s
        .rig
        .world
        .app
        .call(EnvironmentBrowserCreate, &[&environment])
        .token(&s.ada.token)
        .body(&input);
    let first = body(&call.send_bytes().await, 201);
    let id: Id = serde_json::from_value(first["id"].clone())?;
    let header = new_id().to_string();
    for _ in 0..2 {
        let call = s
            .rig
            .world
            .app
            .call(EnvironmentBrowserCreate, &[&environment])
            .token(&s.ada.token)
            .header("Idempotency-Key", &header)
            .body(&input);
        assert_eq!(body(&call.send_bytes().await, 200)["id"], json!(id));
    }
    let stored: (i32, Value) =
        sqlx::query_as("SELECT status,response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(s.ada.id())
            .bind(header)
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(stored.0, 200);
    assert_eq!(stored.1["outcome"]["id"], json!(id));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_display_creation_replays_current_native_content_or_a_receipt_without_another_open()
-> Proof {
    let (s, _, count) = scene().await?;
    let environment = s.on.environment;
    let op = "EnvironmentDisplayCreate";
    let writer = key(
        &s.rig.world,
        s.ada.id(),
        vec![
            rule(op, "environment", environment, "self", "manage"),
            rule(op, "environment", environment, "self", "read"),
        ],
    )
    .await?;
    let header = new_id().to_string();
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let header = header.clone();
        async move {
            let call = app
                .call(EnvironmentDisplayCreate, &[&environment])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .body(&EnvironmentDisplayCreateBody {
                    name: Some("One native screen".into()),
                    geometry: Some("800x600".into()),
                });
            call.send_bytes().await
        }
    };
    let first = body(&send(writer.secret).await, 201);
    let id: Id = serde_json::from_value(first["id"].clone())?;
    sqlx::query("UPDATE display_sessions SET name='Current native screen' WHERE id=$1")
        .bind(id)
        .execute(s.rig.db.pool())
        .await?;
    for selected in [op, "DisplayClose"] {
        let reader = key(
            &s.rig.world,
            s.ada.id(),
            vec![rule(selected, "environment", environment, "self", "read")],
        )
        .await?;
        let again = body(&send(reader.secret).await, 201);
        if selected == op {
            assert_eq!(again["id"], json!(id));
            assert_eq!(again["name"], "Current native screen");
        } else {
            assert_eq!(
                again,
                json!({"projection":"receipt","display_id":id,"environment_id":environment})
            );
        }
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM display_sessions WHERE environment_id=$1")
            .bind(environment)
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(rows, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_creation_headers_keep_one_native_command_and_read_the_current_profile_with_a_new_key()
-> Proof {
    use orch_api_contract::browsers::{BrowserProfileCreate, BrowserProfileCreateBody};
    use orch_link_contract::browser::{
        BrowserProfile, BrowserProfileAction, BrowserProfileResult, ScreenLookResult,
    };
    let (s, _, _) = scene().await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::BrowserProfile, move |command| {
            let Command::BrowserProfile(command) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "expected native profile create",
                ));
            };
            assert_eq!(command.action, BrowserProfileAction::Create);
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(CommandResult::BrowserProfile(BrowserProfileResult {
                profiles: vec![BrowserProfile {
                    name: command.name.clone(),
                    size_bytes: Some(1),
                    used_at: None,
                    browser: None,
                }],
            }))
        });
    let act = new_id();
    let input = BrowserProfileCreateBody {
        name: "retry-profile".into(),
    };
    let first_request = s
        .rig
        .world
        .app
        .call(BrowserProfileCreate, &[&s.on.environment])
        .token(&s.ada.token)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let first = first_request.send_bytes().await;
    let first_body = body(&first, 201);
    assert_eq!(
        first_body,
        json!({"name":"retry-profile","size_bytes":1,"used_at":null,"browser":null})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let saved: Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(s.ada.id())
            .bind(act.to_string())
            .fetch_one(s.rig.db.pool())
            .await?;
    let command: Id = serde_json::from_value(saved["outcome"]["id"].clone())?;
    assert_eq!(
        saved["outcome"]["data"]["native_command"]["id"],
        json!(command)
    );
    assert!(saved["outcome"]["data"].get("name").is_none());
    assert!(saved["outcome"]["data"].get("size_bytes").is_none());
    s.rig.machine.handler.on(CommandKind::ScreenLook, |_| {
        Ok(CommandResult::ScreenLook(ScreenLookResult {
            browser: serde_json::from_value(json!({"programs":[{"name":"Chromium","path":"/usr/bin/chromium","version":null}],"window":true,"playwright_cli":null,"sandbox":true})).unwrap(),
            vnc: Default::default(),
            profiles: vec![BrowserProfile { name: "retry-profile".into(), size_bytes: Some(9001), used_at: None, browser: None }],
            browsers: Vec::new(), displays: Vec::new(),
        }))
    });
    s.rig
        .world
        .app
        .app
        .engine
        .browsers()
        .look(s.on.environment)
        .await?;
    let reader = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule(
            "BrowserProfileCreate",
            "environment",
            s.on.environment,
            "self",
            "read",
        )],
    )
    .await?;
    let request = s
        .rig
        .world
        .app
        .call(BrowserProfileCreate, &[&s.on.environment])
        .token(&reader.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let current = request.send_bytes().await;
    let current = body(&current, 201);
    assert_eq!(current["name"], "retry-profile");
    assert_eq!(current["size_bytes"], 9001);
    let unrelated = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule(
            "EnvironmentBrowserList",
            "environment",
            s.on.environment,
            "self",
            "read",
        )],
    )
    .await?;
    let request = s
        .rig
        .world
        .app
        .call(BrowserProfileCreate, &[&s.on.environment])
        .token(&unrelated.secret)
        .header("Idempotency-Key", &act.to_string())
        .body(&input);
    let receipt = request.send_bytes().await;
    assert_eq!(
        body(&receipt, 201),
        json!({"projection":"receipt","command_id":command,"environment_id":s.on.environment})
    );
    let request = s
        .rig
        .world
        .app
        .call(BrowserProfileCreate, &[&s.on.environment])
        .token(&s.ada.token)
        .header("Idempotency-Key", &act.to_string())
        .body(&BrowserProfileCreateBody {
            name: "other-profile".into(),
        });
    assert_eq!(request.send_bytes().await.status.as_u16(), 409);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let commands: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind='browser.profile' AND body->>'environment_id'=$1",
    )
    .bind(s.on.environment.to_string())
    .fetch_one(s.rig.db.pool())
    .await?;
    assert_eq!(commands, 1);
    let unchanged: Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(s.ada.id())
            .bind(act.to_string())
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(unchanged, saved);
    Ok(())
}
