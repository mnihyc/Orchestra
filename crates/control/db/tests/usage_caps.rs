//! A cap is the same as another by the speeds it names as by its models, the caps that could cover a turn are found by the speed the turn asks for, and what the people it covers used is added up by provider and model and only at the speeds the cap names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{TimeZone, Utc};
use orch_db::queries::identity::testing::insert_user;
use orch_db::usage::caps::{self, CapRow, Change, NewCap, TierAsked, Updated};
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens, record};
use orch_db::{Conn, Db};
use orch_testkit::{clock, database::TestDb};
use orch_types::{Id, Json, new_id};
use sqlx::postgres::PgPoolOptions;

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

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// A cap of the system for everyone that counts tokens in a day, naming the models and speeds given.
async fn cap(
    conn: &mut Conn,
    by: Id,
    models: &[&str],
    tiers: &[&str],
) -> Result<Option<CapRow>, orch_db::DbError> {
    let (models, tiers) = (words(models), words(tiers));
    caps::insert(
        conn,
        &NewCap {
            id: new_id(),
            node_id: None,
            provider_id: None,
            subject_kind: caps::subject::EVERYONE,
            subject_id: None,
            subject_role: None,
            split: caps::split::SHARED,
            models: &models,
            tiers: &tiers,
            harness_kinds: &[],
            meter: caps::meter::TOKENS,
            period: Some(caps::period::DAY),
            counted_from: None,
            amount: Some("1000"),
            at_limit: caps::at_limit::STOP,
            warn_percent: None,
            uncounted: caps::uncounted::ALLOW,
            note: "",
            created_by: by,
            at: Utc::now(),
        },
    )
    .await
}

async fn a_person(conn: &mut Conn) -> Id {
    let id = new_id();
    insert_user(conn, id, "ada").await.unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cap_is_the_same_as_another_by_the_speeds_it_names_as_by_its_models() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let ada = a_person(&mut conn).await;
    let all = cap(&mut conn, ada, &["@frontier"], &[])
        .await
        .unwrap()
        .expect("the first is made");
    assert!(
        cap(&mut conn, ada, &["@frontier"], &[])
            .await
            .unwrap()
            .is_none(),
        "the same models and no speed: the same cap"
    );
    let fast = cap(&mut conn, ada, &["@frontier"], &["fast"])
        .await
        .unwrap()
        .expect("a speed makes another cap of it");
    assert!(
        cap(&mut conn, ada, &["@frontier"], &["fast"])
            .await
            .unwrap()
            .is_none()
    );
    let flex = cap(&mut conn, ada, &["@frontier"], &["flex"])
        .await
        .unwrap()
        .expect("another speed is another cap");
    assert_eq!(all.tiers, Vec::<String>::new());
    assert_eq!(fast.tiers, ["fast"]);

    // Changed into the same as another, a cap is refused; changed into something of its own, it is not.
    let at = Utc::now();
    let clash = caps::update(
        &mut conn,
        flex.id,
        flex.revision,
        &Change {
            tiers: Some(&words(&["fast"])),
            ..Change::default()
        },
        at,
    )
    .await
    .unwrap();
    assert_eq!(clash, Updated::Exists);
    let both = caps::update(
        &mut conn,
        flex.id,
        flex.revision,
        &Change {
            tiers: Some(&words(&["fast", "flex"])),
            ..Change::default()
        },
        at,
    )
    .await
    .unwrap();
    let Updated::Done(both) = both else {
        panic!("a cap of its own: {both:?}");
    };
    assert_eq!(both.tiers, ["fast", "flex"]);
    assert_eq!(both.revision, flex.revision + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_caps_that_could_cover_a_turn_are_found_by_the_speed_it_asks_for() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let ada = a_person(&mut conn).await;
    let every = cap(&mut conn, ada, &[], &[]).await.unwrap().unwrap();
    let fast = cap(&mut conn, ada, &[], &["fast"]).await.unwrap().unwrap();
    let fast_or_flex = cap(&mut conn, ada, &[], &["fast", "flex"])
        .await
        .unwrap()
        .unwrap();
    let provider = new_id();
    let ids = |found: Vec<CapRow>| -> Vec<Id> { found.into_iter().map(|cap| cap.id).collect() };
    let mut asked = async |tier| {
        let found = caps::covering(
            &mut conn,
            &[],
            provider,
            tier,
            None,
            None,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        let mut found = ids(found);
        found.sort();
        found
    };
    let sorted = |mut wanted: Vec<Id>| {
        wanted.sort();
        wanted
    };
    assert_eq!(
        asked(TierAsked::Ordinary).await,
        [every.id],
        "a turn at the ordinary speed is covered by the cap that names none"
    );
    assert_eq!(
        asked(TierAsked::Is("fast")).await,
        sorted(vec![every.id, fast.id, fast_or_flex.id])
    );
    assert_eq!(
        asked(TierAsked::Is("flex")).await,
        sorted(vec![every.id, fast_or_flex.id])
    );
    assert_eq!(
        asked(TierAsked::Any).await,
        sorted(vec![every.id, fast.id, fast_or_flex.id]),
        "whatever the speed: every cap that could cover some turn of the thread"
    );
}

/// A counted row of use by a provider's model at a speed, in a place of its own.
async fn use_of(
    conn: &mut Conn,
    provider: Id,
    model: &str,
    tier: Option<&str>,
    person: Id,
    tokens: i64,
) {
    let place = Attribution {
        org_id: new_id(),
        project_id: new_id(),
        topic_id: new_id(),
        thread_id: new_id(),
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: provider,
        entry_id: None,
        user_id: Some(person),
        agent_id: None,
        harness_kind: Some("codex".to_owned()),
        environment_id: None,
        environment_kind: None,
        arrangement: None,
        turn_model: model.to_owned(),
        effort: None,
        params: Json::Null,
    };
    record(
        conn,
        &NewEvent {
            id: new_id(),
            at: Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: &place,
            model,
            tier,
            tokens: Tokens {
                input: tokens,
                cached_input: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: tokens,
            },
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: None,
            credits: None,
        },
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_cap_covers_was_used_is_added_up_by_provider_and_model_and_at_the_speeds_it_names() {
    let (_db, db) = world().await;
    let mut conn = db.acquire().await.unwrap();
    let ada = a_person(&mut conn).await;
    let (one, other) = (new_id(), new_id());
    use_of(&mut conn, one, "gpt-6-sol", None, ada, 100).await;
    use_of(&mut conn, one, "gpt-6-sol", Some("fast"), ada, 20).await;
    use_of(&mut conn, other, "gpt-6-sol", Some("fast"), ada, 3).await;
    use_of(&mut conn, other, "gpt-6-luna", Some("flex"), ada, 4).await;

    let everything = cap(&mut conn, ada, &[], &[]).await.unwrap().unwrap();
    let mut used = caps::used(&mut conn, &everything, None, None, clock::start())
        .await
        .unwrap();
    used.sort_by(|a, b| (a.provider_id, &a.model).cmp(&(b.provider_id, &b.model)));
    let seen: Vec<(Id, &str, i64)> = used
        .iter()
        .map(|row| (row.provider_id, row.model.as_str(), row.total_tokens))
        .collect();
    assert_eq!(
        seen.len(),
        3,
        "a model of two providers is two figures: a group is a group of one provider's entries"
    );
    assert!(seen.contains(&(one, "gpt-6-sol", 120)));
    assert!(seen.contains(&(other, "gpt-6-sol", 3)));
    assert!(seen.contains(&(other, "gpt-6-luna", 4)));

    let fast = cap(&mut conn, ada, &[], &["fast"]).await.unwrap().unwrap();
    let used = caps::used(&mut conn, &fast, None, None, clock::start())
        .await
        .unwrap();
    assert_eq!(
        used.iter().map(|row| row.total_tokens).sum::<i64>(),
        23,
        "only the use at the speed `fast`: neither the ordinary one nor `flex`"
    );
    let who = caps::users_of(&mut conn, &fast, None, clock::start())
        .await
        .unwrap();
    assert_eq!(
        who.len(),
        2,
        "the person, once for each provider and model they used at it"
    );
    assert!(who.iter().all(|(person, _, _)| *person == ada));
}
