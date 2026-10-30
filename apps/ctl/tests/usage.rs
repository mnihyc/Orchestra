//! `orchctl usage catalogue`: the rate catalogue loaded from a file as an upload is, or put back as the release came with it; in force at once, and audited as done by the system.
//!
//! `orchctl usage rebuild` makes the rollups again from the ledger alone, and what
//! it makes is what the live tables held, from a day on or from the beginning.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::{TimeZone, Utc};
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::path::{Path, PathBuf};
use support::Deployment;

/// A catalogue of two families, whose figures are `input` for the first.
fn catalogue(as_of: &str, input: &str) -> Json {
    json!({
        "format": 1, "as_of": as_of, "unit": "USD",
        "families": [
            {
                "match": "gpt-x*", "vendor": "Example", "source": "https://example.test/pricing", "as_of": as_of,
                "rate": { "input_per_mtok": input, "cached_input_per_mtok": "0.1", "output_per_mtok": "10" }
            },
            {
                "match": "other-model*", "vendor": "Example", "source": "https://example.test/other", "as_of": as_of,
                "rate": { "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "8" }
            }
        ]
    })
}

fn write(dir: &Path, name: &str, doc: &Json) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, doc.to_string()).unwrap();
    path
}

/// What is in force: where it came from, its date, the first figure of the
/// first family, and who loaded it.
async fn in_force(world: &Deployment) -> (String, String, String, Option<String>) {
    sqlx::query_as(
        "SELECT origin, as_of::text, doc->'families'->0->'rate'->>'input_per_mtok', loaded_by::text FROM rate_catalogue",
    )
    .fetch_one(world.pool())
    .await
    .unwrap()
}

async fn lines(world: &Deployment) -> Vec<(String, String, Option<String>, Json)> {
    sqlx::query_as(
        "SELECT action, actor_kind, user_agent, details FROM audit_log WHERE action = 'rate_catalogue.loaded' ORDER BY at, id",
    )
    .fetch_all(world.pool())
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalogue_from_a_file_is_in_force_at_once_and_audited_as_done_by_the_system() {
    let world = Deployment::new().await;
    let file = write(&world.dir(), "rates.json", &catalogue("2026-10-10", "3"));

    let run = world
        .run(&["usage", "catalogue", file.to_str().unwrap()])
        .await;

    let said = run.succeeded();
    assert!(
        said.contains("in force now") && said.contains("2026-10-10") && said.contains("2 families"),
        "{said}"
    );
    assert_eq!(
        in_force(&world).await,
        (
            "uploaded".to_owned(),
            "2026-10-10".to_owned(),
            "3".to_owned(),
            None
        ),
        "nobody signed in loaded it"
    );
    let trail = lines(&world).await;
    assert_eq!(trail.len(), 1);
    assert_eq!(trail[0].1, "system");
    assert!(
        trail[0]
            .2
            .as_deref()
            .is_some_and(|agent| agent.starts_with("orchctl "))
    );
    assert_eq!(trail[0].3["origin"], "uploaded");
    assert_eq!(trail[0].3["families"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_file_replaces_the_first_whatever_its_date_and_says_what_it_replaced() {
    let world = Deployment::new().await;
    let newer = write(&world.dir(), "newer.json", &catalogue("2026-10-10", "3"));
    let older = write(&world.dir(), "older.json", &catalogue("2026-01-01", "4"));
    world
        .run(&["usage", "catalogue", newer.to_str().unwrap()])
        .await
        .succeeded();

    let run = world
        .run(&["usage", "catalogue", older.to_str().unwrap()])
        .await;

    assert!(run.succeeded().contains("replacing the one of 2026-10-10"));
    assert_eq!(in_force(&world).await.1, "2026-01-01");
    assert_eq!(in_force(&world).await.2, "4");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalogue_with_one_bad_entry_changes_nothing_and_names_the_entry() {
    let world = Deployment::new().await;
    let good = write(&world.dir(), "good.json", &catalogue("2026-10-10", "3"));
    world
        .run(&["usage", "catalogue", good.to_str().unwrap()])
        .await
        .succeeded();
    let before = in_force(&world).await;
    let audited = lines(&world).await.len();
    let mut broken = catalogue("2026-11-11", "5");
    broken["families"][1]["rate"]["output_per_mtok"] = json!("plenty");
    let bad = write(&world.dir(), "bad.json", &broken);

    let run = world
        .run(&["usage", "catalogue", bad.to_str().unwrap()])
        .await;

    let line = run.refused_with("other-model*");
    assert!(line.contains("nothing was changed"), "{line}");
    assert_eq!(in_force(&world).await, before);
    assert_eq!(
        lines(&world).await.len(),
        audited,
        "a refusal leaves no line"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_cannot_be_a_catalogue_is_refused_in_one_line_that_says_why() {
    let world = Deployment::new().await;
    let dir = world.dir();
    let not_json = dir.join("not.json");
    std::fs::write(&not_json, "{ this is not json").unwrap();
    let wrong_format = write(&dir, "format.json", &json!({ "format": 2, "families": [] }));
    let huge = dir.join("huge.json");
    std::fs::write(&huge, vec![b' '; 3 * 1024 * 1024]).unwrap();

    world
        .run(&["usage", "catalogue", "/no/such/file.json"])
        .await
        .refused_with("cannot read /no/such/file.json");
    world
        .run(&["usage", "catalogue", not_json.to_str().unwrap()])
        .await
        .refused_with("is refused, and nothing was changed");
    world
        .run(&["usage", "catalogue", wrong_format.to_str().unwrap()])
        .await
        .refused_with("format");
    world
        .run(&["usage", "catalogue", huge.to_str().unwrap()])
        .await
        .refused_with("larger than the 2 MiB");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM rate_catalogue")
        .fetch_one(world.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalogue_of_the_release_is_put_back_where_the_release_keeps_it() {
    let world = Deployment::new().await;
    // A release is laid out as bin/, migrations/ and share/.
    let release = world.dir().join("release");
    std::fs::create_dir_all(release.join("share")).unwrap();
    write(
        &release.join("share"),
        "model-rates.json",
        &catalogue("2026-10-10", "1"),
    );
    std::fs::write(
        world.config(),
        format!(
            "[system]\nname = \"Test Orch\"\npublic_url = \"https://orch.test\"\n[database]\nmigrations_dir = \"{}\"\n",
            release.join("migrations").display()
        ),
    )
    .unwrap();
    let given = write(&world.dir(), "given.json", &catalogue("2026-12-31", "9"));
    world
        .run(&["usage", "catalogue", given.to_str().unwrap()])
        .await
        .succeeded();
    assert_eq!(in_force(&world).await.2, "9");

    let run = world.run(&["usage", "catalogue", "--shipped"]).await;

    assert!(
        run.succeeded()
            .contains("the rate catalogue of the release is in force now"),
        "{}",
        run.out
    );
    assert_eq!(
        in_force(&world).await,
        (
            "shipped".to_owned(),
            "2026-10-10".to_owned(),
            "1".to_owned(),
            None
        ),
        "an admin's file and a newer date do not stand in its way"
    );
    let trail = lines(&world).await;
    assert_eq!(trail.last().map(|line| line.1.as_str()), Some("system"));
    assert_eq!(trail.last().unwrap().3["origin"], "shipped");
}

#[tokio::test(flavor = "multi_thread")]
async fn where_the_release_keeps_its_catalogue_can_be_told_and_a_release_without_one_is_told_where_it_was_looked_for()
 {
    let world = Deployment::new().await;

    // Run from where a release puts it: a build of a checkout has the
    // repository's own catalogue two directories up, and would find that.
    let lost = support::run(world.installed(), &["usage", "catalogue", "--shipped"]).await;
    let line = lost.refused_with("was not found");
    assert!(
        line.contains("share/model-rates.json") && line.contains("--model-rates"),
        "{line}"
    );

    let elsewhere = write(
        &world.dir(),
        "elsewhere.json",
        &catalogue("2026-10-10", "6"),
    );
    let run = world
        .run(&[
            "usage",
            "catalogue",
            "--shipped",
            "--model-rates",
            elsewhere.to_str().unwrap(),
        ])
        .await;
    run.succeeded();
    assert_eq!(in_force(&world).await.0, "shipped");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_catalogue_that_ships_with_the_repository_is_one_the_command_accepts() {
    let world = Deployment::new().await;
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/model-rates.json");

    let run = world
        .run(&[
            "usage",
            "catalogue",
            "--shipped",
            "--model-rates",
            shipped.to_str().unwrap(),
        ])
        .await;

    assert!(run.succeeded().contains("is in force now"), "{}", run.out);
}

// ---- usage rebuild ----

/// Writes rows into the ledger the way the writer does, which keeps the rollups: five days of October and
/// November 2026 for one topic.
async fn spend(world: &Deployment) {
    let mut conn = world.pool().acquire().await.unwrap();
    let (org, project, topic, thread) = (new_id(), new_id(), new_id(), new_id());
    let place = Attribution {
        org_id: org,
        project_id: project,
        topic_id: topic,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: new_id(),
        entry_id: None,
        user_id: Some(new_id()),
        agent_id: None,
        harness_kind: Some("codex".to_owned()),
        environment_id: None,
        environment_kind: None,
        arrangement: None,
        turn_model: "gpt-6-sol".to_owned(),
        effort: None,
        params: Json::Null,
    };
    for (month, day, total) in [
        (10, 3, 100),
        (10, 3, 50),
        (10, 17, 200),
        (10, 25, 10),
        (10, 30, 70),
        (11, 2, 5),
    ] {
        let id: Id = new_id();
        ledger::record(
            &mut conn,
            &NewEvent {
                id,
                at: Utc.with_ymd_and_hms(2026, month, day, 12, 0, 0).unwrap(),
                origin: ledger::origin::FORWARDER,
                scope: ledger::scope::REQUEST,
                counted: true,
                attribution: &place,
                model: "gpt-6-sol",
                tier: None,
                tokens: Tokens {
                    input: total,
                    cached_input: 0,
                    cache_write: 0,
                    output: 0,
                    reasoning: 0,
                    total,
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
}

/// Both rollup tables as text, in a fixed order.
async fn rollups(world: &Deployment) -> (Vec<String>, Vec<String>) {
    let daily =
        sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_daily t ORDER BY day, 1")
            .fetch_all(world.pool())
            .await
            .unwrap();
    let monthly = sqlx::query_scalar("SELECT row_to_json(t)::text FROM usage_monthly t ORDER BY 1")
        .fetch_all(world.pool())
        .await
        .unwrap();
    (daily, monthly)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_rollups_are_made_again_from_the_ledger_alone_and_are_what_they_were() {
    let world = Deployment::new().await;
    spend(&world).await;
    let kept = rollups(&world).await;
    assert_eq!(
        (kept.0.len(), kept.1.len()),
        (5, 2),
        "four days of October and one of November"
    );
    sqlx::query("UPDATE usage_daily SET total_tokens = 0, input_tokens = 0")
        .execute(world.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM usage_monthly")
        .execute(world.pool())
        .await
        .unwrap();
    assert_ne!(rollups(&world).await, kept);

    let run = world.run(&["usage", "rebuild"]).await;

    let said = run.succeeded();
    assert!(
        said.contains("5 daily rows") && said.contains("2 monthly rows"),
        "{said}"
    );
    assert_eq!(
        rollups(&world).await,
        kept,
        "the very rows the live writer made"
    );
    let ledger_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_events")
        .fetch_one(world.pool())
        .await
        .unwrap();
    assert_eq!(ledger_rows, 6, "the ledger is what it was");
}

#[tokio::test(flavor = "multi_thread")]
async fn from_a_day_on_only_the_days_from_it_are_made_again() {
    let world = Deployment::new().await;
    spend(&world).await;
    let kept = rollups(&world).await;
    sqlx::query("UPDATE usage_daily SET total_tokens = 0, input_tokens = 0")
        .execute(world.pool())
        .await
        .unwrap();

    let run = world
        .run(&["usage", "rebuild", "--from", "2026-10-17"])
        .await;

    assert!(run.succeeded().contains("from 2026-10-17"), "{}", run.out);
    let after = rollups(&world).await;
    assert_eq!(after.0.len(), kept.0.len());
    let total_of = |row: &str| -> i64 {
        serde_json::from_str::<Json>(row).unwrap()["total_tokens"]
            .as_i64()
            .unwrap()
    };
    let totals: Vec<i64> = after.0.iter().map(|row| total_of(row)).collect();
    assert_eq!(
        totals,
        [0, 200, 10, 70, 5],
        "the third of October, before the day, stays as it was; the rest is made again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_day_that_is_not_a_day_is_refused_in_one_line_and_nothing_is_changed() {
    let world = Deployment::new().await;
    spend(&world).await;
    let kept = rollups(&world).await;

    world
        .run(&["usage", "rebuild", "--from", "yesterday"])
        .await
        .refused_with("YYYY-MM-DD");
    world
        .run(&["usage", "rebuild", "--from", "2026-13-40"])
        .await
        .refused_with("is not a day");

    assert_eq!(rollups(&world).await, kept);
}
