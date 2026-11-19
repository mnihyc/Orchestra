//! The usage ledger keeps one row for a use however often it is written, says what the use ran on and whom it is put down to as it was, stores the credits exactly, and adds up only the rows that count.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::Utc;
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::queries::threads::{
    self, NewInput, NewLease, NewSegment, NewTurn, Place, insert_input, insert_segment,
    insert_thread, insert_turn, open_lease, set_lease_turn,
};
use orch_db::usage::ledger::{
    self, Attribution, NewEvent, Tokens, attribution_of_lease, counted_request_totals,
    events_of_turn, record,
};
use orch_db::{Conn, Db};
use orch_testkit::database::TestDb;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

/// A pool for a database of its own.
async fn world() -> (TestDb, Db) {
    let db = TestDb::fresh().await;
    let pool = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&db.url)
            .await
            .unwrap(),
    );
    (db, pool)
}

/// A turn that holds a lease, in a thread of a topic of a project of an org, on a profile, a provider and an
/// environment; the person is who sent its input, where a person did.
struct Chain {
    person: Id,
    org: Id,
    project: Id,
    topic: Id,
    thread: Id,
    segment: Id,
    turn: Id,
    lease: Id,
    provider: Id,
    agent: Id,
    environment: Id,
}

async fn chain(conn: &mut Conn, by_a_person: bool) -> Chain {
    let now = Utc::now();
    let owner = new_id();
    insert_user(conn, owner, "ada").await.unwrap();
    let author = by_a_person.then_some(owner);
    let (org, project, topic, thread) = (new_id(), new_id(), new_id(), new_id());
    insert_node(conn, org, "org", None, "Acme", owner)
        .await
        .unwrap();
    insert_node(conn, project, "project", Some(org), "Web", owner)
        .await
        .unwrap();
    insert_node(conn, topic, "topic", Some(project), "Login", owner)
        .await
        .unwrap();
    assert!(
        insert_thread(conn, thread, topic, "Fix", owner, now)
            .await
            .unwrap()
    );
    let machine = insert_machine(conn, "m1", "connected").await.unwrap();
    let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
    sqlx::query(
        "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
         VALUES ($1, 'Codex', $2, $3, $3, 'codex', '{}', '{}')",
    )
    .bind(agent)
    .bind(owner)
    .bind(org)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, protocols,
                                machine_id, arrangements, status)
         VALUES ($1, 'Key', $2, $3, $3, 'api', $4, ARRAY['openai-responses'], $5,
                 ARRAY['key_separated'], 'ready')",
    )
    .bind(provider)
    .bind(owner)
    .bind(org)
    .bind(json!([{ "protocol": "openai-responses", "base_url": "https://api.example.com/v1", "options": {} }]))
    .bind(machine)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 't', 'test', '{}')",
    )
    .bind(template)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                   template_id, generation, status)
         VALUES ($1, 'Work', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running')",
    )
    .bind(environment)
    .bind(owner)
    .bind(org)
    .bind(topic)
    .bind(machine)
    .bind(template)
    .execute(&mut *conn)
    .await
    .unwrap();
    let segment = insert_segment(
        conn,
        &NewSegment {
            id: new_id(),
            thread_id: thread,
            agent_id: agent,
            provider_id: provider,
            environment_id: environment,
            env_generation: 1,
            arrangement: "key_separated",
            executor: None,
            tool_set: None,
            credential_path: "forwarded",
            native_session_ref: None,
            started_by: author,
            started_at: now,
        },
        1,
    )
    .await
    .unwrap();
    let content = json!([{ "type": "text", "text": "go" }]);
    let input = insert_input(
        conn,
        &NewInput {
            plan: false,
            id: new_id(),
            thread_id: thread,
            author_id: author,
            submission_authority: Some(
                &serde_json::json!({"credentials":[{"type":"session","as_admin":false}]}),
            ),
            run_as: None,
            run_as_authority: None,
            source: if author.is_some() { "user" } else { "system" },
            content: &content,
            delivery: "queue",
            state: "pending",
            turn_id: None,
            client_id: None,
            limit_bypass: None,
            place: Place::End,
            at: now,
        },
        2,
    )
    .await
    .unwrap()
    .unwrap();
    let lease = open_lease(
        conn,
        &NewLease {
            id: new_id(),
            provider_id: provider,
            thread_id: thread,
            mode: "forwarded",
            form: None,
            allowed_models: &["gpt-6-sol".to_owned()],
            allowance: None,
            at: now,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let turn = insert_turn(
        conn,
        &NewTurn {
            id: new_id(),
            thread_id: thread,
            segment_id: segment.id,
            input_id: input.id,
            model: "gpt-6-sol",
            effort: Some("high"),
            params: &json!({ "service_tier": "fast" }),
            lease_id: Some(lease.id),
            limit_bypass: None,
            at: now,
        },
        3,
    )
    .await
    .unwrap();
    set_lease_turn(conn, lease.id, turn.id).await.unwrap();
    Chain {
        person: owner,
        org,
        project,
        topic,
        thread,
        segment: segment.id,
        turn: turn.id,
        lease: lease.id,
        provider,
        agent,
        environment,
    }
}

fn used(input: i64, cached: i64, output: i64) -> Tokens {
    Tokens {
        input,
        cached_input: cached,
        cache_write: 0,
        output,
        reasoning: 0,
        total: input + output,
    }
}

fn a_request<'a>(
    id: Id,
    attribution: &'a Attribution,
    tokens: Tokens,
    counted: bool,
) -> NewEvent<'a> {
    NewEvent {
        id,
        at: Utc::now(),
        origin: ledger::origin::FORWARDER,
        scope: ledger::scope::REQUEST,
        counted,
        attribution,
        model: "gpt-6-sol",
        tier: None,
        tokens,
        context_tokens: Some(tokens.input),
        request_ref: Some("resp_1"),
        rate: None,
        rate_source: None,
        credits: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_use_that_is_written_again_changes_nothing() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let place = chain(&mut conn, true).await;
    let attribution = attribution_of_lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    let id = new_id();
    assert!(
        record(
            &mut conn,
            &a_request(id, &attribution, used(100, 0, 10), true)
        )
        .await
        .unwrap()
    );
    assert!(
        !record(
            &mut conn,
            &a_request(id, &attribution, used(999, 0, 999), true)
        )
        .await
        .unwrap(),
        "the identifier is the use: what is written again is the same use"
    );
    let rows = events_of_turn(&mut conn, place.turn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].input_tokens, 100,
        "the first stands, and nothing is edited"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_says_what_the_use_ran_on_and_the_person_and_place_it_is_put_down_to() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let place = chain(&mut conn, true).await;
    let person = place.person;
    let attribution = attribution_of_lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    record(
        &mut conn,
        &a_request(new_id(), &attribution, used(1_000, 200, 100), true),
    )
    .await
    .unwrap();
    let rows = events_of_turn(&mut conn, place.turn).await.unwrap();
    let row = &rows[0];
    assert_eq!(
        (row.org_id, row.project_id, row.topic_id, row.thread_id),
        (place.org, place.project, place.topic, Some(place.thread))
    );
    assert_eq!(
        (row.segment_id, row.turn_id),
        (Some(place.segment), Some(place.turn))
    );
    assert_eq!(row.provider_id, place.provider);
    assert_eq!(row.user_id, Some(person));
    assert_eq!(row.agent_id, Some(place.agent));
    assert_eq!(row.harness_kind.as_deref(), Some("codex"));
    assert_eq!(row.environment_id, Some(place.environment));
    assert_eq!(row.environment_kind.as_deref(), Some("managed"));
    assert_eq!(row.arrangement.as_deref(), Some("key_separated"));
    assert_eq!(row.effort.as_deref(), Some("high"));
    assert_eq!(
        (row.origin.as_str(), row.scope.as_str(), row.counted),
        ("forwarder", "request", true)
    );
    assert_eq!(
        (
            row.input_tokens,
            row.cached_input_tokens,
            row.output_tokens,
            row.total_tokens
        ),
        (1_000, 200, 100, 1_100)
    );
    assert_eq!(attribution.params, json!({ "service_tier": "fast" }));
    assert_eq!(attribution.turn_model, "gpt-6-sol");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_turn_that_no_person_started_is_put_down_to_nobody() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let place = chain(&mut conn, false).await;
    let attribution = attribution_of_lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attribution.user_id, None);
    record(
        &mut conn,
        &a_request(new_id(), &attribution, used(1, 0, 1), true),
    )
    .await
    .unwrap();
    assert_eq!(
        events_of_turn(&mut conn, place.turn).await.unwrap()[0].user_id,
        None
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credits_are_kept_exactly_with_the_rate_they_were_counted_at_and_a_row_without_a_rate_has_none()
 {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let place = chain(&mut conn, true).await;
    let attribution = attribution_of_lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    let rate: Json = json!({ "input_per_mtok": "2.50", "cached_input_per_mtok": "0.25", "output_per_mtok": "15" });
    let (rated, unrated) = (new_id(), new_id());
    record(
        &mut conn,
        &NewEvent {
            rate: Some(&rate),
            rate_source: Some(ledger::rate_source::CATALOGUE),
            credits: Some("0.003550"),
            ..a_request(rated, &attribution, used(1_000, 200, 100), true)
        },
    )
    .await
    .unwrap();
    record(
        &mut conn,
        &a_request(unrated, &attribution, used(5, 0, 5), true),
    )
    .await
    .unwrap();
    let rows = events_of_turn(&mut conn, place.turn).await.unwrap();
    let rated = rows.iter().find(|row| row.id == rated).unwrap();
    assert_eq!(rated.credits.as_deref(), Some("0.003550"));
    assert_eq!(rated.rate, Some(rate));
    assert_eq!(rated.rate_source.as_deref(), Some("catalogue"));
    let unrated = rows.iter().find(|row| row.id == unrated).unwrap();
    assert!(unrated.credits.is_none() && unrated.rate.is_none() && unrated.rate_source.is_none());
    assert_eq!(
        unrated.total_tokens, 10,
        "tokens are counted though no credits are"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_model_requests_that_count_are_added_up() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    let place = chain(&mut conn, true).await;
    let attribution = attribution_of_lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    for (tokens, counted) in [
        (used(100, 10, 20), true),
        (used(50, 0, 5), true),
        (used(7_000, 0, 700), false),
    ] {
        record(
            &mut conn,
            &a_request(new_id(), &attribution, tokens, counted),
        )
        .await
        .unwrap();
    }
    // A turn's total as its harness reported it is a row too, and is not a request.
    record(
        &mut conn,
        &NewEvent {
            origin: ledger::origin::HARNESS,
            scope: ledger::scope::TURN,
            ..a_request(new_id(), &attribution, used(9_000, 0, 900), true)
        },
    )
    .await
    .unwrap();
    let total = counted_request_totals(&mut conn, place.turn).await.unwrap();
    assert_eq!(
        (total.input, total.cached_input, total.output, total.total),
        (150, 10, 25, 175)
    );
    let none = counted_request_totals(&mut conn, new_id()).await.unwrap();
    assert_eq!(none, Tokens::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_with_no_turn_has_nothing_to_be_put_down_to() {
    let (_db, pool) = world().await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        attribution_of_lease(&mut conn, new_id())
            .await
            .unwrap()
            .is_none()
    );
    let place = chain(&mut conn, true).await;
    let _ = threads::lease(&mut conn, place.lease)
        .await
        .unwrap()
        .unwrap();
    assert!(
        attribution_of_lease(&mut conn, place.lease)
            .await
            .unwrap()
            .is_some()
    );
}
