//! Provider managers choose registered detail sources with write-only secrets, current policy and revision, and see each member's state.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::{
    common::ErrorCode,
    resources::{MemberState, Provider, ProviderGet, ProviderSourcesSet, SourceBy},
};
use orch_api_core::routes::resources::sealing::install_master_key;
use orch_db::secrets::MasterKey;
use orch_types::{Clock, Json};
use rig::Scene;
use serde_json::json;

async fn scene() -> Scene {
    install_master_key(MasterKey::new([7; 32]));
    let scene = Scene::new().await;
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(scene.on.provider)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    scene
}

async fn shown(s: &Scene) -> Provider {
    s.rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_providers_manager_gets_only_its_keepers_programs_without_needing_creation_or_machine_ownership()
 {
    use orch_api_contract::resources::{ProviderSetupGet, ProviderSetupGetQuery};
    let s = scene().await;
    let query = ProviderSetupGetQuery {
        org_id: s.tree.org.base.id,
        provider_id: Some(s.on.provider),
    };
    let denied = s
        .rig
        .world
        .app
        .call(ProviderSetupGet, &[])
        .token(&s.bea.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    s.rig
        .lets_use(
            "provider",
            s.on.provider,
            "provider.manage",
            &s.bea,
            json!({}),
        )
        .await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    let unrelated = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut conn,
        s.bea.id(),
        "Unrelated machine",
    )
    .await
    .unwrap();
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "keeper-plan",
        "Keeper plan",
        "Reports this keeper's plan.",
    );
    descriptor.facts = Some(Default::default());
    for (id, name) in [
        (s.rig.machine_id, "keeper-plan"),
        (unrelated, "unrelated-plan"),
    ] {
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{plugins}',$2) WHERE id=$1")
            .bind(id)
            .bind(json!([{"kind":"provider","name":name,"descriptor":descriptor}]))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    drop(conn);
    let page = s
        .rig
        .world
        .app
        .call(ProviderSetupGet, &[])
        .token(&s.bea.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(page.sources.iter().any(|source| {
        source.name == "keeper-plan"
            && source
                .machine
                .as_ref()
                .is_some_and(|machine| machine.id == s.rig.machine_id)
    }));
    assert!(
        !page
            .sources
            .iter()
            .any(|source| source.name == "unrelated-plan")
    );
    assert_eq!(
        page.key_machines
            .iter()
            .map(|machine| machine.id)
            .collect::<Vec<_>>(),
        [s.rig.machine_id]
    );
    let denied = s
        .rig
        .world
        .app
        .call(ProviderSetupGet, &[])
        .token(&s.bea.token)
        .query(&ProviderSetupGetQuery {
            org_id: query.org_id,
            provider_id: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    let wrong = s
        .rig
        .world
        .app
        .call(ProviderSetupGet, &[])
        .token(&s.bea.token)
        .query(&ProviderSetupGetQuery {
            org_id: orch_types::new_id(),
            provider_id: query.provider_id,
        })
        .send()
        .await
        .problem();
    assert_eq!(wrong.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn selecting_sources_requires_management_and_refuses_http_sign_in_key_opt_in_and_another_machines_program()
 {
    let s = scene().await;
    let provider = shown(&s).await;
    let none = json!({"expected_revision":provider.revision,"facts":{"by":"none"}});
    let denied = s
        .rig
        .world
        .app
        .call(ProviderSourcesSet, &[&s.on.provider])
        .token(&s.bea.token)
        .raw_body(&none)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    for choice in [
        json!({"by":"address","address":{"url":"http://plan.internal/orch"}}),
        json!({"by":"address","address":{"url":"https://plan.example.invalid/orch","send_credential":true}}),
        json!({"by":"program","name":"on-another-machine"}),
        json!({"by":"address","address":{"url":"https://plan.example.invalid/orch"},"config":{"token":{"$secret":s.on.provider}}}),
    ] {
        let denied = s
            .rig
            .world
            .app
            .call(ProviderSourcesSet, &[&s.on.provider])
            .token(&s.ada.token)
            .raw_body(&json!({"expected_revision":provider.revision,"facts":choice}))
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::RequestInvalid);
    }
    let (sources, revision): (Json, i64) =
        sqlx::query_as("SELECT sources,revision FROM providers WHERE id=$1")
            .bind(s.on.provider)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(sources, json!({}));
    assert_eq!(revision, provider.revision);
    let changed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='provider.sources_changed'",
    )
    .bind(s.on.provider)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        changed, 0,
        "a refused choice makes no change or audit claim"
    );
    s.rig
        .world
        .set_at(
            orch_settings::Target::Node(s.tree.org.base.id),
            "providers.allow_http_upstreams",
            json!(true),
        )
        .await;
    let allowed=s.rig.world.app.call(ProviderSourcesSet,&[&s.on.provider]).token(&s.ada.token)
        .raw_body(&json!({"expected_revision":provider.revision,"facts":{"by":"address","address":{"url":"http://plan.internal/orch"}}})).send().await.ok();
    assert_eq!(
        allowed.sources.facts.address.unwrap().url,
        "http://plan.internal/orch"
    );
    let audit:Json=sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1 AND action='provider.sources_changed' ORDER BY at DESC,id DESC LIMIT 1").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(audit["unencrypted"], true);
}

#[tokio::test]
async fn saving_the_same_source_cannot_reset_its_minimum_read_interval() {
    use orch_link_contract::commands::{CommandKind, CommandResult, ProviderRefreshResult};
    use std::sync::{Arc, Mutex};
    let s = scene().await;
    let at = s.rig.world.app.clock.time();
    let mut descriptor = orch_provider_contract::SourceDescriptor::new(
        "plan",
        "Plan details",
        "Reads no more than every fifteen minutes.",
    );
    descriptor.facts = Some(orch_provider_contract::source::SourceDescriptorFacts {
        min_interval_secs: Some(900),
        ..Default::default()
    });
    sqlx::query("UPDATE providers SET status='ready',sources=$2,facts_requested_at=$3 WHERE id=$1")
        .bind(s.on.provider)
        .bind(json!({"facts":{"by":"program","name":"plan","descriptor":descriptor}}))
        .bind(at.get())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{plugins}',$2) WHERE id=$1")
        .bind(s.rig.machine_id)
        .bind(json!([{"kind":"provider","name":"plan"}]))
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let count: Arc<Mutex<usize>> = Arc::default();
    let called = count.clone();
    s.rig
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, move |_| {
            *called.lock().unwrap() += 1;
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "not_told".into(),
                facts: None,
            }))
        });
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let saved = s
        .rig
        .world
        .app
        .call(ProviderSourcesSet, &[&s.on.provider])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":revision,"facts":{"by":"program","name":"plan"}}))
        .send()
        .await
        .ok();
    assert_eq!(
        *count.lock().unwrap(),
        0,
        "saving the same choice does not buy a second read inside its minimum"
    );
    assert_eq!(saved.facts_requested_at, Some(at));
}

#[tokio::test]
async fn address_authorization_is_sealed_never_echoed_and_is_not_reused_for_a_different_address() {
    const TOKEN: &str = "source-token-never-public";
    let s = scene().await;
    let old = shown(&s).await;
    let reply=s.rig.world.app.call(ProviderSourcesSet,&[&s.on.provider]).token(&s.ada.token)
        .raw_body(&json!({"expected_revision":old.revision,"facts":{"by":"address","address":{"url":"https://plan.example.invalid/orch","auth":"bearer"},"authorization":TOKEN}})).send().await;
    assert!(!String::from_utf8_lossy(&reply.raw).contains(TOKEN));
    let saved = reply.ok();
    assert_eq!(saved.sources.facts.by, SourceBy::Address);
    assert_eq!(saved.revision, old.revision + 1);
    let (sources,sealed):(Json,Vec<u8>)=sqlx::query_as("SELECT p.sources,s.sealed FROM providers p JOIN secrets s ON s.owner_kind='provider' AND s.owner_id=p.id AND s.name='source:facts:authorization' WHERE p.id=$1").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert!(!sources.to_string().contains(TOKEN));
    assert!(
        !sealed
            .windows(TOKEN.len())
            .any(|part| part == TOKEN.as_bytes())
    );
    let audit:Json=sqlx::query_scalar("SELECT details FROM audit_log WHERE object_id=$1 AND action='provider.sources_changed' ORDER BY at DESC,id DESC LIMIT 1").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(audit["host"], "plan.example.invalid");
    assert_eq!(audit["key_sent"], false);
    assert!(!audit.to_string().contains(TOKEN));
    let stale = s
        .rig
        .world
        .app
        .call(ProviderSourcesSet, &[&s.on.provider])
        .token(&s.ada.token)
        .raw_body(&json!({"expected_revision":old.revision,"facts":{"by":"none"}}))
        .send()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    let same=s.rig.world.app.call(ProviderSourcesSet,&[&s.on.provider]).token(&s.ada.token)
        .raw_body(&json!({"expected_revision":saved.revision,"facts":{"by":"address","address":{"url":"https://plan.example.invalid/orch","auth":"bearer"}}})).send().await.ok();
    let kept:i64=sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='source:facts:authorization'").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(kept, 1);
    s.rig.world.app.call(ProviderSourcesSet,&[&s.on.provider]).token(&s.ada.token)
        .raw_body(&json!({"expected_revision":same.revision,"facts":{"by":"address","address":{"url":"https://other.example.invalid/orch","auth":"none"}}})).send().await.ok();
    let kept:i64=sqlx::query_scalar("SELECT count(*) FROM secrets WHERE owner_kind='provider' AND owner_id=$1 AND name='source:facts:authorization'").bind(s.on.provider).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(kept, 0);
}

#[tokio::test]
async fn member_states_distinguish_unsupported_empty_and_told_and_public_facts_use_only_balances_and_spend()
 {
    let s = scene().await;
    sqlx::query("UPDATE providers SET sources=$2,facts=$3 WHERE id=$1").bind(s.on.provider)
        .bind(json!({"facts":{"by":"none"},"models":{"by":"none"}}))
        .bind(json!({"plan":null,"key_usage_supported":true,"key_usage":{"vendor":"Vendor","unit":"USD","used":12.5,"limit":50.0,"remaining":37.5,"used_daily":1.0,"used_weekly":2.0,"used_monthly":3.0},"field_times":{"plan":"2026-10-14T10:00:00.000Z","key_usage":"2026-10-14T10:00:00.000Z"}}))
        .execute(s.rig.db.pool()).await.unwrap();
    let reply = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await;
    let json: Json = serde_json::from_slice(&reply.raw).unwrap();
    let provider = reply.ok();
    assert_eq!(provider.facts_state.plan, MemberState::Empty);
    assert_eq!(provider.facts_state.spend, MemberState::Told);
    assert_eq!(provider.facts_state.windows, MemberState::Unsupported);
    assert_eq!(provider.facts_state.credential, MemberState::NotReadYet);
    assert_eq!(provider.facts_state.models, MemberState::Unsupported);
    assert_eq!(json["facts"]["spend"][0]["used"], "12.5");
    for legacy in [
        "plan_credits",
        "key_credits",
        "key_usage",
        "key_usage_supported",
    ] {
        assert!(
            json["facts"].get(legacy).is_none(),
            "{legacy} is an internal normalization input"
        );
    }
}
