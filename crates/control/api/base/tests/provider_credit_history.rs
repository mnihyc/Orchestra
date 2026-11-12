//! Exact reported credit history shares the existing provider reader and its current key bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use chrono::Duration;
use orch_api_contract::common::{ErrorCode, Page};
use orch_api_contract::resources::{
    ProviderCreditQuantity, ProviderHistoryMetric, ProviderHistorySeries, ProviderPlanGet,
    ProviderPlanGetQuery,
};
use orch_api_core::testing::Reply;
use orch_db::queries::resources::keeper::set_facts;
use orch_types::{Id, new_id};
use serde_json::{Value, json};
use support::World;

fn rule(operation: &str, provider: Id, action: &str) -> Value {
    json!({"target":{"kind":"provider","id":provider,"reach":"self"},
        "allow":[{"operation":operation,"actions":[action]}]})
}

async fn key(world: &World, user: Id, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let token = new_token(API_TOKEN_PREFIX);
    create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Credit history observer",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    token
}

fn query(world: &World, metric: Option<ProviderHistoryMetric>) -> ProviderPlanGetQuery {
    ProviderPlanGetQuery {
        metric,
        from: (world.now() - Duration::days(2)).date_naive().to_string(),
        to: world.now().date_naive().to_string(),
        bucket: Some(orch_api_contract::usage::UsageBucket::Hour),
        limit: Some(1),
        cursor: None,
    }
}

async fn read(
    world: &World,
    provider: Id,
    token: &str,
    query: &ProviderPlanGetQuery,
) -> Reply<Page<ProviderHistorySeries>> {
    world
        .app
        .call(ProviderPlanGet, &[&provider])
        .token(token)
        .query(query)
        .send()
        .await
}

#[tokio::test]
async fn exact_credit_history_uses_the_same_provider_read_and_leaves_subscription_json_unchanged() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let at = world.now() - Duration::hours(1);
    let exact = "9007199254740993.12345678901234567890";
    let facts = json!({
        "windows":[{"id":"five","label":"5h","used_percent":12.5,"duration_minutes":300,
            "resets_at":world.now()+Duration::hours(1)}],
        "balances":[{"id":"plan","unit":"vendor credits","remaining":exact,"unlimited":false,
            "total":null,"used":null}],
        "field_times":{"windows":at,"balances.plan":at}
    });
    set_facts(&mut *world.conn().await, team.provider, &facts, world.now())
        .await
        .unwrap();
    let subscription = read(
        &world,
        team.provider,
        &team.boss.token,
        &query(&world, None),
    )
    .await;
    let wire: Value = serde_json::from_slice(&subscription.raw).unwrap();
    assert_eq!(wire["items"][0]["window_id"], "five");
    assert!(
        wire["items"][0].get("kind").is_none(),
        "legacy subscription shape has no new discriminator"
    );
    assert!(matches!(
        subscription.ok().items[0],
        ProviderHistorySeries::Subscription(_)
    ));
    let credits = query(&world, Some(ProviderHistoryMetric::Credits));
    let ordinary = read(&world, team.provider, &team.boss.token, &credits)
        .await
        .ok();
    assert_eq!(
        ordinary.items.len(),
        1,
        "unsupported total/used nulls do not create series"
    );
    let ProviderHistorySeries::Credits(series) = &ordinary.items[0] else {
        panic!("credit shape")
    };
    assert_eq!(series.quantity, ProviderCreditQuantity::Remaining);
    assert_eq!(series.points[0].amount.as_deref(), Some(exact));
    assert_eq!(series.points[0].at.get(), at);
    assert_eq!(series.unit.as_deref(), Some("vendor credits"));
    let token = key(
        &world,
        team.boss.id(),
        vec![rule("ProviderPlanGet", team.provider, "read")],
    )
    .await;
    let scoped = read(&world, team.provider, &token, &credits).await;
    let wire: Value = serde_json::from_slice(&scoped.raw).unwrap();
    assert!(wire["items"][0].get("source_revision").is_none());
    assert!(wire["items"][0].get("source_key").is_none());
    assert_eq!(scoped.ok(), ordinary);
    for selected in [
        rule("ProviderGet", team.provider, "read"),
        rule("ProviderPlanGet", new_id(), "read"),
        rule("ProviderPlanGet", team.provider, "write"),
    ] {
        let denied = key(&world, team.boss.id(), vec![selected]).await;
        assert_eq!(
            read(&world, team.provider, &denied, &credits)
                .await
                .problem()
                .code,
            ErrorCode::AuthzDenied
        );
    }
    assert_eq!(
        read(&world, team.provider, &team.stranger.token, &credits)
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn credit_pages_keep_related_quantities_together_and_reject_other_metric_or_changed_source_cursors()
 {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let at = world.now() - Duration::hours(1);
    let facts = json!({"spend":[
        {"id":"first","unit":"USD","period":"month","used":"0","limit":"10"},
        {"id":"second","unit":"USD","period":"month","used":"2.50","limit":"20"}],
        "field_times":{"spend":at}});
    set_facts(&mut *world.conn().await, team.provider, &facts, world.now())
        .await
        .unwrap();
    let mut request = query(&world, Some(ProviderHistoryMetric::Credits));
    let first = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    assert_eq!(
        first.items.len(),
        2,
        "limit1 counts complete groups, not individual quantities"
    );
    let group = match &first.items[0] {
        ProviderHistorySeries::Credits(row) => row.group_id.clone(),
        _ => panic!("credit"),
    };
    for row in &first.items {
        let ProviderHistorySeries::Credits(row) = row else {
            panic!("credit")
        };
        assert_eq!(row.group_id, group);
    }
    request.cursor = first.next_cursor.clone();
    assert!(request.cursor.is_some());
    let second = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    assert_eq!(second.items.len(), 2);
    assert!(second.next_cursor.is_none());
    assert!(
        second
            .items
            .iter()
            .all(|row| matches!(row,ProviderHistorySeries::Credits(row) if row.group_id!=group))
    );
    let mut wrong = request.clone();
    wrong.metric = None;
    assert_eq!(
        read(&world, team.provider, &team.boss.token, &wrong)
            .await
            .status,
        http::StatusCode::BAD_REQUEST
    );
    // A later accepted source reading changes the snapshot even while the keeper clock stands
    // still: facts_at is unchanged, but revision and the actual observed unit are different.
    let mut changed = facts.clone();
    changed["spend"][0]["unit"] = json!("GBP");
    changed["field_times"]["spend"] = json!(world.now() - Duration::minutes(30));
    set_facts(
        &mut *world.conn().await,
        team.provider,
        &changed,
        world.now(),
    )
    .await
    .unwrap();
    assert_eq!(
        read(&world, team.provider, &team.boss.token, &request)
            .await
            .status,
        http::StatusCode::BAD_REQUEST
    );
    let mut current = query(&world, Some(ProviderHistoryMetric::Credits));
    current.limit = Some(20);
    let reread = read(&world, team.provider, &team.boss.token, &current)
        .await
        .ok();
    assert!(reread.items.iter().any(
        |row| matches!(row,ProviderHistorySeries::Credits(row) if row.unit.as_deref()==Some("USD"))
    ));
    assert!(reread.items.iter().any(
        |row| matches!(row,ProviderHistorySeries::Credits(row) if row.unit.as_deref()==Some("GBP"))
    ));
    // The existing source-choice writer rotates only the actual facts source, not model scheduling.
    current.limit = Some(1);
    let page = read(&world, team.provider, &team.boss.token, &current)
        .await
        .ok();
    current.cursor = page.next_cursor;
    assert!(current.cursor.is_some());
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM providers WHERE id=$1")
        .bind(team.provider)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert!(
        orch_db::queries::provider_facts::set_sources(
            &mut *world.conn().await,
            team.provider,
            revision,
            &json!({"facts":{"by":"none"}}),
            true,
            true,
            world.now()
        )
        .await
        .unwrap()
    );
    assert_eq!(
        read(&world, team.provider, &team.boss.token, &current)
            .await
            .status,
        http::StatusCode::BAD_REQUEST
    );
}

/// The same canonicalize/merge boundary used when a correlated provider observation is accepted.
async fn accepted_observation(world: &World, provider: Id, incoming: Value) {
    let mut conn = world.conn().await;
    let known: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(provider)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let known = orch_engine::provider_fact_reads::normalize_observation(&known).unwrap();
    let incoming = orch_engine::provider_fact_reads::normalize_observation(&incoming).unwrap();
    let merged = orch_engine::provider_fact_reads::merge_observation(&known, incoming);
    set_facts(&mut conn, provider, &merged, world.now())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_whole_group_explicit_null_retains_its_gap_after_normalization_and_merge() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(3);
    let empty = first + Duration::hours(1);
    let resumed = empty + Duration::hours(1);
    for (at, amount) in [
        (first, Some("10.125")),
        (empty, None),
        (resumed, Some("11.250")),
    ] {
        accepted_observation(&world, team.provider, json!({
            "balances":[{"id":"plan","unit":"credits","remaining":amount}],
            "extra_usage":{"enabled":true,"used":amount,"limit":null,"used_percent":null,"unit":"USD","decimal_places":2,"disabled_reason":null,"limit_reached":null},
            "field_times":{"balances":at,"balances.plan":at,"extra_usage":at}
        })).await;
    }
    let mut request = query(&world, Some(ProviderHistoryMetric::Credits));
    request.limit = Some(20);
    let page = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    for quantity in [
        ProviderCreditQuantity::Remaining,
        ProviderCreditQuantity::Used,
    ] {
        let series = page
            .items
            .iter()
            .find_map(|series| match series {
                ProviderHistorySeries::Credits(series) if series.quantity == quantity => {
                    Some(series)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            series.points.len(),
            3,
            "the accepted explicit null must not disappear behind an older finer clock"
        );
        assert_eq!(
            series
                .points
                .iter()
                .map(|point| (point.at.get(), point.amount.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (first, Some("10.125")),
                (empty, None),
                (resumed, Some("11.250"))
            ]
        );
        assert!(series.points[1].gap_before);
    }
}

#[tokio::test]
async fn unrelated_items_and_enabled_only_events_keep_their_original_amount_clocks_through_the_real_merge()
 {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(4);
    let second = first + Duration::hours(1);
    let disabled = second + Duration::hours(1);
    accepted_observation(&world, team.provider, json!({
        "balances":[{"id":"older","unit":"credits","remaining":"10"},{"id":"newer","unit":"credits","remaining":"20"}],
        "extra_usage":{"enabled":true,"used":"1.25","limit":null,"used_percent":null,"unit":"USD","decimal_places":2,"disabled_reason":null,"limit_reached":null},
        "field_times":{"balances":first,"balances.older":first,"balances.newer":first,"extra_usage":first}
    })).await;
    accepted_observation(&world, team.provider, json!({
        "balances":[{"id":"older","unit":"credits","remaining":"10"},{"id":"newer","unit":"credits","remaining":"21"}],
        "field_times":{"balances":second,"balances.older":first,"balances.newer":second}
    })).await;
    accepted_observation(&world, team.provider, json!({
        "extra_usage":{"enabled":false,"used":null,"limit":null,"used_percent":null,"unit":null,"decimal_places":null,"disabled_reason":null,"limit_reached":null},
        "field_times":{"extra_usage.enabled":disabled}
    })).await;
    let mut request = query(&world, Some(ProviderHistoryMetric::Credits));
    request.limit = Some(20);
    let page = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    let series: Vec<_> = page
        .items
        .iter()
        .filter_map(|series| match series {
            ProviderHistorySeries::Credits(series) => Some(series),
            _ => None,
        })
        .collect();
    let older = series
        .iter()
        .find(|series| series.item_id == "older")
        .unwrap();
    assert_eq!(older.points.len(), 1);
    assert_eq!(older.points[0].at.get(), first);
    let newer = series
        .iter()
        .find(|series| series.item_id == "newer")
        .unwrap();
    assert_eq!(
        newer
            .points
            .iter()
            .map(|point| point.amount.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("20"), Some("21")]
    );
    let extra = series
        .iter()
        .find(|series| series.quantity == ProviderCreditQuantity::Used)
        .unwrap();
    assert_eq!(
        extra
            .points
            .iter()
            .map(|point| (point.at.get(), point.amount.as_deref(), point.enabled))
            .collect::<Vec<_>>(),
        vec![
            (first, Some("1.25"), Some(true)),
            (disabled, None, Some(false))
        ]
    );
}

async fn third_party_ids_keep_their_own_clocks(
    balance_id: &str,
    spend_id: &str,
    encoded_balance: Option<&str>,
    encoded_spend: Option<&str>,
) {
    use orch_api_contract::resources::ProviderCreditMember;
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(3);
    let changed = first + Duration::hours(1);
    let balance_alias = format!("{balance_id}.remaining");
    let spend_alias = format!("{spend_id}.used");
    let mut balances = vec![
        json!({"id":balance_id,"unit":"points","remaining":"10"}),
        json!({"id":balance_alias,"unit":"points","remaining":"20"}),
    ];
    let mut spend = vec![
        json!({"id":spend_id,"unit":"USD","period":"month","used":"1.25"}),
        json!({"id":spend_alias,"unit":"USD","period":"month","used":"2.50"}),
    ];
    if let Some(id) = encoded_balance {
        balances.push(json!({"id":id,"unit":"points","remaining":"30"}));
    }
    if let Some(id) = encoded_spend {
        spend.push(json!({"id":id,"unit":"USD","period":"month","used":"3.75"}));
    }
    // The ordinary source cleaner establishes that these identifiers are legal third-party input.
    let source = orch_connector_core::sources::reports::read(
        &json!({"balances":balances,"spend":spend}),
        orch_types::Time::new(first),
        false,
        &[],
    )
    .unwrap();
    assert!(source.notes.is_empty());
    assert_eq!(
        source.facts["balances"].as_array().unwrap().len(),
        if encoded_balance.is_some() { 3 } else { 2 }
    );
    assert_eq!(
        source.facts["spend"].as_array().unwrap().len(),
        if encoded_spend.is_some() { 3 } else { 2 }
    );
    accepted_observation(&world, team.provider, source.facts).await;
    // Existing item-only observation semantics retain the other items from the earlier read.
    let changed_times = serde_json::Map::from_iter([
        (format!("balances.{balance_id}"), json!(changed)),
        (format!("spend.{spend_id}"), json!(changed)),
    ]);
    accepted_observation(
        &world,
        team.provider,
        json!({
            "balances":[{"id":balance_id,"unit":"points","remaining":"11"}],
            "spend":[{"id":spend_id,"unit":"USD","period":"month","used":"1.50"}],
            "field_times":changed_times
        }),
    )
    .await;
    let mut request = query(&world, Some(ProviderHistoryMetric::Credits));
    request.limit = Some(20);
    let page = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    let mut expected = vec![
        (
            ProviderCreditMember::Balances,
            balance_id,
            vec![(first, "10"), (changed, "11")],
        ),
        (
            ProviderCreditMember::Balances,
            balance_alias.as_str(),
            vec![(first, "20")],
        ),
        (
            ProviderCreditMember::Spend,
            spend_id,
            vec![(first, "1.25"), (changed, "1.50")],
        ),
        (
            ProviderCreditMember::Spend,
            spend_alias.as_str(),
            vec![(first, "2.50")],
        ),
    ];
    if let Some(id) = encoded_balance {
        expected.push((ProviderCreditMember::Balances, id, vec![(first, "30")]));
    }
    if let Some(id) = encoded_spend {
        expected.push((ProviderCreditMember::Spend, id, vec![(first, "3.75")]));
    }
    for (member, id, points) in expected {
        let series = page
            .items
            .iter()
            .find_map(|row| match row {
                ProviderHistorySeries::Credits(row)
                    if row.member == member && row.item_id == id =>
                {
                    Some(row)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            series
                .points
                .iter()
                .map(|point| (point.at.get(), point.amount.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            points,
            "updating another legitimate id must not create a reading for {id}"
        );
    }
    let stored: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(team.provider)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    for key in [
        format!("balances.{balance_alias}"),
        format!("spend.{spend_alias}"),
    ] {
        assert_eq!(
            stored["field_times"][key]
                .as_str()
                .unwrap()
                .parse::<orch_types::Time>()
                .unwrap()
                .get(),
            first,
            "normalization must retain the ordinary item clock"
        );
    }
}

#[tokio::test]
async fn third_party_dotted_ids_do_not_alias_generated_quantity_clocks() {
    third_party_ids_keep_their_own_clocks("plan", "ledger", None, None).await;
}

#[tokio::test]
async fn third_party_slash_and_tilde_ids_remain_distinct_from_encoded_looking_ids() {
    third_party_ids_keep_their_own_clocks(
        "p/~.plan",
        "a/~.ledger",
        Some("p~1~0.plan"),
        Some("a~1~0.ledger"),
    )
    .await;
}

#[tokio::test]
async fn an_explicit_older_amount_clock_survives_a_label_only_whole_item_read() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(3);
    let label_only = first + Duration::hours(1);
    accepted_observation(&world,team.provider,json!({"balances":[{"id":"plan","label":"Original","unit":"points","remaining":"10"}],"field_times":{"balances":first,"balances.plan":first}})).await;
    accepted_observation(&world,team.provider,json!({"balances":[{"id":"plan","label":"Renamed","unit":"points","remaining":"10"}],"field_times":{"balances":label_only,"balances.plan":label_only,"/balances/plan/remaining":first}})).await;
    let page = read(
        &world,
        team.provider,
        &team.boss.token,
        &query(&world, Some(ProviderHistoryMetric::Credits)),
    )
    .await
    .ok();
    let ProviderHistorySeries::Credits(series) = &page.items[0] else {
        panic!("credit series")
    };
    assert_eq!(
        series.points.len(),
        1,
        "an explicitly older quantity observation must not acquire the label's newer timestamp"
    );
    assert_eq!(series.points[0].at.get(), first);
    assert_eq!(series.points[0].amount.as_deref(), Some("10"));
    let stored: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(team.provider)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(
        stored["balances"][0]["label"], "Renamed",
        "the actual item replacement was accepted"
    );
}

#[tokio::test]
async fn an_encoded_quantity_only_clock_is_fresh_without_changing_the_old_item_clock() {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(3);
    let changed = first + Duration::hours(1);
    accepted_observation(&world,team.provider,json!({"balances":[{"id":"plan/~.remaining","unit":"points","remaining":"10"}],"field_times":{"balances":first,"balances.plan/~.remaining":first}})).await;
    accepted_observation(&world,team.provider,json!({"balances":[{"id":"plan/~.remaining","unit":"points","remaining":"11"}],"field_times":{"/balances/plan~1~0.remaining/remaining":changed}})).await;
    let page = read(
        &world,
        team.provider,
        &team.boss.token,
        &query(&world, Some(ProviderHistoryMetric::Credits)),
    )
    .await
    .ok();
    let ProviderHistorySeries::Credits(series) = &page.items[0] else {
        panic!("credit series")
    };
    assert_eq!(series.item_id, "plan/~.remaining");
    assert_eq!(
        series
            .points
            .iter()
            .map(|point| (point.at.get(), point.amount.as_deref()))
            .collect::<Vec<_>>(),
        vec![(first, Some("10")), (changed, Some("11"))]
    );
    let stored: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(team.provider)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert_eq!(
        stored["field_times"]["balances.plan/~.remaining"]
            .as_str()
            .unwrap()
            .parse::<orch_types::Time>()
            .unwrap()
            .get(),
        first
    );
}

#[tokio::test]
async fn older_explicit_amount_observations_cannot_regress_current_values_or_resurrect_cleared_amounts()
 {
    let world = World::new().await;
    let team = usage_world::team(&world).await;
    let first = world.now() - Duration::hours(8);
    let older = first - Duration::hours(1);
    let label_only = first + Duration::hours(1);
    let fresh = first + Duration::hours(2);
    let cleared = first + Duration::hours(3);
    let stale_after_clear = first + Duration::hours(4);
    let report = |at: chrono::DateTime<chrono::Utc>,
                  fine: Option<chrono::DateTime<chrono::Utc>>,
                  amount: Option<&str>,
                  label: &str| {
        let mut times = serde_json::Map::from_iter(
            [
                "balances",
                "balances.plan",
                "spend",
                "spend.month",
                "extra_usage",
            ]
            .into_iter()
            .map(|key| (key.to_owned(), json!(at))),
        );
        if let Some(fine) = fine {
            for key in [
                "/balances/plan/remaining",
                "/spend/month/used",
                "extra_usage.used",
            ] {
                times.insert(key.to_owned(), json!(fine));
            }
        }
        json!({"balances":[{"id":"plan","label":label,"unit":"points","remaining":amount}],
            "spend":[{"id":"month","label":label,"unit":"USD","period":"month","used":amount}],
            "extra_usage":{"enabled":true,"used":amount,"limit":null,"used_percent":null,"unit":"USD","decimal_places":2,"disabled_reason":null,"limit_reached":null},
            "field_times":times})
    };
    let mut request = query(&world, Some(ProviderHistoryMetric::Credits));
    request.limit = Some(20);
    for (at, fine, incoming, label, kept, clock, amounts) in [
        (
            first,
            None,
            Some("10"),
            "Initial",
            Some("10"),
            first,
            vec![Some("10")],
        ),
        (
            label_only,
            Some(older),
            Some("2"),
            "Fresh label",
            Some("10"),
            first,
            vec![Some("10")],
        ),
        (
            fresh,
            Some(fresh),
            Some("12"),
            "Fresh amount",
            Some("12"),
            fresh,
            vec![Some("10"), Some("12")],
        ),
        (
            cleared,
            None,
            None,
            "Known null",
            None,
            cleared,
            vec![Some("10"), Some("12"), None],
        ),
        (
            stale_after_clear,
            Some(label_only),
            Some("5"),
            "New label, old amount",
            None,
            cleared,
            vec![Some("10"), Some("12"), None],
        ),
    ] {
        let mut observation = report(at, fine, incoming, label);
        if at == label_only {
            // Exercise the existing item-only list branch as well as later whole replacements.
            let times = observation["field_times"].as_object_mut().unwrap();
            times.remove("balances");
            times.remove("spend");
        }
        accepted_observation(&world, team.provider, observation).await;
        let stored: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
            .bind(team.provider)
            .fetch_one(&mut *world.conn().await)
            .await
            .unwrap();
        for value in [
            &stored["balances"][0]["remaining"],
            &stored["spend"][0]["used"],
            &stored["extra_usage"]["used"],
        ] {
            assert_eq!(
                value.as_str(),
                kept,
                "the accepted current amount must remain paired with its current clock: {label}"
            );
            if kept.is_none() {
                assert!(
                    value.is_null(),
                    "no zero/default may stand in for the known null"
                );
            }
        }
        assert_eq!(stored["balances"][0]["label"], label);
        assert_eq!(stored["spend"][0]["label"], label);
        for key in [
            "/balances/plan/remaining",
            "/spend/month/used",
            "extra_usage.used",
        ] {
            assert_eq!(
                stored["field_times"][key]
                    .as_str()
                    .unwrap()
                    .parse::<orch_types::Time>()
                    .unwrap()
                    .get(),
                clock
            );
        }
        let page = read(&world, team.provider, &team.boss.token, &request)
            .await
            .ok();
        assert_eq!(page.items.len(), 3);
        for row in &page.items {
            let ProviderHistorySeries::Credits(series) = row else {
                panic!("credit series")
            };
            assert_eq!(
                series
                    .points
                    .iter()
                    .map(|point| point.amount.as_deref())
                    .collect::<Vec<_>>(),
                amounts,
                "current facts and history must agree: {label}"
            );
            if clock == cleared {
                assert!(series.points.last().unwrap().gap_before);
            }
        }
    }
    // An absent old item has no required values to fabricate. Reject only stale optional money;
    // the fresh incoming required shape remains valid when the source reintroduces that item.
    let removed = first + Duration::hours(5);
    accepted_observation(&world,team.provider,json!({"balances":[],"spend":[],"extra_usage":null,"field_times":{"balances":removed,"spend":removed,"extra_usage":removed}})).await;
    let reintroduced = removed + Duration::hours(1);
    accepted_observation(
        &world,
        team.provider,
        report(
            reintroduced,
            Some(stale_after_clear),
            Some("99"),
            "Reintroduced",
        ),
    )
    .await;
    let stored: Value = sqlx::query_scalar("SELECT facts FROM providers WHERE id=$1")
        .bind(team.provider)
        .fetch_one(&mut *world.conn().await)
        .await
        .unwrap();
    assert!(stored["balances"][0].get("remaining").is_none());
    assert!(stored["spend"][0].get("used").is_none());
    assert!(stored["extra_usage"].get("used").is_none());
    let canonical = orch_engine::provider_fact_reads::normalize_observation(&stored).unwrap();
    assert_eq!(canonical["balances"][0]["unit"], "points");
    assert_eq!(canonical["spend"][0]["period"], "month");
    assert_eq!(canonical["extra_usage"]["enabled"], true);
    let page = read(&world, team.provider, &team.boss.token, &request)
        .await
        .ok();
    for row in &page.items {
        let ProviderHistorySeries::Credits(series) = row else {
            panic!("credit series")
        };
        assert_eq!(
            series
                .points
                .iter()
                .filter_map(|point| point.amount.as_deref())
                .collect::<Vec<_>>(),
            vec!["10", "12"]
        );
    }
}
