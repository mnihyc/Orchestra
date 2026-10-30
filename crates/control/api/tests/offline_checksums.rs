//! Carried-in vendor checksums are an explicit administrator act whose digest remains in the audit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod admin_support;
mod places;
mod support;

use admin_support::{cast, lines_of};
use orch_api_contract::admin::{
    AdminHarnessChecksumsImport, AdminHarnessList, AdminHarnessSet, HarnessChecksumsImportBody,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use support::World;

#[tokio::test]
async fn making_a_mirror_build_default_preserves_its_origin_and_remains_subject_to_mirror_trust() {
    use orch_api_contract::admin::{AdminSettingSet, AdminSettingSetBody, HarnessBuildSource};
    let world = World::new().await;
    let c = cast(&world).await;
    let build: orch_api_contract::admin::HarnessBuild = serde_json::from_value(json!({
        "kind":"codex", "version":"0.170.0", "os":"linux", "arch":"x86_64",
        "url":"https://mirror.example.test/build", "sha256":"ab".repeat(32),
        "is_default":true, "source":"mirror"
    }))
    .unwrap();
    let key = "agents.harness_trust_mirror";
    world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(true),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let all = world
        .app
        .call(AdminHarnessSet, &[&build.kind, &build.version])
        .token(&c.root.token)
        .body(&build)
        .send()
        .await
        .ok();
    let kept = all
        .iter()
        .find(|kept| kept.version == build.version)
        .unwrap();
    assert!(kept.is_default);
    assert_eq!(
        kept.source,
        HarnessBuildSource::Mirror,
        "choosing a default cannot launder its checksum origin"
    );
    world
        .app
        .call(AdminSettingSet, &[&key])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let all = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(!all.iter().any(|kept| kept.version == build.version));
    let refused = world
        .app
        .call(AdminHarnessSet, &[&build.kind, &build.version])
        .token(&c.root.token)
        .body(&build)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn old_channel_metadata_cannot_select_or_execute_a_build_until_its_checksum_is_anchored() {
    let world = World::new().await;
    let c = cast(&world).await;
    let raw = json!({"kind":"codex", "version":"0.170.0", "os":"linux", "arch":"x86_64",
        "url":"https://example.test/build", "sha256":"ab".repeat(32), "is_default":false,
        "source":"channel", "status":"untested"});
    let build = serde_json::from_value(raw.clone()).unwrap();
    let key = orch_api_core::harness_channels::channel_key(&build);
    orch_db::queries::resources::management::put_channel_build(
        &mut *world.conn().await,
        &key,
        &raw,
        world.now(),
    )
    .await
    .unwrap();
    orch_api_core::harness_checks::check_waiting(&world.app.app).await;
    let all = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(!all.iter().any(|build| build.version == "0.170.0"));
    let row = orch_db::queries::settings::row(&mut *world.conn().await, None, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.value["status"], "untested",
        "the background checker never claimed the untrusted executable"
    );
    let chosen = orch_api_contract::admin::HarnessBuild {
        is_default: true,
        ..build.clone()
    };
    let refused = world
        .app
        .call(
            orch_api_contract::admin::AdminHarnessSet,
            &[&chosen.kind, &chosen.version],
        )
        .token(&c.root.token)
        .body(&chosen)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::RequestInvalid,
        "a stale default action cannot grant old channel metadata checksum authority"
    );
    orch_api_core::harness_channels::keep(&mut *world.conn().await, &[build], world.now())
        .await
        .unwrap();
    let all = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(all.iter().any(|build| build.version == "0.170.0"));
}

#[tokio::test]
async fn importing_vendor_checksums_records_the_exact_list_and_refuses_nonadministrators() {
    let world = World::new().await;
    let c = cast(&world).await;
    let document = json!([{
        "kind":"codex", "version":"0.170.0", "os":"linux", "arch":"x86_64",
        "url":"https://github.com/openai/codex/releases/download/rust-v0.170.0/codex-package-x86_64-unknown-linux-musl.tar.gz",
        "sha256":"ab".repeat(32), "is_default":false, "source":"channel"
    }]).to_string();
    let body = HarnessChecksumsImportBody {
        document: document.clone(),
    };
    let refused = world
        .app
        .call(AdminHarnessChecksumsImport, &[])
        .token(&c.boss.token)
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 403);
    let reply = world
        .app
        .call(AdminHarnessChecksumsImport, &[])
        .token(&c.root.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(reply.count, 1);
    assert_eq!(
        reply.sha256,
        hex::encode(Sha256::digest(document.as_bytes()))
    );
    let all = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(
        all.iter()
            .any(|build| build.version == "0.170.0" && build.sha256 == "ab".repeat(32))
    );
    let lines = lines_of(&world, "system.settings_updated").await;
    assert!(lines.iter().any(|line| line.actor_id == Some(c.root.id())
        && line.details["sha256"] == reply.sha256
        && line.details["count"] == 1));
}

#[tokio::test]
async fn importing_a_thousand_checksums_among_a_million_settings_has_constant_statements() {
    use support::Statements;
    let world = World::new().await;
    let c = cast(&world).await;
    {
        let mut conn = world.conn().await;
        sqlx::query(
            "INSERT INTO settings (node_id,key,value)
             SELECT NULL,'synthetic_scale:'||g,jsonb_build_object('sample',g)
             FROM generate_series(1,1000000) AS g",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("ANALYZE settings")
            .execute(&mut *conn)
            .await
            .unwrap();
        let stored: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM settings WHERE starts_with(key,'synthetic_scale:')",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(stored, 1_000_000);
    }
    async fn import(world: &World, token: &str, minor: u32, count: usize) -> usize {
        let builds: Vec<_> = (1..=count)
            .map(|patch| {
                json!({"kind":"codex","version":format!("0.{minor}.{patch}"),
                    "os":"linux","arch":"x86_64","url":"https://vendor.example.test/build",
                    "sha256":"ab".repeat(32),"is_default":true,"source":"mirror"})
            })
            .collect();
        let body = HarnessChecksumsImportBody {
            document: serde_json::to_string(&builds).unwrap(),
        };
        assert!(
            serde_json::to_vec(&body).unwrap().len() < orch_api_contract::common::MAX_JSON_BYTES
        );
        let call = world
            .app
            .call(AdminHarnessChecksumsImport, &[])
            .token(token)
            .body(&body);
        let before = Statements::now();
        let imported = call.send().await.ok();
        let statements = Statements::now() - before;
        assert_eq!(imported.count, count as i64);
        assert_eq!(
            imported.sha256,
            hex::encode(Sha256::digest(body.document.as_bytes()))
        );
        statements
    }
    let few = import(&world, &c.root.token, 180, 2).await;
    let many = import(&world, &c.root.token, 181, 1_000).await;
    assert!(few > 0, "the SQL observer must see actual statements");
    assert_eq!(
        few, many,
        "one checksum list must not execute one statement per build"
    );
    let mut conn = world.conn().await;
    let (count, defaults, trusted): (i64, i64, bool) = sqlx::query_as(
        "SELECT count(*),count(*) FILTER (WHERE value @> '{\"is_default\":true}'),
            bool_and(value @> '{\"checksum_origin\":\"vendor\",\"source\":\"admin\",\"status\":\"untested\"}')
         FROM settings WHERE node_id IS NULL AND
             (starts_with(key,'harness_build:codex:0.180.') OR starts_with(key,'harness_build:codex:0.181.'))",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!((count, defaults, trusted), (1_002, 1, true));
    let chosen: String = sqlx::query_scalar(
        "SELECT value->>'version' FROM settings WHERE node_id IS NULL AND
            key='harness_build:codex:0.181.1000:linux:x86_64' AND value @> '{\"is_default\":true}'",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(chosen, "0.181.1000");
    drop(conn);
    let lines = lines_of(&world, "system.settings_updated").await;
    assert!(
        lines
            .iter()
            .any(|line| line.actor_id == Some(c.root.id()) && line.details["count"] == 1_000)
    );
}

#[tokio::test]
async fn a_bad_checksum_list_is_refused_whole_before_it_can_change_the_builds() {
    let world = World::new().await;
    let c = cast(&world).await;
    let document = json!([{
        "kind":"codex", "version":"0.170.0", "os":"linux", "arch":"x86_64",
        "url":"https://example.test/build", "sha256":"not a checksum", "is_default":false, "source":"channel"
    }]).to_string();
    let refused = world
        .app
        .call(AdminHarnessChecksumsImport, &[])
        .token(&c.root.token)
        .body(&HarnessChecksumsImportBody { document })
        .send()
        .await
        .problem();
    assert_eq!(refused.status, 400);
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    let all = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(!all.iter().any(|build| build.version == "0.170.0"));
}

#[tokio::test]
async fn an_old_default_promoted_from_unknown_metadata_does_not_gain_admin_checksum_authority() {
    let world = World::new().await;
    let c = cast(&world).await;
    let raw = json!({"kind":"codex", "version":"0.171.0", "os":"linux", "arch":"x86_64",
        "url":"https://mirror.test/build", "sha256":"ab".repeat(32), "is_default":true,
        "source":"admin", "status":"untested"});
    let key = format!(
        "{}codex:0.171.0:linux:x86_64",
        orch_db::queries::resources::management::HARNESS_BUILD_PREFIX
    );
    orch_db::queries::resources::management::put_harness_build(
        &mut *world.conn().await,
        &key,
        &raw,
        c.root.id(),
        world.now(),
    )
    .await
    .unwrap();
    orch_api_core::harness_checks::check_waiting(&world.app.app).await;
    let listed = world
        .app
        .call(AdminHarnessList, &[])
        .token(&c.root.token)
        .send()
        .await
        .ok();
    assert!(!listed.iter().any(|build| build.version == "0.171.0"));
    let held = orch_db::queries::settings::row(&mut *world.conn().await, None, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        held.value["status"], "untested",
        "unknown old metadata was not executed"
    );
    let build: orch_api_contract::admin::HarnessBuild = serde_json::from_value(raw).unwrap();
    let listed = world
        .app
        .call(AdminHarnessSet, &[&build.kind, &build.version])
        .token(&c.root.token)
        .body(&build)
        .send()
        .await
        .ok();
    assert!(
        listed.iter().any(|build| build.version == "0.171.0"),
        "an administrator can explicitly reaffirm the expected checksum"
    );
}

#[tokio::test]
async fn a_channel_default_action_without_a_kept_vendor_anchor_is_refused() {
    let world = World::new().await;
    let c = cast(&world).await;
    let build: orch_api_contract::admin::HarnessBuild = serde_json::from_value(json!({
        "kind":"codex", "version":"0.172.0", "os":"linux", "arch":"x86_64", "url":"https://mirror.test/program", "sha256":"ef".repeat(32), "source":"channel", "status":"untested", "is_default":true
    })).unwrap();
    let denied = world
        .app
        .call(AdminHarnessSet, &[&build.kind, &build.version])
        .token(&c.root.token)
        .body(&build)
        .send()
        .await
        .problem();
    assert_eq!(
        denied.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn revoking_a_mirror_during_download_prevents_the_following_execution() {
    use orch_api_contract::admin::{AdminSettingSet, AdminSettingSetBody, HarnessBuild};
    use orch_types::Id;
    use std::time::Duration;
    let world = World::new().await;
    let c = cast(&world).await;
    let setting = "agents.harness_trust_mirror";
    world
        .app
        .call(AdminSettingSet, &[&setting])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(true),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let build: HarnessBuild = serde_json::from_value(json!({
        "kind":"codex", "version":"0.173.0", "os":"linux", "arch":"x86_64",
        "url":"https://mirror.test/program", "sha256":"ef".repeat(32),
        "source":"mirror", "status":"untested", "is_default":false
    }))
    .unwrap();
    let key = orch_api_core::harness_channels::channel_key(&build);
    let machine = {
        let mut conn = world.conn().await;
        orch_api_core::harness_channels::keep(&mut conn, std::slice::from_ref(&build), world.now())
            .await
            .unwrap();
        let host = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "host",
            None,
            None,
            "Checksum check host",
        )
        .await
        .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(host)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET online=true WHERE id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await
            .unwrap();
        machine
    };
    let app = world.app.app.clone();
    let stored_key = key.clone();
    let expected = build.sha256.clone();
    let running = tokio::spawn(async move {
        orch_api_core::harness_checks::finish(&app, &stored_key, &build)
            .await
            .unwrap();
    });
    let (command, body): (Id, serde_json::Value) =
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(command) = sqlx::query_as(
                    "SELECT id,body FROM commands WHERE machine_id=$1 AND kind='node.tool.ensure'",
                )
                .bind(machine)
                .fetch_optional(&world.db.pool)
                .await
                .unwrap()
                {
                    break command;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
    assert_eq!(
        body["sha256"], expected,
        "only the initially authorized checksum is downloaded"
    );
    world
        .app
        .call(AdminSettingSet, &[&setting])
        .token(&c.root.token)
        .body(&AdminSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE id=$1")
        .bind(command)
        .bind(json!({"path":"/tools/codex/0.173.0"}))
        .execute(&world.db.pool)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    let executed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE machine_id=$1 AND kind='node.tool.check'",
    )
    .bind(machine)
    .fetch_one(&world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        executed, 0,
        "revoked publisher bytes are never executed after the download finishes"
    );
    let held = orch_db::queries::settings::row(&mut *world.conn().await, None, &key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.value["status"], "untested");
    assert!(
        held.value["check"]["error"]
            .as_str()
            .unwrap()
            .contains("trusted checksum authority")
    );
}
