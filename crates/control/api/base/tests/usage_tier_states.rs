//! Actual-tier reporting remains distinct from historical ordinary NULL through HTTP charts and drilldowns.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod places;
mod resources;
mod support;
mod usage_world;

use orch_api_contract::usage::{UsageEventList, UsageGet};
use serde_json::{Value, json};
use support::World;
use usage_world::{FROM, TODAY, Team, Use, spend, team};

async fn mixed(world: &World, team: &Team) {
    for (tokens, tier, states) in [
        (1, None, json!({})),
        (
            2,
            None,
            json!({"requested_service_tier":"auto","reported_service_tier":{"state":"empty"}}),
        ),
        (
            4,
            None,
            json!({"requested_service_tier":"priority","reported_service_tier":{"state":"unsupported"}}),
        ),
        (
            8,
            Some("priority"),
            json!({"requested_service_tier":"priority","reported_service_tier":{"state":"told","value":"priority"}}),
        ),
    ] {
        let turn = spend(world, team, &Use::of(world, team.boss.id(), tokens)).await;
        let mut conn = world.conn().await;
        let changed =
            sqlx::query("UPDATE usage_events SET tier=$3,measurement_states=$2 WHERE turn_id=$1")
                .bind(turn)
                .bind(states)
                .bind(tier)
                .execute(&mut *conn)
                .await
                .unwrap();
        assert_eq!(changed.rows_affected(), 1);
    }
    let mut tx = world.app.app.db.begin().await.unwrap();
    orch_db::usage::rollup::rebuild(&mut tx, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

fn query(extra: &str) -> String {
    format!("scope=me&from={FROM}&to={TODAY}{extra}")
}

#[tokio::test]
async fn tier_chart_keys_keep_omitted_actual_tier_separate_without_dropping_total_usage() {
    let world = World::new().await;
    let team = team(&world).await;
    mixed(&world, &team).await;
    let reply = world
        .app
        .call(UsageGet, &[])
        .token(&team.boss.token)
        .raw_query(&query("&breakdowns=tier&series_by=tier&measure=tokens"))
        .send_bytes()
        .await;
    assert_eq!(reply.status, http::StatusCode::OK);
    let body: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        body["totals"]["tokens"]["total_tokens"], 15,
        "unsupported tier capability still contributes usage totals"
    );
    let rows = body["breakdowns"][0]["rows"].as_array().unwrap();
    let mut groups: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row["key"]["tier"].as_str().map(str::to_owned),
                row["key"]["tier_unknown"].as_bool().unwrap_or(false),
                row["figures"]["tokens"]["total_tokens"].as_i64().unwrap(),
            )
        })
        .collect();
    groups.sort();
    assert_eq!(
        groups,
        vec![
            (None, false, 1),
            (None, true, 2),
            (Some("priority".into()), false, 8)
        ]
    );
    let points = body["series"].as_array().unwrap();
    let unknown: i64 = points
        .iter()
        .flat_map(|point| point["by"].as_array().unwrap())
        .filter(|split| split["key"]["tier_unknown"] == true)
        .map(|split| split["figures"]["tokens"]["total_tokens"].as_i64().unwrap())
        .sum();
    assert_eq!(
        unknown, 2,
        "series keys carry the same discriminator as table keys"
    );
}

#[tokio::test]
async fn actual_unreported_tier_has_its_own_exact_figures_and_event_filter() {
    let world = World::new().await;
    let team = team(&world).await;
    mixed(&world, &team).await;
    let asked = query("&tier_unknown=true");
    let reply = world
        .app
        .call(UsageGet, &[])
        .token(&team.boss.token)
        .raw_query(&asked)
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        http::StatusCode::OK,
        "the actual-unreported filter uses a boolean, never a fake tier value"
    );
    let body: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(body["totals"]["tokens"]["total_tokens"], 2);
    let reply = world
        .app
        .call(UsageEventList, &[])
        .token(&team.boss.token)
        .raw_query(&asked)
        .send_bytes()
        .await;
    assert_eq!(reply.status, http::StatusCode::OK);
    let body: Value = serde_json::from_slice(&reply.raw).unwrap();
    let rows = body["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["tier"], Value::Null);
    assert_eq!(rows[0]["tier_unknown"], true);
    assert_eq!(rows[0]["tokens"]["total_tokens"], 2);
    let reply = world
        .app
        .call(UsageGet, &[])
        .token(&team.boss.token)
        .raw_query(&query("&tier=none"))
        .send_bytes()
        .await;
    assert_eq!(reply.status, http::StatusCode::OK);
    let body: Value = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(
        body["totals"]["tokens"]["total_tokens"], 1,
        "ordinary drilldown never borrows unknown or unsupported requests"
    );
}
