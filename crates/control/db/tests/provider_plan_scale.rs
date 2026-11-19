//! Plan reads aggregate a million-row store in SQL, and window snapshots write with a fixed query count.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{Duration, TimeZone, Utc};
use orch_db::queries::provider_plan::{self, PlanRead, WindowDefinition};
use orch_provider_contract::forecast::{
    FORECAST_LONG_LOOKBACK, FORECAST_MAX_AGE, ForecastPoint, ForecastSource, ReadingPace, forecast,
};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Statements(Arc<AtomicUsize>);
impl tracing::Subscriber for Statements {
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

fn indexes(value: &Json, found: &mut Vec<String>) {
    match value {
        Json::Object(object) => {
            if let Some(index) = object.get("Index Name").and_then(Json::as_str) {
                found.push(index.to_owned());
            }
            if let (Some(kind), Some(relation)) = (
                object.get("Node Type").and_then(Json::as_str),
                object.get("Relation Name").and_then(Json::as_str),
            ) {
                let scanned = (object
                    .get("Actual Rows")
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0)
                    + object
                        .get("Rows Removed by Filter")
                        .and_then(Json::as_f64)
                        .unwrap_or(0.0))
                    * object
                        .get("Actual Loops")
                        .and_then(Json::as_f64)
                        .unwrap_or(1.0);
                // Empty future partitions may choose a sequential path; the million-row partitions must not.
                assert!(
                    !(kind == "Seq Scan"
                        && scanned >= 100_000.0
                        && (relation.starts_with("metric_samples")
                            || relation.starts_with("metric_hours")
                            || relation == "audit_log")),
                    "unindexed large-table read: {object:?}"
                );
            }
            for child in object.values() {
                indexes(child, found);
            }
        }
        Json::Array(items) => {
            for child in items {
                indexes(child, found);
            }
        }
        _ => {}
    }
}

async fn explain(conn: &mut orch_db::Conn, read: &PlanRead<'_>, suffix: &str, name: &str) -> Json {
    let statement = format!(
        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {}{suffix}",
        include_str!("../src/queries/provider_plan_base.sql")
    );
    let plan: Json = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
        .bind(read.provider)
        .bind(serde_json::to_value(read.windows).unwrap())
        .bind(read.from)
        .bind(read.to)
        .bind(read.now)
        .bind(read.bucket)
        .bind(provider_plan::MAX_PLAN_POINTS)
        .bind(FORECAST_MAX_AGE.num_seconds())
        .bind(5.0_f64)
        .bind(60_i64)
        .bind(800_i64)
        .fetch_one(conn)
        .await
        .unwrap();
    if let Some(directory) = std::env::var_os("ORCH_SCALE_EXPLAIN_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{name}.json")),
            serde_json::to_vec_pretty(&plan).unwrap(),
        )
        .unwrap();
    }
    let mut used = Vec::new();
    indexes(&plan, &mut used);
    assert!(
        used.iter()
            .any(|index| index.contains("metric_samples") && index.ends_with("pkey")),
        "minute primary-key seek missing: {used:?}"
    );
    assert!(
        used.iter()
            .any(|index| index.contains("metric_hours") && index.ends_with("pkey")),
        "hour primary-key seek missing: {used:?}"
    );
    assert!(
        used.iter()
            .any(|index| index == "audit_provider_plan_window_idx"),
        "reset seek missing: {used:?}"
    );
    plan
}

async fn resource(conn: &mut orch_db::Conn, owner: Id, org: Id, name: &str) -> Id {
    orch_db::queries::resources::access_fixtures::insert_resource(
        conn,
        "provider",
        Some(owner),
        Some(org),
        name,
    )
    .await
    .unwrap()
}

async fn measured_chart(
    points: &[(chrono::DateTime<Utc>, f64)],
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    bucket: &str,
) -> provider_plan::ChartRow {
    let fixture = TestDb::fresh().await;
    let mut conn = fixture.pool.acquire().await.unwrap();
    let days: std::collections::BTreeSet<_> =
        points.iter().map(|(at, _)| at.date_naive()).collect();
    for day in days {
        orch_db::migrations::ensure_metric_partition(&mut conn, day)
            .await
            .unwrap();
    }
    let provider = new_id();
    let times: Vec<_> = points.iter().map(|(at, _)| *at).collect();
    let values: Vec<_> = points.iter().map(|(_, value)| *value).collect();
    sqlx::query("INSERT INTO metric_samples(at,name,labels,value)
        SELECT p.at,'provider_plan_used_percent',jsonb_build_object('provider',$1::uuid,'window','sample'),p.value
        FROM unnest($2::timestamptz[],$3::float8[]) AS p(at,value)")
        .bind(provider).bind(times).bind(values).execute(&mut *conn).await.unwrap();
    let windows = [WindowDefinition {
        window_id: "sample".to_owned(),
        natural_start: Some(Time::new(from)),
        lookback_seconds: 3600,
    }];
    provider_plan::chart(
        &mut conn,
        &PlanRead {
            provider,
            windows: &windows,
            from,
            to,
            now: to,
            bucket,
        },
    )
    .await
    .unwrap()
    .remove(0)
}

#[tokio::test]
async fn decimation_keeps_peaks_and_the_last_reading_within_six_hundred_points() {
    let from = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    let points: Vec<_> = (0..2400)
        .map(|minute| {
            (
                from + Duration::minutes(minute),
                if minute == 1201 { 100.0 } else { 20.0 },
            )
        })
        .collect();
    let row = measured_chart(&points, from, from + Duration::minutes(2400), "day").await;
    let selected: Vec<ForecastPoint> = serde_json::from_value(row.points).unwrap();
    assert!(selected.len() <= 600);
    assert!(selected.iter().any(|point| point.used_percent == 100.0));
    let last = points.last().unwrap();
    assert_eq!(
        selected.last().unwrap(),
        &ForecastPoint {
            at: Time::new(last.0),
            used_percent: last.1
        }
    );
}

#[tokio::test]
async fn history_counts_rises_and_never_subtracts_a_start_over() {
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    let points: Vec<_> = [80.0, 100.0, 0.0, 12.0, 25.0]
        .into_iter()
        .enumerate()
        .map(|(index, value)| (start + Duration::hours(index as i64), value))
        .collect();
    let row = measured_chart(&points, start, start + Duration::days(1), "day").await;
    let rows: Vec<orch_api_contract::resources::PlanUsageBucket> =
        serde_json::from_value(row.history).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].used_percent, 45.0);
    assert!(rows[0].reached_limit);
    assert_eq!(row.summary["mean_used_percent"], 45.0);
    assert_eq!(row.summary["limit_buckets"], 1);
}

#[tokio::test]
async fn an_hourly_history_retains_the_requested_hours_through_a_full_month() {
    let start = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap();
    let points: Vec<_> = (0..744)
        .map(|hour| (start + Duration::hours(hour), (hour % 5) as f64 * 10.0))
        .collect();
    let row = measured_chart(&points, start, start + Duration::days(31), "hour").await;
    let rows: Vec<orch_api_contract::resources::PlanUsageBucket> =
        serde_json::from_value(row.history).unwrap();
    assert_eq!(rows.len(), 744);
    assert!(
        rows.iter()
            .all(|row| row.ends_at.get() - row.at.get() == Duration::hours(1))
    );
}

#[tokio::test]
async fn million_measurements_and_audits_return_only_bounded_chart_and_pace_rows_and_bulk_writes_stay_fixed()
 {
    let fixture = TestDb::fresh().await;
    let mut conn = fixture.pool.acquire().await.unwrap();
    let end = Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap();
    let start = end - Duration::minutes(999);
    orch_db::migrations::ensure_partitions(&mut conn, end)
        .await
        .unwrap();
    orch_db::migrations::ensure_metric_partition(&mut conn, start.date_naive())
        .await
        .unwrap();
    let owner = new_id();
    let org = new_id();
    orch_db::queries::identity::testing::insert_user(&mut conn, owner, "scale-reader")
        .await
        .unwrap();
    orch_db::queries::hierarchy::testing::insert_node(&mut conn, org, "org", None, "Scale", owner)
        .await
        .unwrap();
    let provider = resource(&mut conn, owner, org, "Measured provider").await;
    let windows = [WindowDefinition {
        window_id: "week".to_owned(),
        natural_start: Some(Time::new(start)),
        lookback_seconds: FORECAST_LONG_LOOKBACK.num_seconds(),
    }];
    let read = PlanRead {
        provider,
        windows: &windows,
        from: end - Duration::days(70),
        to: end + Duration::seconds(1),
        now: end,
        bucket: "day",
    };
    let before = Statements::default();
    let empty = provider_plan::chart(&mut conn, &read)
        .with_subscriber(before.clone())
        .await
        .unwrap();
    assert_eq!(empty.len(), 1);
    assert_eq!(before.0.load(Ordering::Relaxed), 1);

    sqlx::query("INSERT INTO metric_samples(at,name,labels,value)
        SELECT $1::timestamptz+(g.n%1000)*interval '1 minute','provider_plan_used_percent',
          jsonb_build_object('provider',CASE WHEN g.n/1000=0 THEN $2::uuid ELSE md5('plan-provider-'||(g.n/1000)::text)::uuid END,'window','week'),
          CASE WHEN g.n%1000=501 THEN 100 ELSE (g.n%1000)%100 END
        FROM generate_series(0,999999) AS g(n)")
        .bind(start).bind(provider).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO metric_hours(at,name,labels,mean,peak,total,samples)
        SELECT $1::timestamptz+(g.n%1000)*interval '1 hour','provider_plan_used_percent',
          jsonb_build_object('provider',CASE WHEN g.n/1000=0 THEN $2::uuid ELSE md5('plan-provider-'||(g.n/1000)::text)::uuid END,'window','week'),
          20,CASE WHEN g.n%1000=501 THEN 100 ELSE 20 END,20,1 FROM generate_series(0,999999) AS g(n)")
        .bind(end-Duration::days(60)).bind(provider).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO audit_log(id,at,actor_kind,action,object_kind,object_id,summary,details)
        SELECT md5('plan-audit-'||g.n::text)::uuid,$1::timestamptz+(g.n%1000)*interval '1 hour','system','provider.updated','provider',
          md5('plan-provider-'||(g.n/1000)::text)::uuid,'A retained provider event','{}'::jsonb FROM generate_series(0,999999) AS g(n)")
        .bind(end-Duration::days(60)).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO audit_log(id,at,actor_kind,action,object_kind,object_id,summary,details,history)
        SELECT md5('plan-reset-'||g.n::text)::uuid,$1::timestamptz+(g.n%1000)*interval '1 hour','system','provider.window_started_over','provider',$2,
          'A window began again',jsonb_build_object('window_id','week','window_label','Weekly','before_percent',80,'after_percent',0,'cause','vendor'),true
        FROM generate_series(0,99999) AS g(n)")
        .bind(end-Duration::days(60)).bind(provider).execute(&mut *conn).await.unwrap();
    sqlx::raw_sql("ANALYZE metric_samples; ANALYZE metric_hours; ANALYZE audit_log;")
        .execute(&mut *conn)
        .await
        .unwrap();
    let reads = Statements::default();
    let mut charts = provider_plan::chart(&mut conn, &read)
        .with_subscriber(reads.clone())
        .await
        .unwrap();
    assert_eq!(
        reads.0.load(Ordering::Relaxed),
        before.0.load(Ordering::Relaxed)
    );
    assert_eq!(charts.len(), 1);
    let chart = charts.remove(0);
    let points: Vec<ForecastPoint> = serde_json::from_value(chart.points).unwrap();
    assert!(points.len() <= 600);
    assert!(points.iter().any(|point| point.used_percent == 100.0));
    assert_eq!(points.last().unwrap().at.get(), end);
    assert!(chart.history.as_array().unwrap().len() <= 800);
    assert!(chart.resets.as_array().unwrap().len() <= 800);
    assert_eq!(chart.summary["early_resets"], 100000);
    assert!(chart.current_reset.is_none());
    assert!(!chart.overflow);
    explain(
        &mut conn,
        &read,
        include_str!("../src/queries/provider_plan_chart.sql"),
        "plan-chart-million",
    )
    .await;

    let pace_read = PlanRead {
        from: start,
        ..read
    };
    let reads = Statements::default();
    let paces = provider_plan::paces(&mut conn, &pace_read)
        .with_subscriber(reads.clone())
        .await
        .unwrap();
    assert_eq!(reads.0.load(Ordering::Relaxed), 1);
    assert_eq!(paces.len(), 1);
    let pace = &paces[0];
    let actual = forecast(
        start,
        end + Duration::days(6),
        Duration::weeks(1),
        end,
        ForecastSource::Aggregate(ReadingPace {
            first_at: Time::new(pace.first_at.unwrap()),
            first_percent: pace.first_percent.unwrap(),
            last_at: Time::new(pace.last_at.unwrap()),
            last_percent: pace.last_percent.unwrap(),
            rise: pace.rise,
        }),
    );
    let oracle: Vec<_> = (0..1000)
        .map(|minute| ForecastPoint {
            at: Time::new(start + Duration::minutes(minute)),
            used_percent: if minute == 501 {
                100.0
            } else {
                (minute % 100) as f64
            },
        })
        .collect();
    assert_eq!(
        actual,
        forecast(
            start,
            end + Duration::days(6),
            Duration::weeks(1),
            end,
            ForecastSource::Readings(&oracle)
        )
    );
    explain(
        &mut conn,
        &pace_read,
        include_str!("../src/queries/provider_plan_pace.sql"),
        "plan-pace-million",
    )
    .await;

    let mut counts = Vec::new();
    for size in [10, 1000] {
        let id = resource(&mut conn, owner, org, "Window snapshot").await;
        let facts = |used, count| json!({"windows":(0..size).map(|index|json!({"id":format!("w{index:04}"),"label":"Weekly","duration_minutes":10080,"used_percent":used,"resets_at":end+Duration::days(7)})).collect::<Vec<_>>(),"available_resets":{"available_count":count}});
        let old = facts(80, 3);
        let new = facts(0, 2);
        assert!(
            serde_json::to_vec(&new).unwrap().len()
                <= orch_link_contract::version::MAX_FRAME_BYTES as usize
        );
        orch_db::queries::resources::keeper::set_facts(
            &mut conn,
            id,
            &old,
            end - Duration::minutes(1),
        )
        .await
        .unwrap();
        let statements = Statements::default();
        orch_db::queries::resources::keeper::set_facts(&mut conn, id, &new, end)
            .with_subscriber(statements.clone())
            .await
            .unwrap();
        let count = statements.0.load(Ordering::Relaxed);
        assert!(
            count > 0 && count <= 24,
            "unbounded snapshot write: {count}"
        );
        counts.push(count);
        eprintln!("window snapshot size={size}, statements={count}");
        let samples:i64=sqlx::query_scalar("SELECT count(*) FROM metric_samples WHERE name='provider_plan_used_percent' AND labels->>'provider'=$1 AND at=$2")
            .bind(id.to_string()).bind(end).fetch_one(&mut *conn).await.unwrap();
        let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='provider.window_started_over'")
            .bind(id).fetch_one(&mut *conn).await.unwrap();
        assert_eq!(samples, size);
        assert_eq!(audits, size);
    }
    assert_eq!(
        counts[0], counts[1],
        "SQL count must not grow with the source's window count"
    );
}
