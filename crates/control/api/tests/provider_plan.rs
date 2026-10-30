//! Plan charts use measured window history, hide with the plan and project only the current window's pace.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use chrono::{Duration, Timelike};
use orch_api_contract::auth::{ApiTokenCreateBody, MeTokenCreate, SensitiveProof};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::resources::{ProviderGet, ProviderPlanGet, ProviderPlanGetQuery};
use orch_api_contract::usage::UsageBucket;
use orch_db::queries::resources::keeper::set_facts;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::World;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct ReadStatements(Arc<AtomicUsize>);
impl tracing::Subscriber for ReadStatements {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn provider_window_reads_have_fixed_statement_counts_and_pages_reject_a_changed_snapshot() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let now = world.now();
    let mut counts = Vec::new();
    for size in [10, 1000] {
        let facts = json!({"windows":(0..size).map(|index|json!({"id":format!("w{index:04}"),"label":"Weekly","duration_minutes":10080,"used_percent":20,"resets_at":now+Duration::days(6)})).collect::<Vec<_>>()});
        let mut conn = world.conn().await;
        set_facts(&mut conn, team.provider, &facts, now)
            .await
            .unwrap();
        drop(conn);
        let statements = ReadStatements::default();
        let provider = world
            .app
            .call(ProviderGet, &[&team.provider])
            .token(&team.boss.token)
            .send()
            .with_subscriber(statements.clone())
            .await
            .ok();
        assert_eq!(provider.facts.windows.len(), size);
        let count = statements.0.load(Ordering::Relaxed);
        assert!(
            count > 0 && count <= 124,
            "provider window read issued {count} statements"
        );
        eprintln!("provider windows={size}, statements={count}");
        counts.push(count);
    }
    assert_eq!(
        counts[0], counts[1],
        "provider read SQL must not grow with window rows"
    );
    let mut query = ProviderPlanGetQuery {
        from: (now - Duration::days(7)).format("%Y-%m-%d").to_string(),
        to: now.format("%Y-%m-%d").to_string(),
        bucket: Some(UsageBucket::Day),
        limit: Some(20),
        cursor: None,
    };
    let mut ids = std::collections::BTreeSet::new();
    let mut page_counts = Vec::new();
    for _ in 0..50 {
        let statements = ReadStatements::default();
        let page = world
            .app
            .call(ProviderPlanGet, &[&team.provider])
            .token(&team.boss.token)
            .query(&query)
            .send()
            .with_subscriber(statements.clone())
            .await
            .ok();
        assert_eq!(page.items.len(), 20);
        assert!(page.items.iter().all(|row| row.points.len() <= 600
            && row.history.len() <= 800
            && row.resets.len() <= 800));
        let count = statements.0.load(Ordering::Relaxed);
        assert!(
            count > 0 && count <= 32,
            "plan page issued {count} statements"
        );
        page_counts.push(count);
        for row in page.items {
            assert!(ids.insert(row.window_id));
        }
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 1000);
    assert_eq!(page_counts.len(), 50);
    assert!(page_counts.iter().all(|count| *count == page_counts[0]));
    eprintln!(
        "plan windows={}, pages={}, statements per page={}",
        ids.len(),
        page_counts.len(),
        page_counts[0]
    );
    query.cursor = None;
    let first = world
        .app
        .call(ProviderPlanGet, &[&team.provider])
        .token(&team.boss.token)
        .query(&query)
        .send()
        .await
        .ok();
    query.cursor = first.next_cursor;
    let mut conn = world.conn().await;
    set_facts(
        &mut conn,
        team.provider,
        &json!({"windows":[]}),
        now + Duration::minutes(1),
    )
    .await
    .unwrap();
    drop(conn);
    let changed = world
        .app
        .call(ProviderPlanGet, &[&team.provider])
        .token(&team.boss.token)
        .query(&query)
        .send()
        .await;
    assert_eq!(changed.status, http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_provider_read_projects_both_short_and_weekly_windows_and_one_read_supplies_both_chart_modes()
 {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let now = world
        .now()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();
    let mut conn = world.conn().await;
    for (at, short, weekly) in [
        (now - Duration::days(1), 0, 10),
        (now - Duration::hours(1), 20, 20),
        (now, 60, 20),
    ] {
        set_facts(&mut conn,team.provider,&json!({"windows":[
            {"id":"five","label":"5h","duration_minutes":300,"used_percent":short,"resets_at":now+Duration::hours(2)},
            {"id":"week","label":"Weekly","duration_minutes":10080,"used_percent":weekly,"resets_at":now+Duration::hours(120)}
        ]}),at).await.unwrap();
    }
    drop(conn);
    let provider = world
        .app
        .call(ProviderGet, &[&team.provider])
        .token(&team.boss.token)
        .send()
        .await
        .ok();
    let forecasts = provider.plan_forecasts.unwrap();
    assert_eq!(forecasts["five"].percent_at_end, 140.0);
    assert_eq!(
        forecasts["five"].runs_out_at.unwrap().get(),
        now + Duration::hours(1)
    );
    assert_eq!(forecasts["week"].percent_at_end, 70.0);
    let mut conn = world.conn().await;
    let first = now - Duration::minutes(2399);
    orch_db::migrations::ensure_metric_partition(&mut conn, first.date_naive())
        .await
        .unwrap();
    sqlx::query("INSERT INTO metric_samples(at,name,labels,value) SELECT at,'provider_plan_used_percent',$3,CASE WHEN at=$4 THEN 100 ELSE 20 END FROM generate_series($1::timestamptz,$2::timestamptz,interval '1 minute') AS points(at) ON CONFLICT(name,labels,at) DO UPDATE SET value=EXCLUDED.value")
        .bind(first).bind(now).bind(json!({"provider":team.provider,"window":"week"})).bind(first+Duration::minutes(1201)).execute(&mut *conn).await.unwrap();
    drop(conn);
    let series = world
        .app
        .call(ProviderPlanGet, &[&team.provider])
        .token(&team.boss.token)
        .query(&ProviderPlanGetQuery {
            from: (now - Duration::days(2)).format("%Y-%m-%d").to_string(),
            to: now.format("%Y-%m-%d").to_string(),
            bucket: Some(UsageBucket::Hour),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    let series = series.items;
    assert_eq!(series.len(), 2);
    assert!(series.iter().all(|window| window.points.len() <= 600));
    assert!(
        series
            .iter()
            .find(|window| window.window_id == "week")
            .unwrap()
            .points
            .iter()
            .any(|point| point.used_percent == 100.0)
    );
    assert!(series.iter().all(|window| !window.history.is_empty()));
    assert!(series.iter().all(|window| {
        window
            .history
            .iter()
            .all(|bucket| bucket.ends_at.get() - bucket.at.get() == Duration::hours(1))
    }));
    let mut changed = serde_json::to_value(&provider.facts).unwrap();
    changed["windows"][1]["used_percent"] = json!(0);
    let mut conn = world.conn().await;
    set_facts(&mut conn, team.provider, &changed, world.now())
        .await
        .unwrap();
    drop(conn);
    let past = world
        .app
        .call(ProviderPlanGet, &[&team.provider])
        .token(&team.boss.token)
        .query(&ProviderPlanGetQuery {
            from: (now - Duration::days(14)).format("%Y-%m-%d").to_string(),
            to: (now - Duration::days(7)).format("%Y-%m-%d").to_string(),
            bucket: Some(UsageBucket::Day),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    let past = past.items;
    assert!(past.iter().all(|series| series.history.is_empty()));
    assert!(
        !past
            .iter()
            .find(|series| series.window_id == "five")
            .unwrap()
            .points
            .is_empty(),
        "This window stays current when History covers a past span"
    );
    let week = past
        .iter()
        .find(|series| series.window_id == "week")
        .unwrap();
    assert!(week.resets.is_empty());
    assert_eq!(week.current_reset.as_ref().unwrap().count, 1);
    assert_eq!(
        week.current_reset.as_ref().unwrap().cause,
        orch_api_contract::resources::PlanResetCause::Vendor
    );
    let refreshed = world
        .app
        .call(ProviderGet, &[&team.provider])
        .token(&team.boss.token)
        .send()
        .await
        .ok();
    assert!(
        !refreshed.plan_forecasts.unwrap().contains_key("week"),
        "a start-over cannot borrow the previous window's pace"
    );
}

#[tokio::test]
async fn a_reader_without_provider_view_gets_neither_forecasts_nor_plan_points() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let credential = world
        .app
        .call(MeTokenCreate, &[])
        .token(&team.boss.token)
        .body(&ApiTokenCreateBody {
            name: "Plan reader".to_owned(),
            scopes: Some(vec!["provider.manage".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok()
        .token
        .unwrap();
    let single = world
        .app
        .call(ProviderGet, &[&team.provider])
        .token(&credential)
        .send()
        .await;
    assert_eq!(single.problem().code, ErrorCode::AuthzDenied);
    let points = world
        .app
        .call(ProviderPlanGet, &[&team.provider])
        .token(&credential)
        .query(&ProviderPlanGetQuery {
            from: world.now().format("%Y-%m-%d").to_string(),
            to: world.now().format("%Y-%m-%d").to_string(),
            bucket: None,
            limit: None,
            cursor: None,
        })
        .send()
        .await;
    assert_eq!(points.problem().code, ErrorCode::AuthzDenied);
}
