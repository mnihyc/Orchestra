//! Creation retries retain native identities and project only the current caller's content.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod desk;
#[path = "creation_replays/permission_rules.rs"]
mod permission_rules;
mod places;
mod rig;
mod support;
mod thread_history_scope;

use desk::{Desk, body};
use orch_api_contract::{
    auth::AuthRefresh,
    binding::EnvironmentChoice,
    common::ErrorCode,
    documents::{DocumentUpdate, NodeDocumentCreate, NodeDocumentCreateBody},
    nodes::{
        CreateThread, CreateThreadBinding, OrgChildCreate, OrgCreate, ProjectChildCreate,
        ProjectCreate, ThreadCreate, TopicCreate, TopicCreateBody,
    },
    routes::Endpoint,
};
use orch_api_core::testing::RawReply;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use thread_history_scope::{Proof, key, rule};

fn show_internal_failures() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .without_time()
            .try_init();
    });
}

#[tokio::test]
async fn a_system_profile_creation_replays_current_native_singleton_content_or_its_receipt() -> Proof
{
    show_internal_failures();
    use orch_api_contract::{
        admin::AdminAgentCreate,
        auth::{AdminModeSetBody, MeAdminModeSet},
    };
    let w = Desk::new().await;
    let admin = places::admin(&w.world, "profile-creator").await;
    let header = new_id().to_string();
    let input = json!({"name":"Shared once","harness_kind":"codex","profile":{"env":{"NOTE":"native profile sentinel"}}});
    let send = |secret: String| {
        let app = w.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(AdminAgentCreate, &[])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(admin.token.clone()).await, 200);
    let profile = id(&first);
    sqlx::query("UPDATE agents SET name='Current shared profile',revision=revision+1 WHERE id=$1")
        .bind(profile)
        .execute(&w.world.db.pool)
        .await?;
    for op in ["AdminAgentCreate", "AgentGet"] {
        let actor = key(
            &w.world,
            admin.id(),
            vec![
                json!({"target":{"kind":"system","reach":"self"},"resources":["agent"],
            "allow":[{"operation":op,"actions":["read"]}]}),
            ],
        )
        .await?;
        let actual = value(&send(actor.secret).await, 200);
        if op == "AdminAgentCreate" {
            assert_eq!(id(&actual), profile);
            assert_eq!(actual["name"], "Current shared profile");
            assert_eq!(actual["profile"]["env"]["NOTE"], "native profile sentinel");
            assert!(actual["owner"].is_null() && actual["home"].is_null());
        } else {
            assert_eq!(actual, json!({"projection":"receipt","agent_id":profile}));
        }
    }
    w.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&admin.token)
        .body(&AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    assert_eq!(
        value(&send(admin.token.clone()).await, 200),
        json!({"projection":"receipt","agent_id":profile})
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agents WHERE home_id IS NULL AND name=ANY($1)")
            .bind(vec!["Shared once", "Current shared profile"])
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(count, 1);
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(admin.id())
            .bind(header)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(record["outcome"]["id"], json!(profile));
    assert!(!serde_json::to_string(&record)?.contains("native profile sentinel"));
    Ok(())
}

#[tokio::test]
async fn a_push_subscription_retry_is_an_empty_acknowledgement_without_refreshing_its_native_row()
-> Proof {
    show_internal_failures();
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use orch_api_contract::notifications::MePushSubscriptionCreate;
    let w = Desk::new().await;
    let header = new_id().to_string();
    let mut point = [7_u8; 65];
    point[0] = 4;
    let input = json!({"endpoint":"https://push.example/one-native-device","keys":{
        "p256dh":URL_SAFE_NO_PAD.encode(point),"auth":URL_SAFE_NO_PAD.encode([9_u8;16])}});
    let send = |secret: String| {
        let app = w.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(MePushSubscriptionCreate, &[])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .header("User-Agent", "original-browser")
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    assert_eq!(send(w.ada.token.clone()).await.status, 204);
    let device: Id = sqlx::query_scalar("SELECT id FROM push_subscriptions WHERE user_id=$1")
        .bind(w.ada.id())
        .fetch_one(&w.world.db.pool)
        .await?;
    sqlx::query("UPDATE push_subscriptions SET failed_count=2 WHERE id=$1")
        .bind(device)
        .execute(&w.world.db.pool)
        .await?;
    let before: (String, i32, String) = sqlx::query_as(
        "SELECT xmin::text,failed_count,user_agent FROM push_subscriptions WHERE id=$1",
    )
    .bind(device)
    .fetch_one(&w.world.db.pool)
    .await?;
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule(
            "MePushSubscriptionList",
            "account",
            w.ada.id(),
            "self",
            "read",
        )],
    )
    .await?;
    let repeated = send(actor.secret.clone()).await;
    assert_eq!(repeated.status, 204);
    assert!(repeated.raw.is_empty());
    let after: (String, i32, String) = sqlx::query_as(
        "SELECT xmin::text,failed_count,user_agent FROM push_subscriptions WHERE id=$1",
    )
    .bind(device)
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(
        before, after,
        "replay neither renews the endpoint nor resets its failures"
    );
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(w.ada.id())
            .bind(header)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(record["outcome"]["id"], json!(device));
    let record = serde_json::to_string(&record)?;
    assert!(!record.contains("push.example"));
    assert!(
        !record.contains(
            input["keys"]["auth"]
                .as_str()
                .ok_or("fixture auth missing")?
        )
    );
    let fresh = w
        .world
        .app
        .call(MePushSubscriptionCreate, &[])
        .token(&actor.secret)
        .header("Idempotency-Key", &new_id().to_string())
        .raw_body(&input)
        .send_bytes()
        .await;
    assert_eq!(fresh.status, 403);
    Ok(())
}

#[tokio::test]
async fn a_person_notice_retry_keeps_one_native_delivery_and_audit_without_storing_its_body()
-> Proof {
    show_internal_failures();
    use orch_api_contract::notifications::PersonNotifyCreate;
    let w = Desk::new().await;
    let reader = w.member("notice-reader", "viewer").await;
    let doc = w.make(&w.ada, "One native notice").await.base.id;
    let header = new_id().to_string();
    let input = json!({"user_ids":[reader.id()],"note":"Private request-body sentinel"});
    let send = |secret: String| {
        let app = w.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(PersonNotifyCreate, &[&"documents", &doc])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    assert_eq!(send(w.ada.token.clone()).await.status, 204);
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule("DocumentGet", "document", doc, "self", "read")],
    )
    .await?;
    let repeated = send(actor.secret).await;
    assert_eq!(repeated.status, 204);
    assert!(repeated.raw.is_empty());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(reader.id())
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(count, 1);
    let audits: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM audit_log WHERE actor_id=$1 AND action=$2 AND object_id=$3",
    )
    .bind(w.ada.id())
    .bind(orch_access_contract::audit::AuditAction::PersonNotified.as_str())
    .bind(doc)
    .fetch_all(&w.world.db.pool)
    .await?;
    assert_eq!(audits.len(), 1);
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(w.ada.id())
            .bind(header)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(record["outcome"]["id"], json!(audits[0]));
    assert!(!serde_json::to_string(&record)?.contains("Private request-body sentinel"));
    assert!(record.get("user_ids").is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_setup_retries_create_one_session_then_return_only_the_sign_in_receipt() -> Proof
{
    show_internal_failures();
    use orch_api_contract::auth::SetupCreate;
    let world = support::World::new().await;
    let header = new_id().to_string();
    let input = json!({"username":"first-admin","display_name":"First administrator","password":support::PASSWORD});
    let send = || async {
        let call = world
            .app
            .call(SetupCreate, &[])
            .header("Idempotency-Key", &header)
            .raw_body(&input);
        call.send_bytes().await
    };
    let (left, right) = tokio::join!(send(), send());
    let left_value = value(&left, 200);
    let right_value = value(&right, 200);
    let (first, repeated) = if left_value.get("access_token").is_some() {
        (&left_value, &right)
    } else {
        (&right_value, &left)
    };
    assert!(first["access_token"].is_string());
    let account: Id = serde_json::from_value(first["user"]["id"].clone())?;
    let later = send().await;
    for reply in [repeated, &later] {
        let receipt = value(reply, 200);
        assert_eq!(
            receipt,
            json!({"projection":"receipt","user_id":account,"sign_in_required":true})
        );
        assert!(reply.headers.get_all("set-cookie").iter().next().is_none());
        assert_eq!(
            reply
                .headers
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
    }
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM sessions WHERE user_id=$1)",
    )
    .bind(account)
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(counts, (1, 1));
    let wrong = json!({"password":"wrong-but-valid-secret","username":"first-admin","display_name":"First administrator"});
    let ordinary = world
        .app
        .call(SetupCreate, &[])
        .raw_body(&wrong)
        .send_bytes()
        .await;
    let refused = world
        .app
        .call(SetupCreate, &[])
        .header("Idempotency-Key", &header)
        .raw_body(&wrong)
        .send_bytes()
        .await;
    assert_same_public_refusal(&ordinary, &refused);
    Ok(())
}

#[tokio::test]
async fn registration_retries_prove_the_account_and_never_return_or_mint_another_session() -> Proof
{
    show_internal_failures();
    use orch_api_contract::auth::AuthRegister;
    let world = support::World::new().await;
    world.set("registration.mode", json!("open")).await;
    let header = new_id().to_string();
    let input = json!({"username":"registered-once","display_name":"Registered once","password":support::PASSWORD});
    let send = |input: Json| {
        let app = world.app.clone();
        let header = header.clone();
        async move {
            let call = app
                .call(AuthRegister, &[])
                .header("Idempotency-Key", &header)
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(input.clone()).await, 201);
    let account: Id = serde_json::from_value(first["user"]["id"].clone())?;
    let reply = send(input.clone()).await;
    assert_eq!(
        value(&reply, 201),
        json!({"projection":"receipt","user_id":account,"sign_in_required":true})
    );
    assert!(reply.headers.get_all("set-cookie").iter().next().is_none());
    let mut changed = input.clone();
    changed["display_name"] = json!("A different request");
    assert_eq!(
        value(&send(changed).await, 409)["code"],
        "conflict.idempotency_mismatch"
    );
    let mut wrong = input.clone();
    wrong["password"] = json!("wrong-but-valid-secret");
    let ordinary = world
        .app
        .call(AuthRegister, &[])
        .raw_body(&wrong)
        .send_bytes()
        .await;
    assert_same_public_refusal(&ordinary, &send(wrong).await);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM users),(SELECT count(*) FROM sessions WHERE user_id=$1)",
    )
    .bind(account)
    .fetch_one(&world.db.pool)
    .await?;
    assert_eq!(counts, (1, 1));
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(account)
            .bind(header)
            .fetch_one(&world.db.pool)
            .await?;
    let encoded = serde_json::to_string(&record)?;
    assert!(!encoded.contains(support::PASSWORD));
    assert!(
        !encoded.contains(
            first["access_token"]
                .as_str()
                .ok_or("first access token missing")?
        )
    );
    assert_eq!(record["outcome"]["id"], json!(account));
    Ok(())
}

fn assert_same_public_refusal(ordinary: &RawReply, repeated: &RawReply) {
    assert_eq!(repeated.status, ordinary.status);
    assert!(!repeated.status.is_success());
    let ordinary: Json = serde_json::from_slice(&ordinary.raw).unwrap();
    let repeated: Json = serde_json::from_slice(&repeated.raw).unwrap();
    for field in ["code", "detail", "errors", "required"] {
        assert_eq!(repeated[field], ordinary[field], "{field}");
    }
    assert!(repeated.get("user_id").is_none());
    assert!(repeated.get("sign_in_required").is_none());
}

#[tokio::test]
async fn a_cap_creation_replays_its_current_native_content_without_another_insert_or_spent_manage()
-> Proof {
    show_internal_failures();
    use orch_api_contract::usage::{UsageCapCreate, UsageCapUpdate};
    let s = rig::Scene::new().await;
    let at = s.tree.project.base.id;
    let header = new_id().to_string();
    let input = json!({
        "node_id":at,"subject":{"kind":"everyone"},"split":"shared",
        "meter":"tokens","period":"day","amount":"1000","note":"First cap"
    });
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(UsageCapCreate, &[])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(s.ada.token.clone()).await, 201);
    let cap = id(&first);
    let changed = s
        .rig
        .world
        .app
        .call(UsageCapUpdate, &[&cap])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":first["revision"],"note":"Current cap"}))
        .send_bytes()
        .await;
    let changed = value(&changed, 200);
    for operation in ["UsageCapCreate", "UsageCapList"] {
        let actor = key(
            &s.rig.world,
            s.ada.id(),
            vec![rule(operation, "project", at, "self", "read")],
        )
        .await?;
        let replay = value(&send(actor.secret).await, 201);
        assert_eq!(id(&replay), cap);
        if operation == "UsageCapCreate" {
            assert_eq!(replay["note"], "Current cap");
            assert_eq!(replay["revision"], changed["revision"]);
        } else {
            assert_eq!(replay["projection"], "receipt");
            assert_eq!(replay["revision"], first["revision"]);
            assert!(replay.get("note").is_none());
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_caps WHERE node_id=$1")
        .bind(at)
        .fetch_one(s.rig.db.pool())
        .await?;
    assert_eq!(count, 1);
    let row = orch_db::usage::caps::get(&mut *s.rig.world.conn().await, cap)
        .await?
        .ok_or("the native cap is missing")?;
    assert_eq!(json!(row.revision), changed["revision"]);
    assert_eq!(row.note, "Current cap");
    Ok(())
}

#[tokio::test]
async fn an_announcement_creation_replays_one_native_delivery_with_current_own_read_or_its_counts_receipt()
-> Proof {
    show_internal_failures();
    use orch_api_contract::topics::NodeAnnouncementCreate;
    let s = rig::Scene::new().await;
    let left = s.thread_by(&s.ada, "First recipient").await.base.id;
    let right = s.thread_by(&s.ada, "Second recipient").await.base.id;
    let at = s.tree.topic.base.id;
    let header = new_id().to_string();
    let input = json!({"title":"One announcement","body":"Only sent once","wake_idle":false});
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let header = header.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(NodeAnnouncementCreate, &[&at])
                .token(&secret)
                .header("Idempotency-Key", &header)
                .raw_body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(s.ada.token.clone()).await, 201);
    let group = first["group_id"].clone();
    let announcement = first["announcements"][0]["id"].clone();
    assert!(group.is_string() && announcement.is_string());
    let before:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM announcements WHERE group_id=$1),(SELECT count(*) FROM thread_inputs WHERE thread_id=ANY($2))")
        .bind(serde_json::from_value::<Id>(group.clone())?).bind(vec![left,right]).fetch_one(s.rig.db.pool()).await?;
    for operation in ["NodeAnnouncementCreate", "NodeAnnouncementList"] {
        let actor = key(
            &s.rig.world,
            s.ada.id(),
            vec![rule(operation, "topic", at, "self", "read")],
        )
        .await?;
        let again = value(&send(actor.secret).await, 201);
        assert_eq!(again["group_id"], group);
        assert_eq!(again["announcements"][0]["id"], announcement);
        if operation == "NodeAnnouncementCreate" {
            assert_eq!(again["announcements"][0]["body"], "Only sent once");
        } else {
            assert_eq!(again["projection"], "receipt");
            assert!(again["announcements"][0].get("body").is_none());
        }
    }
    let after:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM announcements WHERE group_id=$1),(SELECT count(*) FROM thread_inputs WHERE thread_id=ANY($2))")
        .bind(serde_json::from_value::<Id>(group)?).bind(vec![left,right]).fetch_one(s.rig.db.pool()).await?;
    assert_eq!(after, before);
    Ok(())
}

#[tokio::test]
async fn a_new_replay_credential_admits_only_current_response_read_and_cannot_create_another_cap()
-> Proof {
    show_internal_failures();
    use orch_api_contract::usage::{UsageCapCreate, UsageCapUpdate};
    let s = rig::Scene::new().await;
    let home = s.tree.project.base.id;
    let header = new_id().to_string();
    let input = json!({"node_id":home,"subject":{"kind":"everyone"},"split":"shared",
        "meter":"tokens","period":"day","amount":"1000","note":"One accepted cap"});
    let call = s
        .rig
        .world
        .app
        .call(UsageCapCreate, &[])
        .token(&s.ada.token)
        .header("Idempotency-Key", &header)
        .raw_body(&input);
    let first = value(&call.send_bytes().await, 201);
    let cap = id(&first);
    let call = s
        .rig
        .world
        .app
        .call(UsageCapUpdate, &[&cap])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":first["revision"],"note":"Current stored answer"}));
    value(&call.send_bytes().await, 200);
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(s.ada.id())
    .bind(&header)
    .fetch_one(s.rig.db.pool())
    .await?;
    let read = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule("UsageCapCreate", "project", home, "self", "read")],
    )
    .await?;
    let call = s
        .rig
        .world
        .app
        .call(UsageCapCreate, &[])
        .token(&read.secret)
        .header("Idempotency-Key", &header)
        .raw_body(&input);
    let replay = value(&call.send_bytes().await, 201);
    assert_eq!(id(&replay), cap);
    assert_eq!(replay["note"], "Current stored answer");
    let no_read = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule("UsageCapCreate", "project", home, "self", "manage")],
    )
    .await?;
    let call = s
        .rig
        .world
        .app
        .call(UsageCapCreate, &[])
        .token(&no_read.secret)
        .header("Idempotency-Key", &header)
        .raw_body(&input);
    let receipt = value(&call.send_bytes().await, 201);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(id(&receipt), cap);
    assert!(receipt.get("note").is_none());
    let call = s
        .rig
        .world
        .app
        .call(UsageCapCreate, &[])
        .token(&read.secret)
        .header("Idempotency-Key", &new_id().to_string())
        .raw_body(&input);
    assert_eq!(
        call.send_bytes().await.status,
        http::StatusCode::FORBIDDEN,
        "Read of a completed answer cannot authorize a first creation"
    );
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(s.ada.id())
    .bind(&header)
    .fetch_one(s.rig.db.pool())
    .await?;
    assert_eq!(before, after);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_caps WHERE node_id=$1")
        .bind(home)
        .fetch_one(s.rig.db.pool())
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn administrator_creations_show_reset_secrets_once_and_replay_without_creating_an_account_or_link_again()
-> Proof {
    show_internal_failures();
    use orch_api_contract::{
        admin::{AdminUserCreate, AdminUserResetLinkCreate},
        auth::{AdminModeSetBody, MeAdminModeSet},
    };
    let w = Desk::new().await;
    let admin = places::admin(&w.world, "one-time-admin").await;
    let header = new_id().to_string();
    let input = json!({"username":"created-once","display_name":"Created once"});
    let create = || async {
        let call = w
            .world
            .app
            .call(AdminUserCreate, &[])
            .token(&admin.token)
            .header("Idempotency-Key", &header)
            .raw_body(&input);
        call.send_bytes().await
    };
    let first = value(&create().await, 200);
    let user = id(&first["user"]);
    let original_secret = first["reset_url"]
        .as_str()
        .ok_or("the first one-time URL is missing")?
        .rsplit('/')
        .next()
        .ok_or("native secret missing")?
        .to_owned();
    let repeated = create().await;
    assert_eq!(
        repeated
            .headers
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let replay = value(&repeated, 200);
    assert_eq!(id(&replay["user"]), user);
    assert!(replay["reset_url"].is_null());
    assert_eq!(replay["already_created"], true);
    let link_key = new_id().to_string();
    let link_input = json!({"clear_second_step":false});
    let make_link = || async {
        let call = w
            .world
            .app
            .call(AdminUserResetLinkCreate, &[&user])
            .token(&admin.token)
            .header("Idempotency-Key", &link_key)
            .raw_body(&link_input);
        call.send_bytes().await
    };
    let link = value(&make_link().await, 200);
    assert!(link["url"].is_string());
    let again = value(&make_link().await, 200);
    assert!(again["url"].is_null());
    assert_eq!(again["expires_at"], link["expires_at"]);
    assert_eq!(again["already_created"], true);
    let links: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM email_tokens WHERE user_id=$1 AND purpose='reset_password'",
    )
    .bind(user)
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(
        links, 2,
        "one initial account link and one explicit new link"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE username='created-once'")
        .fetch_one(&w.world.db.pool)
        .await?;
    assert_eq!(count, 1);
    let records: Vec<Json> = sqlx::query_scalar(
        "SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=ANY($2)",
    )
    .bind(admin.id())
    .bind(vec![header.clone(), link_key])
    .fetch_all(&w.world.db.pool)
    .await?;
    let encoded = serde_json::to_string(&records)?;
    assert!(!encoded.contains(&original_secret));
    assert!(
        !encoded.contains(
            link["url"]
                .as_str()
                .ok_or("link missing")?
                .rsplit('/')
                .next()
                .ok_or("secret missing")?
        )
    );
    w.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&admin.token)
        .body(&AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    let receipt = value(&create().await, 200);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["user_id"], json!(user));
    assert!(receipt.get("user").is_none());
    Ok(())
}

#[tokio::test]
async fn an_account_creation_replay_uses_its_current_response_without_the_spent_required_source()
-> Proof {
    show_internal_failures();
    use orch_api_contract::admin::{AdminUserCreate, AdminUserUpdate};
    let w = Desk::new().await;
    let admin = places::admin(&w.world, "response-source-admin").await;
    let header = new_id().to_string();
    let input = json!({"username":"response-source-account","display_name":"Accepted account"});
    let call = w
        .world
        .app
        .call(AdminUserCreate, &[])
        .token(&admin.token)
        .header("Idempotency-Key", &header)
        .raw_body(&input);
    let first = value(&call.send_bytes().await, 200);
    let user = id(&first["user"]);
    assert!(first["reset_url"].is_string());
    let call = w
        .world
        .app
        .call(AdminUserUpdate, &[&user])
        .token(&admin.token)
        .raw_body(&json!({"status":"suspended"}));
    assert_eq!(value(&call.send_bytes().await, 200)["status"], "suspended");
    let before: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(admin.id())
    .bind(&header)
    .fetch_one(&w.world.db.pool)
    .await?;
    let reader = key(
        &w.world,
        admin.id(),
        vec![json!({"target":{"kind":"system","reach":"self"},
        "resources":["account"],"allow":[{"operation":"AdminUserCreate","actions":["read"]}]})],
    )
    .await?;
    let call = w
        .world
        .app
        .call(AdminUserCreate, &[])
        .token(&reader.secret)
        .header("Idempotency-Key", &header)
        .raw_body(&input);
    let replay = value(&call.send_bytes().await, 200);
    assert_eq!(id(&replay["user"]), user);
    assert_eq!(replay["user"]["status"], "suspended");
    assert!(replay["reset_url"].is_null());
    assert_eq!(replay["already_created"], true);
    let after: (Json, String) = sqlx::query_as(
        "SELECT response,xmin::text FROM idempotency_keys WHERE user_id=$1 AND key=$2",
    )
    .bind(admin.id())
    .bind(&header)
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(before, after);
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM users WHERE id=$1),(SELECT count(*) FROM email_tokens WHERE user_id=$1 AND purpose='reset_password')")
        .bind(user).fetch_one(&w.world.db.pool).await?;
    assert_eq!(counts, (1, 1));
    Ok(())
}

#[tokio::test]
async fn a_checksum_import_creation_replays_its_native_count_without_another_catalogue_write_or_audit()
-> Proof {
    show_internal_failures();
    use orch_api_contract::admin::AdminHarnessChecksumsImport;
    let w = Desk::new().await;
    let admin = places::admin(&w.world, "checksum-admin").await;
    let document = serde_json::to_string(
        &json!([{"kind":"codex","version":"1.0.0","os":"linux","arch":"x86_64",
        "url":"https://builds.example/native.tar.gz","sha256":"00".repeat(32),"is_default":false,"source":"admin"}]),
    )?;
    let input = json!({"document":document});
    let header = new_id().to_string();
    let send = || async {
        let call = w
            .world
            .app
            .call(AdminHarnessChecksumsImport, &[])
            .token(&admin.token)
            .header("Idempotency-Key", &header)
            .raw_body(&input);
        call.send_bytes().await
    };
    let first = value(&send().await, 200);
    assert_eq!(first["count"], 1);
    let before = w.world.audit().await;
    let catalogue_before: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT key,revision,xmin::text FROM settings WHERE node_id IS NULL ORDER BY key",
    )
    .fetch_all(&w.world.db.pool)
    .await?;
    assert_eq!(value(&send().await, 200), first);
    assert_eq!(
        w.world.audit().await,
        before,
        "the accepted catalogue import is not audited twice"
    );
    let catalogue_after: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT key,revision,xmin::text FROM settings WHERE node_id IS NULL ORDER BY key",
    )
    .fetch_all(&w.world.db.pool)
    .await?;
    assert_eq!(catalogue_after, catalogue_before);
    let stored: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(admin.id())
            .bind(&header)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert!(stored["outcome"]["id"].is_string());
    assert!(stored["outcome"]["data"].get("document").is_none());
    Ok(())
}

#[tokio::test]
async fn identity_provider_creation_replays_current_masked_configuration_or_a_receipt_after_admin_mode_ends()
-> Proof {
    show_internal_failures();
    use orch_api_contract::{
        admin::{AdminIdentityProviderCreate, AdminIdentityProviderUpdate},
        auth::{AdminModeSetBody, MeAdminModeSet},
    };
    let w = Desk::new().await;
    let admin = places::admin(&w.world, "provider-admin").await;
    let header = new_id().to_string();
    let input = json!({"key":"once-idp","name":"Original provider","issuer":"https://identity.example","client_id":"example-client","client_secret":"ephemeral-test-secret"});
    let send = || async {
        let call = w
            .world
            .app
            .call(AdminIdentityProviderCreate, &[])
            .token(&admin.token)
            .header("Idempotency-Key", &header)
            .raw_body(&input);
        call.send_bytes().await
    };
    let first = value(&send().await, 200);
    let created = id(&first);
    let changed = w
        .world
        .app
        .call(AdminIdentityProviderUpdate, &[&created])
        .token(&admin.token)
        .raw_body(&json!({"name":"Current provider","expected_revision":first["revision"]}))
        .send_bytes()
        .await;
    value(&changed, 200);
    let current = value(&send().await, 200);
    assert_eq!(id(&current), created);
    assert_eq!(current["name"], "Current provider");
    w.world
        .app
        .call(MeAdminModeSet, &[])
        .token(&admin.token)
        .body(&AdminModeSetBody { as_admin: false })
        .send()
        .await
        .ok();
    let receipt = value(&send().await, 200);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["identity_provider_id"], json!(created));
    assert!(receipt.get("issuer").is_none() && receipt.get("client_secret_set").is_none());
    let stored: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(admin.id())
            .bind(&header)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert!(!serde_json::to_string(&stored)?.contains("ephemeral-test-secret"));
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM identity_providers WHERE key='once-idp'")
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test]
async fn role_creation_replay_reads_the_current_role_under_its_own_read_or_keeps_the_original_receipt()
-> Proof {
    show_internal_failures();
    use orch_api_contract::access::{NodeRoleCreate, RoleUpdate};
    let w = Desk::new().await;
    let recipient = w.member("role-recipient", "viewer").await;
    let input = json!({"subject":{"kind":"user","id":recipient.id()},"role":"interactor","note":"Original role note"});
    let header = new_id().to_string();
    let at = w.project;
    let first = w
        .world
        .app
        .call(NodeRoleCreate, &[&at])
        .token(&w.ada.token)
        .header("Idempotency-Key", &header)
        .raw_body(&input)
        .send_bytes()
        .await;
    let first = value(&first, 200);
    let created = id(&first);
    let changed = w
        .world
        .app
        .call(RoleUpdate, &[&created])
        .token(&w.ada.token)
        .raw_body(&json!({"note":"Current role note","expected_revision":first["revision"]}))
        .send_bytes()
        .await;
    value(&changed, 200);
    for operation in ["NodeRoleCreate", "NodeRoleList"] {
        let actor = key(
            &w.world,
            w.ada.id(),
            vec![rule(operation, "project", at, "self", "read")],
        )
        .await?;
        let again = w
            .world
            .app
            .call(NodeRoleCreate, &[&at])
            .token(&actor.secret)
            .header("Idempotency-Key", &header)
            .raw_body(&input)
            .send_bytes()
            .await;
        let again = value(&again, 200);
        if operation == "NodeRoleCreate" {
            assert_eq!(id(&again), created);
            assert_eq!(again["note"], "Current role note");
        } else {
            assert_eq!(again["projection"], "receipt");
            assert_eq!(again["role_id"], json!(created));
            assert_eq!(again["revision"], first["revision"]);
            assert!(again.get("note").is_none());
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_access_request_creation_replays_one_batch_and_notifications_under_current_read_or_receipt()
-> Proof {
    show_internal_failures();
    use orch_api_contract::access::AccessRequestCreate;
    let w = Desk::new().await;
    let requester = w.member("one-requester", "viewer").await;
    let input = json!({"items":[{"object":{"kind":"project","id":w.project},"permissions":["project.interact"]}],"reason":"One accepted request"});
    let header = new_id().to_string();
    let first = w
        .world
        .app
        .call(AccessRequestCreate, &[])
        .token(&requester.token)
        .header("Idempotency-Key", &header)
        .raw_body(&input)
        .send_bytes()
        .await;
    let first = value(&first, 200);
    assert_eq!(
        first["approvals"]
            .as_array()
            .ok_or("native approval cards missing")?
            .len(),
        1
    );
    let batch = first["batch_id"].clone();
    let approval = first["approvals"][0]["id"].clone();
    let notices_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE data->>'batch_id'=$1")
            .bind(batch.as_str().ok_or("native batch identity missing")?)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert!(
        notices_before > 0,
        "the native owners received the accepted ask"
    );
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE requester_user_id=$1")
            .bind(requester.id())
            .fetch_one(&w.world.db.pool)
            .await?;
    for operation in ["AccessRequestCreate", "AccessRequestGet"] {
        let actor = key(
            &w.world,
            requester.id(),
            vec![
                rule(operation, "org", w.org, "subtree", "read"),
                rule(operation, "account", requester.id(), "self", "read"),
                rule(operation, "account", w.ada.id(), "self", "read"),
            ],
        )
        .await?;
        let again = w
            .world
            .app
            .call(AccessRequestCreate, &[])
            .token(&actor.secret)
            .header("Idempotency-Key", &header)
            .raw_body(&input)
            .send_bytes()
            .await;
        let again = value(&again, 200);
        assert_eq!(again["batch_id"], batch);
        if operation == "AccessRequestCreate" {
            assert_eq!(again["approvals"][0]["id"], approval);
        } else {
            assert_eq!(again["projection"], "receipt");
            assert_eq!(again["approval_ids"], json!([approval]));
            assert!(again.get("approvals").is_none());
        }
    }
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE requester_user_id=$1")
            .bind(requester.id())
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(after, before);
    let notices_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE data->>'batch_id'=$1")
            .bind(batch.as_str().ok_or("native batch identity missing")?)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(notices_after, notices_before);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn expiry_cannot_delete_the_identity_claim_of_a_native_creation_still_waiting_to_write()
-> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let token = new_id().to_string();
    let at = w.topic;
    let input = body("One creation across its replay window");
    let mut blocker = w.world.db.pool.begin().await?;
    sqlx::query("LOCK TABLE documents IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;
    let app = w.world.app.clone();
    let secret = w.ada.token.clone();
    let held_key = token.clone();
    let held_body = input.clone();
    let pending = tokio::spawn(async move {
        let call = app
            .call(NodeDocumentCreate, &[&at])
            .token(&secret)
            .header("Idempotency-Key", &held_key)
            .body(&held_body);
        call.send_bytes().await
    });
    tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(
                !pending.is_finished(),
                "the native document writer must still be held"
            );
            let claimed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM idempotency_keys WHERE user_id=$1 AND key=$2)",
            )
            .bind(w.ada.id())
            .bind(&token)
            .fetch_one(&w.world.db.pool)
            .await
            .unwrap();
            if claimed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    w.world.advance(chrono::Duration::hours(25));
    let mut conn = w.world.conn().await;
    let before = w.world.now() - chrono::Duration::hours(24);
    assert_eq!(orch_db::idempotency::purge(&mut conn, before).await?, 0);
    drop(conn);
    blocker.rollback().await?;
    let accepted = tokio::time::timeout(rig::PATIENCE, pending).await??;
    value(&accepted, 201);
    assert_eq!(documents(&w).await, 1);
    let mut conn = w.world.conn().await;
    assert_eq!(orch_db::idempotency::purge(&mut conn, before).await?, 1);
    Ok(())
}

#[tokio::test]
async fn raw_upload_replay_keeps_one_native_private_blob_and_rejects_changed_bytes_or_headers()
-> Proof {
    show_internal_failures();
    use orch_api_contract::threads::{FILENAME_HEADER, UploadCreate};
    let w = Desk::new().await;
    // One pixel is the smallest valid native PNG for this transport control.
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1).write_to(&mut png, image::ImageFormat::Png)?;
    let token = new_id().to_string();
    let send = |secret: String, name: String, data: Vec<u8>| {
        let app = w.world.app.clone();
        let token = token.clone();
        async move {
            let call = app
                .call(UploadCreate, &[])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .header("content-type", "image/png")
                .header(FILENAME_HEADER, &name)
                .bytes(&data);
            call.send_bytes().await
        }
    };
    let first = value(
        &send(
            w.ada.token.clone(),
            "once.png".into(),
            png.get_ref().clone(),
        )
        .await,
        200,
    );
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule("NodeGet", "org", w.org, "self", "read")],
    )
    .await?;
    let repeat = value(
        &send(
            actor.secret.clone(),
            "once.png".into(),
            png.get_ref().clone(),
        )
        .await,
        200,
    );
    assert_eq!(
        repeat, first,
        "the generated acknowledgement needs no spent upload Write"
    );
    for (name, bytes) in [
        ("another.png", png.get_ref().clone()),
        ("once.png", b"different body".to_vec()),
    ] {
        let refused = send(actor.secret.clone(), name.into(), bytes).await;
        assert_eq!(refused.status.as_u16(), 409);
        assert_eq!(
            refused.problem().code,
            ErrorCode::ConflictIdempotencyMismatch
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM completed_uploads WHERE user_id=$1")
        .bind(w.ada.id())
        .fetch_one(&w.world.db.pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn raw_attachment_replays_one_reserved_path_and_canonicalizes_its_native_reference_json()
-> Proof {
    show_internal_failures();
    use orch_api_contract::threads::{FILENAME_HEADER, ThreadAttachmentCreate};
    let s = rig::Scene::new().await;
    let thread = s.thread().await.base.id;
    let token = new_id().to_string();
    let bytes = b"one private attachment";
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let token = token.clone();
        async move {
            let call = app
                .call(ThreadAttachmentCreate, &[&thread])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .header(FILENAME_HEADER, "once.txt")
                .bytes(bytes);
            call.send_bytes().await
        }
    };
    let first = value(&send(s.ada.token.clone()).await, 200);
    assert_eq!(value(&send(s.ada.token.clone()).await, 200), first);
    let actor = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule("NodeGet", "thread", thread, "self", "read")],
    )
    .await?;
    assert_eq!(value(&send(actor.secret.clone()).await, 200), first);
    let reference_key = new_id().to_string();
    let original = json!({"blob":first["blob"],"name":"from-reference.txt"});
    let call = s
        .rig
        .world
        .app
        .call(ThreadAttachmentCreate, &[&thread])
        .token(&s.ada.token)
        .header("Idempotency-Key", &reference_key)
        .raw_body(&original);
    let accepted = value(&call.send_bytes().await, 200);
    let reordered = format!(
        "{{\"name\":\"from-reference.txt\",\"blob\":{}}}",
        first["blob"]
    );
    let call = s
        .rig
        .world
        .app
        .call(ThreadAttachmentCreate, &[&thread])
        .token(&actor.secret)
        .header("Idempotency-Key", &reference_key)
        .header("content-type", "application/json")
        .bytes(reordered.as_bytes());
    assert_eq!(value(&call.send_bytes().await, 200), accepted);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_attachment_uploads WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(
        count, 2,
        "one direct and one reference attempt, with no second reservation for either"
    );
    Ok(())
}

#[tokio::test]
async fn the_configured_window_can_exceed_a_day_or_disable_header_replay() -> Proof {
    show_internal_failures();
    for hours in [48, 0] {
        let w = Desk::new().await;
        let window_value = json!(hours);
        orch_db::queries::settings::put(
            &mut *w.world.conn().await,
            None,
            orch_settings_contract::registry::keys::API_IDEMPOTENCY_WINDOW,
            &orch_db::queries::settings::Stored {
                value: &window_value,
                locked: false,
                by: Some(w.ada.id()),
                at: w.world.now(),
            },
            None,
        )
        .await?;
        let token = new_id().to_string();
        let input = body("One configured retry window");
        let first = make(&w, &w.ada.token, w.topic, &token, &input).await;
        let first = id(&value(&first, 201));
        if hours > 0 {
            w.world.advance(chrono::Duration::hours(25));
        }
        let again = make(&w, &w.ada.token, w.topic, &token, &input).await;
        let again = id(&value(&again, 201));
        if hours == 0 {
            assert_ne!(again, first);
            let records: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM idempotency_keys WHERE user_id=$1 AND key=$2",
            )
            .bind(w.ada.id())
            .bind(&token)
            .fetch_one(&w.world.db.pool)
            .await?;
            assert_eq!(
                records, 0,
                "disabled replay must not leave an in-flight claim for the writer or purger"
            );
        } else {
            assert_eq!(again, first, "one day is the default rather than a cap");
        }
    }
    Ok(())
}

async fn personal_retry<E: Endpoint<Reply = ()> + Copy>(
    w: &Desk,
    endpoint: E,
    body: Json,
) -> Proof {
    let token = new_id().to_string();
    let first = w
        .world
        .app
        .call(endpoint, &[])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        first.status.as_u16(),
        204,
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let before: (f64, String) = sqlx::query_as(
        "SELECT visit_count,xmin::text FROM recents WHERE user_id=$1 AND kind='topic' AND id=$2",
    )
    .bind(w.ada.id())
    .bind(w.topic)
    .fetch_one(&w.world.db.pool)
    .await?;
    w.world.advance(chrono::Duration::seconds(10));
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule("NodeGet", "topic", w.topic, "self", "read")],
    )
    .await?;
    let repeat = w
        .world
        .app
        .call(endpoint, &[])
        .token(&actor.secret)
        .header("Idempotency-Key", &token)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        repeat.status.as_u16(),
        204,
        "{}",
        String::from_utf8_lossy(&repeat.raw)
    );
    assert!(repeat.raw.is_empty());
    let after: (f64, String) = sqlx::query_as(
        "SELECT visit_count,xmin::text FROM recents WHERE user_id=$1 AND kind='topic' AND id=$2",
    )
    .bind(w.ada.id())
    .bind(w.topic)
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(
        after, before,
        "the accepted visit is neither counted nor rewritten twice"
    );
    let fresh = new_id().to_string();
    let refused = w
        .world
        .app
        .call(endpoint, &[])
        .token(&actor.secret)
        .header("Idempotency-Key", &fresh)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 403);
    Ok(())
}

#[tokio::test]
async fn identity_free_personal_creations_replay_empty_acknowledgements_without_counting_another_visit()
-> Proof {
    show_internal_failures();
    use orch_api_contract::personal::{MeRecentCreate, MeVisitCreate};
    let w = Desk::new().await;
    personal_retry(&w, MeRecentCreate, json!({"node_id":w.topic})).await?;
    personal_retry(&w, MeVisitCreate, json!({"kind":"topic","id":w.topic})).await?;
    Ok(())
}

#[tokio::test]
async fn an_upload_session_creation_replays_its_generated_acknowledgement_and_admits_the_first_write()
-> Proof {
    show_internal_failures();
    use orch_api_contract::threads::{UploadSessionCreate, UploadSessionCreateBody};
    let w = Desk::new().await;
    let token = new_id().to_string();
    let input = UploadSessionCreateBody {
        size: 3,
        name: "one-upload.bin".into(),
        media_type: None,
    };
    let first = w
        .world
        .app
        .call(UploadSessionCreate, &[])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .body(&input)
        .send_bytes()
        .await;
    let first = value(&first, 200);
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule("NodeGet", "org", w.org, "self", "read")],
    )
    .await?;
    let repeat = w
        .world
        .app
        .call(UploadSessionCreate, &[])
        .token(&actor.secret)
        .header("Idempotency-Key", &token)
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(value(&repeat, 200), first);
    let fresh = new_id().to_string();
    let refused = w
        .world
        .app
        .call(UploadSessionCreate, &[])
        .token(&actor.secret)
        .header("Idempotency-Key", &fresh)
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(
        refused.status.as_u16(),
        403,
        "the first write still needs its exact native key action"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM upload_sessions WHERE user_id=$1")
        .bind(w.ada.id())
        .fetch_one(&w.world.db.pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

async fn node_retry<E: Endpoint + Copy>(
    w: &Desk,
    endpoint: E,
    at: Option<Id>,
    body: Json,
    kind: &str,
) -> Proof<Id> {
    let token = new_id().to_string();
    let send = || async {
        let params: Vec<&dyn std::fmt::Display> = at
            .as_ref()
            .map(|id| vec![id as &dyn std::fmt::Display])
            .unwrap_or_default();
        let call = w
            .world
            .app
            .call(endpoint, &params)
            .token(&w.ada.token)
            .header("Idempotency-Key", &token)
            .raw_body(&body);
        call.send_bytes().await
    };
    let first = send().await;
    let status = first.status.as_u16();
    assert!(
        first.status.is_success(),
        "{}",
        String::from_utf8_lossy(&first.raw)
    );
    let first = value(&first, status);
    let made = id(&first);
    let repeated = send().await;
    assert_eq!(id(&value(&repeated, status)), made);
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![rule("NodeGet", kind, made, "self", "read")],
    )
    .await?;
    let params: Vec<&dyn std::fmt::Display> = at
        .as_ref()
        .map(|id| vec![id as &dyn std::fmt::Display])
        .unwrap_or_default();
    let receipt = w
        .world
        .app
        .call(endpoint, &params)
        .token(&actor.secret)
        .header("Idempotency-Key", &token)
        .raw_body(&body)
        .send_bytes()
        .await;
    let receipt = value(&receipt, status);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["node_id"], json!(made));
    assert!(receipt.get("name").is_none());
    let matched: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM nodes WHERE kind=$1 AND name=$2 AND created_by=$3",
    )
    .bind(kind)
    .bind(first["name"].as_str().unwrap())
    .bind(w.ada.id())
    .fetch_one(&w.world.db.pool)
    .await?;
    assert_eq!(matched, 1);
    Ok(made)
}

#[tokio::test]
async fn native_org_project_and_topic_creators_share_the_key_seam_and_keep_their_own_read_receipts()
-> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    places::invite_to_make_org(&w.world, &w.ada).await;
    let root = node_retry(
        &w,
        OrgCreate,
        None,
        json!({"name":"One root retry","slug":"one-root-retry"}),
        "org",
    )
    .await?;
    node_retry(
        &w,
        OrgChildCreate,
        Some(root),
        json!({"name":"One department retry","slug":"one-department-retry","general":false}),
        "org",
    )
    .await?;
    let project = node_retry(
        &w,
        ProjectCreate,
        Some(w.org),
        json!({"name":"One project retry"}),
        "project",
    )
    .await?;
    node_retry(
        &w,
        ProjectChildCreate,
        Some(project),
        json!({"name":"One folder retry"}),
        "project",
    )
    .await?;
    node_retry(
        &w,
        TopicCreate,
        Some(project),
        json!({"name":"One topic retry"}),
        "topic",
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn compound_project_start_replays_its_topic_and_first_thread_as_one_creation() -> Proof {
    show_internal_failures();
    let s = rig::Scene::new().await;
    let token = new_id().to_string();
    let project = s.tree.project.base.id;
    let input = TopicCreateBody {
        name: Some("One compound topic".into()),
        first_thread: Some(CreateThread {
            name: Some("One compound thread".into()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: s.on.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.into()),
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
            directory: Some("web".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let token = token.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(TopicCreate, &[&project])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .body(&input);
            call.send_bytes().await
        }
    };
    let first = value(&send(s.ada.token.clone()).await, 200);
    let topic = id(&first["topic"]);
    let thread = id(&first["thread"]);
    let again = value(&send(s.ada.token.clone()).await, 200);
    assert_eq!(id(&again["topic"]), topic);
    assert_eq!(id(&again["thread"]), thread);
    let actor = key(
        &s.rig.world,
        s.ada.id(),
        vec![rule("NodeGet", "thread", thread, "self", "read")],
    )
    .await?;
    let receipt = value(&send(actor.secret).await, 200);
    assert_eq!(receipt["projection"], "receipt");
    assert_eq!(receipt["topic_id"], json!(topic));
    assert_eq!(receipt["thread_id"], json!(thread));
    assert!(receipt.get("topic").is_none() && receipt.get("thread").is_none());
    let actual:(i64,i64)=sqlx::query_as("SELECT count(*) FILTER(WHERE kind='topic'),count(*) FILTER(WHERE kind='thread') FROM nodes WHERE created_by=$1 AND name=ANY($2)")
        .bind(s.ada.id()).bind(vec!["One compound topic","One compound thread"]).fetch_one(s.rig.db.pool()).await?;
    assert_eq!(actual, (1, 1));
    Ok(())
}

fn value(reply: &RawReply, status: u16) -> Json {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn id(value: &Json) -> Id {
    serde_json::from_value(value["id"].clone()).unwrap()
}
async fn documents(w: &Desk) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM documents WHERE created_by=$1")
        .bind(w.ada.id())
        .fetch_one(&w.world.db.pool)
        .await
        .unwrap()
}
async fn make(
    w: &Desk,
    secret: &str,
    at: Id,
    token: &str,
    body: &NodeDocumentCreateBody,
) -> RawReply {
    let call = w
        .world
        .app
        .call(NodeDocumentCreate, &[&at])
        .token(secret)
        .header("Idempotency-Key", token)
        .body(body);
    call.send_bytes().await
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_document_retries_make_one_and_replay_current_content_without_cached_bytes()
-> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let token = new_id().to_string();
    let input = body("A lost document answer");
    let before = documents(&w).await;
    let (left, right) = tokio::join!(
        make(&w, &w.ada.token, w.topic, &token, &input),
        make(&w, &w.ada.token, w.topic, &token, &input)
    );
    let left = value(&left, 201);
    let right = value(&right, 201);
    let created = id(&left);
    assert_eq!(id(&right), created);
    assert_eq!(documents(&w).await, before + 1);
    assert_eq!(left["title"], "A lost document answer");
    sqlx::query("UPDATE documents SET title='The current title' WHERE id=$1")
        .bind(created)
        .execute(&w.world.db.pool)
        .await?;
    let replay = make(&w, &w.ada.token, w.topic, &token, &input).await;
    let replay = value(&replay, 201);
    assert_eq!(id(&replay), created);
    assert_eq!(replay["title"], "The current title");
    let record: Json =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(w.ada.id())
            .bind(&token)
            .fetch_one(&w.world.db.pool)
            .await?;
    assert_eq!(record["outcome"]["id"], json!(created));
    assert!(record.get("title").is_none() && record["outcome"].get("title").is_none());
    assert!(!serde_json::to_string(&record)?.contains("A lost document answer"));
    assert_eq!(documents(&w).await, before + 1);
    Ok(())
}

#[tokio::test]
async fn document_replay_survives_session_renewal_and_uses_the_new_keys_own_read_facet() -> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let signed = w.world.sign_in(&w.ada.person).await;
    let token = new_id().to_string();
    let input = body("Private retry content");
    let first = make(&w, &signed.result.access_token, w.topic, &token, &input).await;
    let created = id(&value(&first, 201));
    let refreshed = w
        .world
        .app
        .call(AuthRefresh, &[])
        .header("cookie", &signed.browser())
        .header("X-Orch-Csrf", "1")
        .send()
        .await
        .ok();
    assert_ne!(refreshed.access_token, signed.result.access_token);
    let renewed = make(&w, &refreshed.access_token, w.topic, &token, &input).await;
    assert_eq!(id(&value(&renewed, 201)), created);
    for operation in ["NodeDocumentCreate", "DocumentGet"] {
        let actor = key(
            &w.world,
            w.ada.id(),
            vec![rule(operation, "document", created, "self", "read")],
        )
        .await?;
        let replay = make(&w, &actor.secret, w.topic, &token, &input).await;
        let replay = value(&replay, 201);
        if operation == "NodeDocumentCreate" {
            assert_eq!(id(&replay), created, "replay needs no spent creation Write");
            assert_eq!(replay["title"], "Private retry content");
        } else {
            assert_eq!(replay["projection"], "receipt");
            assert_eq!(replay["document_id"], json!(created));
            assert!(replay.get("title").is_none() && replay.get("blocks").is_none());
        }
    }
    assert_eq!(documents(&w).await, 1);
    Ok(())
}

#[tokio::test]
async fn canonical_bodies_match_but_a_changed_body_or_path_is_a_conflict_until_the_window_ends()
-> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let token = new_id().to_string();
    let first_body = json!({"title":"Canonical", "access":"follows"});
    let first = w
        .world
        .app
        .call(NodeDocumentCreate, &[&w.topic])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .raw_body(&first_body)
        .send_bytes()
        .await;
    let created = id(&value(&first, 201));
    // A different JSON member order and whitespace describe the same canonical request.
    let reordered = br#"{ "access": "follows", "title": "Canonical" }"#;
    let repeat = w
        .world
        .app
        .call(NodeDocumentCreate, &[&w.topic])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .header("content-type", "application/json")
        .bytes(reordered)
        .send_bytes()
        .await;
    assert_eq!(id(&value(&repeat, 201)), created);
    let changed = body("Different content");
    let refused = make(&w, &w.ada.token, w.topic, &token, &changed).await;
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(
        refused.problem().code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    let refused = w
        .world
        .app
        .call(DocumentUpdate, &[&created])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .raw_body(&json!({"title":"Not another act"}))
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(
        refused.problem().code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    let other_topic = w.another_topic("Another retry destination").await;
    let refused = w
        .world
        .app
        .call(NodeDocumentCreate, &[&other_topic])
        .token(&w.ada.token)
        .header("Idempotency-Key", &token)
        .raw_body(&first_body)
        .send_bytes()
        .await;
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(
        refused.problem().code,
        ErrorCode::ConflictIdempotencyMismatch
    );
    assert_eq!(documents(&w).await, 1);
    w.world.advance(chrono::Duration::hours(25));
    let fresh = make(&w, &w.ada.token, w.topic, &token, &changed).await;
    assert_ne!(id(&value(&fresh, 201)), created);
    assert_eq!(documents(&w).await, 2);
    Ok(())
}

#[tokio::test]
async fn the_same_key_value_belongs_to_each_account_independently() -> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let other = w.member("another-creator", "interactor").await;
    let token = new_id().to_string();
    let input = body("Independent account namespaces");
    let left = make(&w, &w.ada.token, w.topic, &token, &input).await;
    let right = make(&w, &other.token, w.topic, &token, &input).await;
    let left = id(&value(&left, 201));
    let right = id(&value(&right, 201));
    assert_ne!(left, right);
    assert_eq!(
        id(&value(
            &make(&w, &other.token, w.topic, &token, &input).await,
            201
        )),
        right
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn losing_the_request_after_native_commit_replays_the_identity_without_a_second_creation()
-> Proof {
    show_internal_failures();
    let w = Desk::new().await;
    let actor = key(
        &w.world,
        w.ada.id(),
        vec![
            rule("NodeDocumentCreate", "org", w.org, "subtree", "write"),
            rule("NodeDocumentCreate", "org", w.org, "subtree", "read"),
        ],
    )
    .await?;
    let token = new_id().to_string();
    let input = body("An accepted answer that is lost");
    let mut locked = w.world.db.pool.begin().await?;
    sqlx::query("LOCK TABLE api_token_activity IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await?;
    let app = w.world.app.clone();
    let secret = actor.secret;
    let held_token = token.clone();
    let held_input = input.clone();
    let at = w.topic;
    let pending = tokio::spawn(async move {
        let call = app
            .call(NodeDocumentCreate, &[&at])
            .token(&secret)
            .header("Idempotency-Key", &held_token)
            .body(&held_input);
        call.send_bytes().await
    });
    let created: Id = tokio::time::timeout(rig::PATIENCE, async {
        loop {
            assert!(
                !pending.is_finished(),
                "the real result publication must wait"
            );
            let row: Option<Id> =
                sqlx::query_scalar("SELECT id FROM documents WHERE created_by=$1")
                    .bind(w.ada.id())
                    .fetch_optional(&w.world.db.pool)
                    .await
                    .unwrap();
            if let Some(id) = row {
                break id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    locked.rollback().await?;
    let retry = tokio::time::timeout(
        rig::PATIENCE,
        make(&w, &w.ada.token, w.topic, &token, &input),
    )
    .await?;
    assert_eq!(id(&value(&retry, 201)), created);
    assert_eq!(documents(&w).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_thread_creation_returns_one_thread_and_current_read_only_or_receipt() -> Proof {
    show_internal_failures();
    let s = rig::Scene::new().await;
    let token = new_id().to_string();
    let input = CreateThread {
        name: Some("A retried native thread".into()),
        binding: Some(CreateThreadBinding {
            agent_id: s.on.agent,
            provider_id: s.on.provider,
            arrangement: None,
        }),
        model: Some(rig::MODEL.into()),
        environment: Some(EnvironmentChoice::Use(s.on.environment)),
        directory: Some("web".into()),
        ..CreateThread::default()
    };
    let at = s.tree.topic.base.id;
    let send = |secret: String| {
        let app = s.rig.world.app.clone();
        let token = token.clone();
        let input = input.clone();
        async move {
            let call = app
                .call(ThreadCreate, &[&at])
                .token(&secret)
                .header("Idempotency-Key", &token)
                .body(&input);
            call.send_bytes().await
        }
    };
    let (left, right) = tokio::join!(send(s.ada.token.clone()), send(s.ada.token.clone()));
    let created = id(&value(&left, 200));
    assert_eq!(id(&value(&right, 200)), created);
    for operation in ["ThreadCreate", "NodeGet"] {
        let actor = key(
            &s.rig.world,
            s.ada.id(),
            vec![
                rule(operation, "thread", created, "self", "read"),
                rule(operation, "environment", s.on.environment, "self", "read"),
            ],
        )
        .await?;
        let replay = value(&send(actor.secret).await, 200);
        if operation == "ThreadCreate" {
            assert_eq!(id(&replay), created);
        } else {
            assert_eq!(replay["projection"], "receipt");
            assert_eq!(replay["thread_id"], json!(created));
            assert!(replay.get("name").is_none());
        }
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM nodes WHERE kind='thread' AND created_by=$1")
            .bind(s.ada.id())
            .fetch_one(s.rig.db.pool())
            .await?;
    assert_eq!(count, 1);
    Ok(())
}
