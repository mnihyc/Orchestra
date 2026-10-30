//! Usage over HTTP: a person reads their own use in full, a place's figures need its row and its records need `usage.view`, what ran on a provider is for whoever may view the provider; credits that are an estimate are marked, tokens without a rate are counted with none, and every figure can be followed to its records, as JSON or CSV.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use chrono::{Duration, TimeZone, Utc};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::usage::{
    RateSource, UsageAnalytics, UsageBreakdown, UsageBucket, UsageDepth, UsageDimension,
    UsageEvent, UsageEventList, UsageEventListQuery, UsageEventOrder, UsageGet, UsageGetQuery,
};
use places::{Who, missing_permissions};
use support::World;
use usage_world::{FROM, TODAY, Team, Use, another_thread, spend, spend_in, team};

fn query(scope: &str) -> UsageGetQuery {
    UsageGetQuery {
        health: None,
        turn_id: None,
        org_id: None,
        measure: None,
        effort: None,
        tier: None,
        program: None,
        outcome: None,
        scope: scope.to_owned(),
        from: FROM.to_owned(),
        to: TODAY.to_owned(),
        bucket: None,
        series_by: None,
        breakdowns: None,
        limit: None,
        user_id: None,
        model: None,
        provider_id: None,
        harness_kind: None,
        environment_kind: None,
        project_id: None,
        topic_id: None,
        thread_id: None,
    }
}

fn events_query(scope: &str) -> UsageEventListQuery {
    UsageEventListQuery {
        turn_id: None,
        org_id: None,
        effort: None,
        tier: None,
        program: None,
        outcome: None,
        include_running: None,
        requests_only: None,
        scope: scope.to_owned(),
        from: FROM.to_owned(),
        to: TODAY.to_owned(),
        user_id: None,
        model: None,
        provider_id: None,
        harness_kind: None,
        environment_kind: None,
        project_id: None,
        topic_id: None,
        thread_id: None,
        counted: None,
        order: None,
        limit: None,
        cursor: None,
    }
}

async fn measured_spend(
    world: &World,
    team: &Team,
    output: i64,
    generation: Option<i64>,
    first: Option<i64>,
    outcome: &str,
) {
    measured_spend_at(
        world,
        team,
        output,
        generation,
        first,
        outcome,
        world.now() - Duration::hours(1),
    )
    .await;
}

async fn measured_spend_at(
    world: &World,
    team: &Team,
    output: i64,
    generation: Option<i64>,
    first: Option<i64>,
    outcome: &str,
    at: chrono::DateTime<Utc>,
) {
    use orch_api_contract::usage::{RequestSample, UsageMeasurement};
    use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
    let attribution = Attribution {
        org_id: team.acme,
        project_id: team.project,
        topic_id: team.topic,
        thread_id: team.thread,
        segment_id: orch_types::new_id(),
        turn_id: orch_types::new_id(),
        provider_id: team.provider,
        entry_id: None,
        user_id: Some(team.boss.id()),
        agent_id: None,
        harness_kind: Some("codex".into()),
        environment_id: None,
        environment_kind: Some("managed".into()),
        arrangement: None,
        turn_model: "perf-model".into(),
        effort: Some("high".into()),
        params: serde_json::json!({"cyber_access_program":"research"}),
    };
    let first_at = first.map(|ms| orch_types::Time::new(at + Duration::milliseconds(ms)));
    let end = first_at
        .zip(generation)
        .map(|(first, ms)| orch_types::Time::new(first.get() + Duration::milliseconds(ms)));
    let timing = orch_api_contract::performance::request_timing(&RequestSample {
        asked: Some(at.into()),
        first_output: first_at,
        end,
        output_tokens: Some(output),
        output_streamed: generation.map(|_| true),
        outcome: UsageMeasurement::Told {
            value: outcome.into(),
        },
        error_message: (outcome == "provider_busy").then(|| "Rate limited".into()),
        vendor_words: (outcome == "provider_busy").then(|| "Too many requests".into()),
        ..Default::default()
    });
    let mut conn = world.conn().await;
    ledger::record_program(
        &mut conn,
        &NewEvent {
            id: orch_types::new_id(),
            at,
            origin: ledger::origin::HARNESS,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: &attribution,
            model: "perf-model",
            tier: Some("fast"),
            tokens: Tokens {
                input: 100,
                cached_input: 0,
                cache_write: 0,
                output,
                reasoning: 0,
                total: 100 + output,
            },
            context_tokens: Some(100),
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
        Some(&timing),
        Some("research"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn performance_percentiles_use_the_measured_buckets_and_errors_exclude_personal_cancellations()
 {
    let world = World::new().await;
    let team = team(&world).await;
    for (output, generation, first, outcome) in [
        (64, Some(2000), Some(0), "done"),
        (64, Some(1000), Some(512), "provider_busy"),
        (64, Some(500), Some(1024), "done"),
        (128, Some(500), Some(2048), "forced"),
        (10, None, None, "done"),
    ] {
        measured_spend(&world, &team, output, generation, first, outcome).await;
    }
    let mut asked = query(&format!("node:{}", team.project));
    asked.series_by = Some(UsageDimension::Program);
    asked.breakdowns = Some("model,program,effort,tier".into());
    asked.measure = Some(orch_api_contract::usage::UsageMeasure::Speed);
    let answer = figures(&world, &team.admin, &asked).await;
    let p = &answer.totals.performance;
    assert_eq!(p.requests, 5);
    assert_eq!(p.speed_samples, 4);
    assert_eq!(p.first_token_samples, 4);
    assert_eq!(
        (p.speed_milli, p.speed_p90_milli),
        (Some(61_287), Some(245_146))
    );
    assert_eq!(
        (p.first_token_ms, p.first_token_p90_ms),
        (Some(491), Some(1962))
    );
    assert_eq!(
        p.errors,
        std::collections::BTreeMap::from([("provider_busy".to_owned(), 1)])
    );
    assert_eq!(answer.performance_before.requests, 0);
    assert_eq!(answer.comparison.len(), 1);
    assert_eq!(answer.comparison[0].program.as_deref(), Some("research"));
    assert_eq!(answer.comparison[0].performance, *p);
    assert_eq!(
        answer
            .breakdowns
            .iter()
            .find(|table| table.by == UsageDimension::Program)
            .unwrap()
            .rows[0]
            .key
            .program,
        Some(Some("research".into()))
    );
    let mut records = events_query(&asked.scope);
    records.program = Some("research".into());
    records.outcome = Some(orch_api_contract::usage::UsageOutcomeFilter::ProviderBusy);
    let rows = events(&world, &team.admin, &records).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].timing.vendor_words.as_deref(),
        Some("Too many requests")
    );
    assert_eq!(
        rows[0].timing.outcome.value().map(String::as_str),
        Some("provider_busy")
    );
    records.outcome = Some(orch_api_contract::usage::UsageOutcomeFilter::Forced);
    assert_eq!(
        events(&world, &team.admin, &records).await.len(),
        1,
        "cancellations remain in the log"
    );
}

#[tokio::test]
async fn provider_error_figures_exclude_harness_environment_and_session_failures_on_ledger_and_rollups()
 {
    let world = World::new().await;
    let team = team(&world).await;
    for code in [
        "provider_busy",
        "model_unavailable",
        "harness_error",
        "environment_unreachable",
        "session_lost",
        "forced",
        "done",
    ] {
        measured_spend(&world, &team, 0, None, None, code).await;
    }
    for bucket in [UsageBucket::Day, UsageBucket::Hour] {
        let mut asked = query(&format!("node:{}", team.project));
        asked.bucket = Some(bucket);
        let p = figures(&world, &team.admin, &asked)
            .await
            .totals
            .performance;
        assert_eq!(p.requests, 7);
        assert_eq!(
            p.errors,
            std::collections::BTreeMap::from([
                ("provider_busy".into(), 1),
                ("model_unavailable".into(), 1)
            ])
        );
    }
    let mut asked = events_query(&format!("node:{}", team.project));
    asked.outcome = Some(orch_api_contract::usage::UsageOutcomeFilter::Errors);
    assert_eq!(
        events(&world, &team.admin, &asked).await.len(),
        5,
        "all failures remain available in the request log"
    );
}

#[tokio::test]
async fn lightweight_provider_health_has_no_analytics_payload_and_needs_ten_latency_samples() {
    use orch_api_contract::usage::{UsageHealthGet, UsageHealthGetQuery};
    let world = World::new().await;
    let team = team(&world).await;
    // An admin of the provider's home views it as one, with As admin on.
    places::admin_mode(&world, &team.admin, true).await;
    for _ in 0..20 {
        measured_spend(&world, &team, 64, Some(1000), Some(1000), "done").await;
    }
    for _ in 0..9 {
        measured_spend_at(
            &world,
            &team,
            64,
            Some(1000),
            Some(4000),
            "done",
            world.now() - Duration::minutes(1),
        )
        .await;
    }
    let query = UsageHealthGetQuery {
        provider_id: team.provider,
    };
    let first = world
        .app
        .call(UsageHealthGet, &[])
        .token(&team.admin.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(first.recent.first_token_samples, 9);
    assert!(!first.first_output);
    assert!(!first.errors);
    measured_spend_at(
        &world,
        &team,
        64,
        Some(1000),
        Some(4000),
        "done",
        world.now() - Duration::minutes(1),
    )
    .await;
    let health = world
        .app
        .call(UsageHealthGet, &[])
        .token(&team.admin.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(health.recent.requests, 10);
    assert!(health.first_output);
    assert_eq!(health.since.get(), world.now() - Duration::minutes(15));
    let serialized = serde_json::to_value(health).unwrap();
    for key in ["series", "comparison", "breakdowns", "performance_before"] {
        assert!(serialized.get(key).is_none());
    }
    let denied = world
        .app
        .call(UsageHealthGet, &[])
        .token(&team.stranger.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn a_summary_reader_can_follow_their_own_turn_records_but_not_another_persons() {
    let world = World::new().await;
    let team = team(&world).await;
    used(&world, &team).await;
    let scope = format!("node:{}", team.project);
    let own = events(&world, &team.viewer, &events_query(&scope)).await;
    let mut asked = events_query(&scope);
    asked.turn_id = own[0].turn_id;
    assert_eq!(events(&world, &team.viewer, &asked).await.len(), 1);
    let all = events(&world, &team.admin, &events_query(&scope)).await;
    asked.turn_id = all
        .iter()
        .find(|row| {
            row.user
                .as_ref()
                .is_some_and(|user| user.id != team.viewer.id())
        })
        .and_then(|row| row.turn_id);
    assert!(asked.turn_id.is_some());
    assert!(events(&world, &team.viewer, &asked).await.is_empty());
}

#[tokio::test]
async fn outcome_filters_on_the_real_routes_share_failures_missing_and_code_semantics() {
    use orch_api_contract::usage::UsageOutcomeFilter;
    let world = World::new().await;
    let team = team(&world).await;
    measured_spend(&world, &team, 64, Some(1000), Some(100), "provider_busy").await;
    measured_spend(&world, &team, 64, Some(1000), Some(100), "done").await;
    measured_spend(&world, &team, 0, None, None, "forced").await;
    let unknown = orch_types::new_id();
    {
        let mut conn = world.conn().await;
        sqlx::query("INSERT INTO usage_events(id,at,origin,purpose,scope,counted,org_id,project_id,topic_id,thread_id,provider_id,user_id,model) VALUES($1,$2,'harness','turn','request',true,$3,$4,$5,$6,$7,$8,'unmeasured')")
            .bind(unknown).bind(world.now()).bind(team.acme).bind(team.project).bind(team.topic).bind(team.thread).bind(team.provider).bind(team.boss.id()).execute(&mut *conn).await.unwrap();
    }
    let mut asked = events_query(&format!("node:{}", team.project));
    asked.outcome = Some(UsageOutcomeFilter::Errors);
    let failed = events(&world, &team.admin, &asked).await;
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed[0].timing.outcome.value().map(String::as_str),
        Some("provider_busy")
    );
    asked.outcome = Some(UsageOutcomeFilter::NotMeasured);
    let missing = events(&world, &team.admin, &asked).await;
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].id, unknown);
    asked.outcome = Some(UsageOutcomeFilter::Forced);
    assert_eq!(events(&world, &team.admin, &asked).await.len(), 1);
    asked.outcome = Some(UsageOutcomeFilter::Running);
    asked.include_running = Some(true);
    assert!(events(&world, &team.admin, &asked).await.is_empty());
    let mut chart = query(&asked.scope);
    chart.outcome = Some(UsageOutcomeFilter::Errors);
    assert_eq!(
        figures(&world, &team.admin, &chart)
            .await
            .totals
            .performance
            .requests,
        1
    );
    chart.outcome = Some(UsageOutcomeFilter::NotMeasured);
    assert_eq!(
        figures(&world, &team.admin, &chart)
            .await
            .totals
            .performance
            .requests,
        0
    );
    let bad = world
        .app
        .call(UsageEventList, &[])
        .token(&team.admin.token)
        .raw_query(&format!(
            "scope=node:{}&from={FROM}&to={TODAY}&outcome=error",
            team.project
        ))
        .send()
        .await;
    assert_eq!(bad.status, http::StatusCode::BAD_REQUEST);
}

async fn figures(world: &World, by: &Who, asked: &UsageGetQuery) -> UsageAnalytics {
    world
        .app
        .call(UsageGet, &[])
        .token(&by.token)
        .query(asked)
        .send()
        .await
        .ok()
}

async fn events(world: &World, by: &Who, asked: &UsageEventListQuery) -> Vec<UsageEvent> {
    world
        .app
        .call(UsageEventList, &[])
        .token(&by.token)
        .query(asked)
        .send()
        .await
        .ok()
        .items
}

#[tokio::test]
async fn out_of_range_usage_dates_and_cursors_are_invalid_requests_instead_of_database_errors() {
    let world = World::new().await;
    let team = team(&world).await;
    for date in ["-262000-01-01", "+262000-01-01", "0000-01-01"] {
        let mut asked = events_query("me");
        asked.from = date.to_owned();
        let problem = world
            .app
            .call(UsageEventList, &[])
            .token(&team.admin.token)
            .query(&asked)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{date}");
        let mut asked = query("me");
        asked.to = date.to_owned();
        let problem = world
            .app
            .call(UsageGet, &[])
            .token(&team.admin.token)
            .query(&asked)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid, "{date}");
    }
    for at in [
        chrono::DateTime::<Utc>::MIN_UTC,
        chrono::DateTime::<Utc>::MAX_UTC,
    ] {
        let mut asked = events_query("me");
        asked.cursor = Some(format!(
            "{}.{}",
            at.timestamp_micros(),
            orch_types::new_id()
        ));
        let problem = world
            .app
            .call(UsageEventList, &[])
            .token(&team.admin.token)
            .query(&asked)
            .send()
            .await
            .problem();
        assert_eq!(problem.code, ErrorCode::RequestInvalid);
    }
    let mut asked = events_query("me");
    asked.cursor = Some(format!(
        "{}.{}",
        Utc.with_ymd_and_hms(1965, 1, 1, 0, 0, 0)
            .unwrap()
            .timestamp_micros(),
        orch_types::new_id()
    ));
    assert!(
        events(&world, &team.admin, &asked).await.is_empty(),
        "valid dates before Unix time stay valid"
    );
}

fn breakdown(answer: &UsageAnalytics, by: UsageDimension) -> &UsageBreakdown {
    answer
        .breakdowns
        .iter()
        .find(|each| each.by == by)
        .unwrap_or_else(|| panic!("no breakdown by {by:?}"))
}

/// The usernames a breakdown by person names, in its order.
fn people_of(rows: &UsageBreakdown) -> Vec<String> {
    rows.rows
        .iter()
        .map(|row| {
            row.key
                .person
                .clone()
                .flatten()
                .map_or_else(|| "nobody".to_owned(), |user| user.username)
        })
        .collect()
}

/// The team has used something: the viewer 1 000 tokens at a catalogue estimate of 0.003 credits, the admin
/// 500 at a rate card's 0.002, the stranger 200 with no rate.
async fn used(world: &World, t: &Team) {
    spend(
        world,
        t,
        &Use::of(world, t.viewer.id(), 1000).rated("0.003", "catalogue"),
    )
    .await;
    spend(
        world,
        t,
        &Use::of(world, t.admin.id(), 500).rated("0.002", "rate_card"),
    )
    .await;
    spend(world, t, &Use::of(world, t.stranger.id(), 200)).await;
}

#[tokio::test]
async fn a_person_reads_their_own_use_in_full_and_a_place_they_cannot_see_is_a_reference_without_a_name()
 {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;

    let stranger = figures(&world, &t.stranger, &query("me")).await;
    assert_eq!(stranger.depth, UsageDepth::Own);
    assert_eq!(
        stranger.totals.tokens.total_tokens, 200,
        "their own, and nobody else's"
    );
    assert_eq!(stranger.totals.turns, 1);
    let projects = breakdown(&stranger, UsageDimension::Project);
    assert_eq!(projects.rows.len(), 1);
    let place = projects.rows[0].key.project.clone().unwrap();
    assert!(
        place.redacted && place.name.is_none(),
        "the stranger cannot see the project, and the use stays: {place:?}"
    );

    let viewer = figures(&world, &t.viewer, &query("me")).await;
    assert_eq!(viewer.totals.tokens.total_tokens, 1000);
    let place = breakdown(&viewer, UsageDimension::Project).rows[0]
        .key
        .project
        .clone()
        .unwrap();
    assert_eq!(
        place.name.as_deref(),
        Some("Billing"),
        "the viewer sees where it was"
    );

    let theirs = events(&world, &t.stranger, &events_query("me")).await;
    assert_eq!(theirs.len(), 1, "every record of it, and only that");
    assert_eq!(theirs[0].tokens.total_tokens, 200);
    assert!(
        theirs[0].project.redacted,
        "named by a reference without a name"
    );
}

#[tokio::test]
async fn a_places_figures_need_its_row_and_show_everybodys_use_by_person_but_not_by_thread() {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;
    let node = format!("node:{}", t.project);

    let summary = figures(&world, &t.viewer, &query(&node)).await;

    assert_eq!(
        summary.depth,
        UsageDepth::Summary,
        "as shipped, everybody who can see the place"
    );
    assert_eq!(
        summary.totals.tokens.total_tokens, 1700,
        "the use of everybody there"
    );
    assert_eq!(summary.totals.turns, 3);
    assert_eq!(
        people_of(breakdown(&summary, UsageDimension::Person)),
        ["viewer", "admin", "stranger"],
        "a person who appears in a summary appears with their name, the largest first"
    );
    let me = summary
        .me
        .clone()
        .expect("the caller's own part of the totals");
    assert_eq!(me.tokens.total_tokens, 1000);
    let mut by_thread = query(&node);
    by_thread.breakdowns = Some("thread".to_owned());
    let refused = world
        .app
        .call(UsageGet, &[])
        .token(&t.viewer.token)
        .query(&by_thread)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "no thread at the depth of a summary"
    );
    assert_eq!(missing_permissions(&refused), ["usage.view"]);
    let mut series_by_thread = query(&node);
    series_by_thread.series_by = Some(UsageDimension::Thread);
    let refused = world
        .app
        .call(UsageGet, &[])
        .token(&t.viewer.token)
        .query(&series_by_thread)
        .send()
        .await
        .problem();
    assert_eq!(missing_permissions(&refused), ["usage.view"]);

    let reports = figures(&world, &t.admin, &query(&node)).await;
    assert_eq!(reports.depth, UsageDepth::Reports);
    assert_eq!(
        reports.totals, summary.totals,
        "the same figures at every depth"
    );
    let mut threads = query(&node);
    threads.breakdowns = Some("thread".to_owned());
    let by_thread = figures(&world, &t.admin, &threads).await;
    let rows = &breakdown(&by_thread, UsageDimension::Thread).rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].key.thread.clone().unwrap().name.as_deref(),
        Some("Fix totals")
    );
}

#[tokio::test]
async fn a_place_is_refused_to_somebody_who_cannot_see_it_and_a_scope_that_is_none_is_invalid() {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;

    let outside = world
        .app
        .call(UsageGet, &[])
        .token(&t.stranger.token)
        .query(&query(&format!("node:{}", t.project)))
        .send()
        .await
        .problem();
    assert_eq!(outside.code, ErrorCode::AuthzDenied);
    assert_eq!(
        missing_permissions(&outside),
        ["project.view"],
        "not even sight of the place"
    );
    let nobody = world
        .app
        .call(UsageGet, &[])
        .query(&query("me"))
        .send()
        .await
        .problem();
    assert_eq!(nobody.code, ErrorCode::AuthRequired);
    for scope in ["", "everything", "node:not-an-id", "provider:"] {
        let invalid = world
            .app
            .call(UsageGet, &[])
            .token(&t.admin.token)
            .query(&query(scope))
            .send()
            .await
            .problem();
        assert_eq!(invalid.code, ErrorCode::RequestInvalid, "{scope:?}");
    }
}

#[tokio::test]
async fn credits_that_are_an_estimate_are_marked_and_tokens_without_a_rate_are_counted_with_none() {
    let world = World::new().await;
    let t = team(&world).await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.viewer.id(), 1000).rated("0.25", "catalogue"),
    )
    .await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.viewer.id(), 3000).rated("0.75", "provider"),
    )
    .await;
    spend(&world, &t, &Use::of(&world, t.viewer.id(), 4000)).await;

    let answer = figures(&world, &t.admin, &query(&format!("node:{}", t.project))).await;

    let totals = answer.totals;
    assert_eq!(totals.tokens.total_tokens, 8000, "every token is counted");
    assert_eq!(
        totals.credits, "1",
        "the credits of the use that had a rate: 0.25 and 0.75"
    );
    assert_eq!(
        totals.estimated_credits, "0.25",
        "the part counted at the shipped catalogue's rates is said to be an estimate"
    );
    assert_eq!(
        totals.unrated_tokens, 4000,
        "and what the sum leaves out is told"
    );
    assert_eq!(totals.turns, 3);

    let rows = events(
        &world,
        &t.admin,
        &events_query(&format!("node:{}", t.project)),
    )
    .await;
    let unrated: Vec<&UsageEvent> = rows
        .iter()
        .filter(|row| row.tokens.total_tokens == 4000)
        .collect();
    assert_eq!(unrated.len(), 1);
    assert_eq!(unrated[0].credits, None, "no rate, no credits: never 0");
    assert_eq!(unrated[0].rate, None);
    assert_eq!(unrated[0].rate_source, None);
    let estimated: Vec<&UsageEvent> = rows
        .iter()
        .filter(|row| row.rate_source == Some(RateSource::Catalogue))
        .collect();
    assert_eq!(estimated.len(), 1);
    assert_eq!(estimated[0].credits.as_deref(), Some("0.25"));
}

#[tokio::test]
async fn every_figure_can_be_followed_to_the_records_that_add_up_to_it() {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;
    let other = another_thread(&world, &t, "Second").await;
    spend_in(
        &world,
        &t,
        other,
        &Use::of(&world, t.admin.id(), 70).rated("0.001", "provider"),
    )
    .await;
    // A trace does not add tokens or credits. Its distinct turn is still work.
    spend(&world, &t, &Use::of(&world, t.admin.id(), 9999).trace()).await;
    let node = format!("node:{}", t.project);

    let answer = figures(&world, &t.admin, &query(&node)).await;
    let rows = events(&world, &t.admin, &events_query(&node)).await;
    let counted: Vec<&UsageEvent> = rows.iter().filter(|row| row.counted).collect();

    assert_eq!(rows.len(), 5, "the trace is a record too");
    assert_eq!(
        counted
            .iter()
            .map(|row| row.tokens.total_tokens)
            .sum::<i64>(),
        answer.totals.tokens.total_tokens
    );
    assert_eq!(answer.totals.tokens.total_tokens, 1770);
    assert_eq!(
        answer.totals.turns, 5,
        "a turn is counted even without a counted token meter"
    );
    let only_counted = {
        let mut asked = events_query(&node);
        asked.counted = Some(true);
        events(&world, &t.admin, &asked).await
    };
    assert_eq!(only_counted.len(), 4);
    let only_traces = {
        let mut asked = events_query(&node);
        asked.counted = Some(false);
        events(&world, &t.admin, &asked).await
    };
    assert_eq!(only_traces.len(), 1);
    assert_eq!(only_traces[0].tokens.total_tokens, 9999);
    let one_thread = {
        let mut asked = events_query(&node);
        asked.thread_id = Some(other);
        events(&world, &t.admin, &asked).await
    };
    assert_eq!(one_thread.len(), 1, "the same filters as the figures have");
    let mut narrowed = query(&node);
    narrowed.thread_id = Some(other);
    assert_eq!(
        figures(&world, &t.admin, &narrowed)
            .await
            .totals
            .tokens
            .total_tokens,
        70
    );
}

#[tokio::test]
async fn unmetered_turns_are_counted_like_the_records_in_totals_series_and_by_thread() {
    let world = World::new().await;
    let team = team(&world).await;
    for _ in 0..19 {
        spend(&world, &team, &Use::of(&world, team.admin.id(), 0).trace()).await;
    }
    let scope = format!("node:{}", team.project);
    let records = events(&world, &team.admin, &events_query(&scope)).await;
    let turns: std::collections::HashSet<_> =
        records.iter().filter_map(|row| row.turn_id).collect();
    assert_eq!(turns.len(), 19);
    for bucket in [UsageBucket::Day, UsageBucket::Hour] {
        let mut asked = query(&scope);
        asked.bucket = Some(bucket);
        asked.breakdowns = Some("thread,model".into());
        let answer = figures(&world, &team.admin, &asked).await;
        assert_eq!(answer.totals.turns, 19);
        assert_eq!(
            answer.totals.tokens.total_tokens, 0,
            "an absent token meter does not invent consumption"
        );
        assert_eq!(
            answer
                .series
                .iter()
                .map(|point| point.figures.turns)
                .sum::<i64>(),
            19
        );
        let threads = answer
            .breakdowns
            .iter()
            .find(|table| table.by == UsageDimension::Thread)
            .unwrap();
        assert_eq!(threads.rows.len(), 1);
        assert_eq!(threads.rows[0].figures.turns, 19);
        let models = answer
            .breakdowns
            .iter()
            .find(|table| table.by == UsageDimension::Model)
            .unwrap();
        assert_eq!(models.rows[0].figures.turns, 19);
    }
}

#[tokio::test]
async fn the_previous_span_is_returned_in_the_same_analytics_answer() {
    let world = World::new().await;
    let team = team(&world).await;
    let yesterday = world.now() - Duration::days(1);
    spend(
        &world,
        &team,
        &Use::of(&world, team.admin.id(), 321).at(yesterday),
    )
    .await;
    let mut asked = query(&format!("node:{}", team.project));
    asked.from = TODAY.into();
    asked.to = TODAY.into();
    let current = figures(&world, &team.admin, &asked).await;
    let mut previous = asked.clone();
    previous.from = yesterday.format("%Y-%m-%d").to_string();
    previous.to = previous.from.clone();
    let expected = figures(&world, &team.admin, &previous).await;
    assert_eq!(current.figures_before.as_ref(), Some(&expected.totals));
    assert_eq!(current.performance_before, expected.totals.performance);
}

#[tokio::test]
async fn a_summary_reader_is_given_no_record_but_their_own_and_cannot_ask_for_somebody_elses() {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;
    let node = format!("node:{}", t.project);

    let mine = events(&world, &t.viewer, &events_query(&node)).await;
    assert_eq!(mine.len(), 1, "of the three rows, theirs");
    assert_eq!(mine[0].user.clone().unwrap().username, "viewer");
    assert!(
        mine[0].thread.is_some(),
        "they may follow their own to its thread"
    );

    let mut asked = events_query(&node);
    asked.user_id = Some(t.admin.id());
    let refused = world
        .app
        .call(UsageEventList, &[])
        .token(&t.viewer.token)
        .query(&asked)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["usage.view"]);

    let everybody = events(&world, &t.admin, &events_query(&node)).await;
    assert_eq!(everybody.len(), 3, "a reader of reports sees all");
}

#[tokio::test]
async fn the_records_are_paged_newest_first_unless_asked_otherwise() {
    let world = World::new().await;
    let t = team(&world).await;
    let base = Utc.with_ymd_and_hms(2026, 10, 7, 9, 0, 0).unwrap();
    for n in 0..5 {
        spend(
            &world,
            &t,
            &Use::of(&world, t.admin.id(), 100 + n).at(base + Duration::minutes(n)),
        )
        .await;
    }
    let node = format!("node:{}", t.project);
    let mut asked = events_query(&node);
    asked.limit = Some(2);

    let first = world
        .app
        .call(UsageEventList, &[])
        .token(&t.admin.token)
        .query(&asked)
        .send()
        .await
        .ok();
    assert_eq!(
        first
            .items
            .iter()
            .map(|row| row.tokens.total_tokens)
            .collect::<Vec<_>>(),
        [104, 103],
        "the newest first"
    );
    asked.cursor = first.next_cursor.clone();
    let second = world
        .app
        .call(UsageEventList, &[])
        .token(&t.admin.token)
        .query(&asked)
        .send()
        .await
        .ok();
    assert_eq!(
        second
            .items
            .iter()
            .map(|row| row.tokens.total_tokens)
            .collect::<Vec<_>>(),
        [102, 101]
    );
    asked.cursor = second.next_cursor.clone();
    let last = world
        .app
        .call(UsageEventList, &[])
        .token(&t.admin.token)
        .query(&asked)
        .send()
        .await
        .ok();
    assert_eq!(last.items.len(), 1);
    assert_eq!(last.next_cursor, None, "the end of the list");

    let mut oldest = events_query(&node);
    oldest.order = Some(UsageEventOrder::Asc);
    oldest.limit = Some(1);
    let oldest = events(&world, &t.admin, &oldest).await;
    assert_eq!(oldest[0].tokens.total_tokens, 100);

    let mut bad = events_query(&node);
    bad.cursor = Some("not a cursor".to_owned());
    let invalid = world
        .app
        .call(UsageEventList, &[])
        .token(&t.admin.token)
        .query(&bad)
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_span_is_cut_into_points_by_the_bucket_asked_or_chosen_by_its_length() {
    let world = World::new().await;
    let t = team(&world).await;
    let at = |day: u32| Utc.with_ymd_and_hms(2026, 10, day, 10, 30, 0).unwrap();
    for (day, tokens) in [(6, 100), (7, 200), (8, 300)] {
        spend(
            &world,
            &t,
            &Use::of(&world, t.admin.id(), tokens).at(at(day)),
        )
        .await;
    }
    let node = format!("node:{}", t.project);

    let days = figures(&world, &t.admin, &query(&node)).await;
    assert_eq!(
        days.bucket,
        UsageBucket::Day,
        "eight days are cut by the day"
    );
    assert_eq!(
        days.series.len(),
        8,
        "a point for each day, those without use too"
    );
    let tokens: Vec<i64> = days
        .series
        .iter()
        .map(|point| point.figures.tokens.total_tokens)
        .collect();
    assert_eq!(tokens, [0, 0, 0, 0, 0, 100, 200, 300]);
    assert_eq!(
        days.series[5].at.get(),
        at(6).date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc()
    );

    let mut by_week = query(&node);
    by_week.bucket = Some(UsageBucket::Week);
    let weeks = figures(&world, &t.admin, &by_week).await;
    let tokens: Vec<i64> = weeks
        .series
        .iter()
        .map(|point| point.figures.tokens.total_tokens)
        .collect();
    assert_eq!(
        tokens,
        [0, 600],
        "weeks begin on Monday: the 28th of September and the 5th of October"
    );

    let mut by_month = query(&node);
    by_month.bucket = Some(UsageBucket::Month);
    let months = figures(&world, &t.admin, &by_month).await;
    assert_eq!(months.series.len(), 1);
    assert_eq!(months.series[0].figures.tokens.total_tokens, 600);

    let mut by_hour = query(&node);
    by_hour.from = "2026-10-08".to_owned();
    let hours = figures(&world, &t.admin, &by_hour).await;
    assert_eq!(hours.bucket, UsageBucket::Hour, "a day is cut by the hour");
    assert_eq!(hours.series.len(), 24);
    assert_eq!(
        hours.series[10].figures.tokens.total_tokens, 300,
        "read from the ledger itself"
    );
    assert_eq!(hours.totals.tokens.total_tokens, 300);

    let mut long = query(&node);
    long.from = "2026-08-01".to_owned();
    long.bucket = Some(UsageBucket::Hour);
    let invalid = world
        .app
        .call(UsageGet, &[])
        .token(&t.admin.token)
        .query(&long)
        .send()
        .await
        .problem();
    assert_eq!(
        invalid.code,
        ErrorCode::RequestInvalid,
        "no answer holds more than some 800 points"
    );
    let mut backwards = query(&node);
    backwards.to = "2026-09-01".to_owned();
    let invalid = world
        .app
        .call(UsageGet, &[])
        .token(&t.admin.token)
        .query(&backwards)
        .send()
        .await
        .problem();
    assert_eq!(invalid.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_series_can_be_split_by_a_dimension_into_its_largest_keys() {
    let world = World::new().await;
    let t = team(&world).await;
    spend(&world, &t, &Use::of(&world, t.admin.id(), 300)).await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.viewer.id(), 100).model("gpt-6-mini"),
    )
    .await;
    let mut asked = query(&format!("node:{}", t.project));
    asked.series_by = Some(UsageDimension::Model);

    let answer = figures(&world, &t.admin, &asked).await;

    let last = answer.series.last().unwrap();
    let by: Vec<(String, i64)> = last
        .by
        .iter()
        .map(|split| {
            (
                split.key.model.clone().unwrap_or_default(),
                split.figures.tokens.total_tokens,
            )
        })
        .collect();
    assert!(by.contains(&("gpt-6-sol".to_owned(), 300)), "{by:?}");
    assert!(by.contains(&("gpt-6-mini".to_owned(), 100)), "{by:?}");
    assert!(
        answer.series[0].by.is_empty()
            || answer.series[0]
                .by
                .iter()
                .all(|split| split.figures.turns == 0)
    );
}

#[tokio::test]
async fn a_series_is_answered_as_csv_with_a_line_for_each_point_when_the_reader_asks_for_it() {
    let world = World::new().await;
    let t = team(&world).await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.admin.id(), 300).rated("0.5", "catalogue"),
    )
    .await;

    let reply = world
        .app
        .call(UsageGet, &[])
        .token(&t.admin.token)
        .header("accept", "text/csv")
        .query(&query(&format!("node:{}", t.project)))
        .send_bytes()
        .await;

    assert!(reply.status.is_success());
    let kind = reply.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(kind.starts_with("text/csv"), "{kind}");
    let text = String::from_utf8(reply.raw.to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        1 + 8,
        "the header and a point for each of the eight days"
    );
    // The figures of each point, then its performance.
    assert!(
        lines[0].starts_with("at,turns,")
            && lines[0].contains(",unrated_tokens,requests,")
            && lines[0].ends_with(",errors_by_code"),
        "{}",
        lines[0]
    );
    assert!(
        lines.iter().any(|line| line.contains(",300,0.5,0.5,0")),
        "the day of the use, with its credits and what was an estimate: {lines:?}"
    );
}

#[tokio::test]
async fn the_records_are_answered_as_csv_and_a_row_without_a_rate_has_an_empty_cell_for_its_credits()
 {
    let world = World::new().await;
    let t = team(&world).await;
    spend(
        &world,
        &t,
        &Use::of(&world, t.admin.id(), 300).rated("0.5", "catalogue"),
    )
    .await;
    spend(&world, &t, &Use::of(&world, t.admin.id(), 400)).await;

    let reply = world
        .app
        .call(UsageEventList, &[])
        .token(&t.admin.token)
        .header("accept", "text/csv")
        .query(&events_query(&format!("node:{}", t.project)))
        .send_bytes()
        .await;

    let text = String::from_utf8(reply.raw.to_vec()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    let columns: Vec<&str> = lines[0].split(',').collect();
    let credits = columns.iter().position(|name| *name == "credits").unwrap();
    let total = columns
        .iter()
        .position(|name| *name == "total_tokens")
        .unwrap();
    let cell = |line: &str, index: usize| line.split(',').nth(index).unwrap().to_owned();
    let by_tokens = |tokens: &str| {
        lines[1..]
            .iter()
            .find(|line| cell(line, total) == tokens)
            .unwrap()
    };
    assert_eq!(cell(by_tokens("300"), credits), "0.5");
    assert_eq!(cell(by_tokens("400"), credits), "", "never 0");
}

#[tokio::test]
async fn provider_totals_include_hidden_places_while_only_managers_can_list_their_requests() {
    let world = World::new().await;
    let t = team(&world).await;
    used(&world, &t).await;
    let scope = format!("provider:{}", t.provider);

    let answer = figures(&world, &t.boss, &query(&scope)).await;
    assert_eq!(answer.depth, UsageDepth::Reports);
    assert_eq!(
        answer.totals.tokens.total_tokens, 1700,
        "of whatever org, by every dimension"
    );

    let refused = world
        .app
        .call(UsageGet, &[])
        .token(&t.viewer.token)
        .query(&query(&scope))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(missing_permissions(&refused), ["provider.view"]);
    let refused = world
        .app
        .call(UsageEventList, &[])
        .token(&t.stranger.token)
        .query(&events_query(&scope))
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);

    // Provider use/view gives totals, never an activity log of places the person cannot view.
    resources::name_person(
        &world,
        "provider",
        t.provider,
        "provider.view",
        t.stranger.id(),
    )
    .await;
    assert!(
        events(&world, &t.stranger, &events_query(&scope))
            .await
            .is_empty()
    );
    assert_eq!(
        figures(&world, &t.stranger, &query(&scope))
            .await
            .totals
            .tokens
            .total_tokens,
        1700
    );
    let hidden = another_thread(&world, &t, "Private work").await;
    spend_in(
        &world,
        &t,
        hidden,
        &Use::of(&world, t.boss.id(), 10).at(world.now()),
    )
    .await;
    {
        // A thread has no roles of its own: the stranger is named on its page.
        let mut conn = world.conn().await;
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", t.thread, "live"),
            "any",
            "thread.view",
            ("user", Some(t.stranger.id()), None),
            None,
            world.now(),
        )
        .await
        .unwrap();
    }
    let mut asked = events_query(&scope);
    asked.limit = Some(1);
    let mut seen = Vec::new();
    loop {
        let page = world
            .app
            .call(UsageEventList, &[])
            .token(&t.stranger.token)
            .query(&asked)
            .send()
            .await
            .ok();
        seen.extend(page.items);
        asked.cursor = page.next_cursor;
        if asked.cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        3,
        "paging never loses visible records behind hidden candidates"
    );
    assert!(seen.iter().all(|row| {
        row.thread
            .as_ref()
            .is_some_and(|thread| thread.id == t.thread)
    }));
    assert_eq!(
        figures(&world, &t.stranger, &query(&scope))
            .await
            .totals
            .tokens
            .total_tokens,
        1710
    );
    resources::name_person(
        &world,
        "provider",
        t.provider,
        "provider.manage",
        t.stranger.id(),
    )
    .await;
    let seen = events(&world, &t.stranger, &events_query(&scope)).await;
    assert_eq!(
        seen.len(),
        4,
        "provider managers may list every place, with names still redacted"
    );
    let seen: Vec<_> = seen
        .into_iter()
        .filter(|row| {
            row.thread
                .as_ref()
                .is_some_and(|thread| thread.id == hidden)
        })
        .collect();
    assert!(
        seen.iter()
            .all(|row| row.turn_id.is_none() && row.turn_ordinal.is_none()),
        "provider access does not expose identifiers or turn numbers from a hidden conversation"
    );
    assert!(
        seen.iter().all(|row| row
            .thread
            .as_ref()
            .is_some_and(|thread| thread.redacted && thread.name.is_none())),
        "nothing of where the use was is shown to somebody who cannot see it"
    );
}
