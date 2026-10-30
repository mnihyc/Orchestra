//! Every directive of the scripted adapter, run through the adapter's own interface: what a script says is what the events say, in the order and at the pace the manual gives. Time is paused, so a pause of the script costs nothing and the moments the events come at are exact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{Fake, Seen, completed, items, model_requests, names, of, said, started};
use orch_agent_contract::adapter::{AdapterError, AgentAdapter};
use orch_agent_contract::events::{OUTPUT_INLINE_MAX_BYTES, count_diff_lines};
use orch_agent_contract::session::ApprovalDecision;
use orch_types::Json;
use serde_json::json;

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// The input of a script: the marker, then the lines.
macro_rules! script {
    ($($line:expr),+ $(,)?) => {
        format!("#!fake\n{}", [$($line.to_string()),+].join("\n"))
    };
}

fn option(id: &str) -> ApprovalDecision {
    ApprovalDecision::Option {
        option_id: id.to_owned(),
        note: None,
    }
}

fn statuses(event: &Seen) -> Vec<String> {
    event.body["payload"]["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["status"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_input_that_is_no_script_is_answered_in_three_pieces_in_three_hundred_milliseconds() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake.say(&session, "hello").await;
    assert_eq!(
        names(&events),
        [
            "turn.started",
            "model.request",
            "item.started",
            "model.request",
            "item.delta",
            "item.delta",
            "item.delta",
            "item.completed",
            "model.request",
            "turn.completed"
        ]
    );
    let pieces: Vec<&str> = of(&events, "item.delta")
        .iter()
        .map(|e| e.body["data"].as_str().unwrap())
        .collect();
    assert_eq!(pieces, ["You s", "aid: ", "hello"]);
    let offsets: Vec<i64> = of(&events, "item.delta")
        .iter()
        .map(|e| e.body["offset"].as_i64().unwrap())
        .collect();
    assert_eq!(offsets, [0, 5, 10]);
    assert_eq!(said(&events), ["You said: hello"]);
    let begun = of(&events, "item.started")[0];
    let completed = of(&events, "item.completed")[0];
    let ended = of(&events, "turn.completed")[0];
    assert_eq!(completed.at - begun.at, ms(300));
    assert_eq!(completed.body["payload"]["phase"], "final");
    assert_eq!(events[0].body["native_turn_ref"], "turn_1");
    assert_eq!(ended.body["status"], "completed");
    assert_eq!(ended.body["turn_id"], events[0].body["turn_id"]);
    assert_eq!(begun.body["stream"], "main");
    assert_eq!(begun.body["kind"], "agent_message");
    let requests = model_requests(&events);
    assert_eq!(
        requests.len(),
        3,
        "asked, first output and one terminal report, not three requests"
    );
    assert!(requests.iter().all(|request| request.id == requests[0].id));
    assert!(
        requests[0].asked.is_some()
            && requests[0].first_output.is_none()
            && requests[0].outcome.is_none()
    );
    assert!(requests[1].first_output.is_some() && requests[1].outcome.is_none());
    assert_eq!(requests[2].outcome.as_ref().unwrap().code(), "done");
    assert!(
        requests[2].tokens.output_tokens.is_none(),
        "an unscripted answer did not declare token counts"
    );
    for request in requests {
        request.check().unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn a_say_streams_forty_characters_every_hundred_milliseconds_and_only_the_last_say_is_final()
{
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let long = "0123456789".repeat(12);
    let events = fake
        .say(&session, &script!(format!("say {long}"), "say bye"))
        .await;
    let data: Vec<&str> = of(&events, "item.delta")
        .iter()
        .map(|e| e.body["data"].as_str().unwrap())
        .collect();
    assert_eq!(data, [&long[..40], &long[40..80], &long[80..], "bye"]);
    let offsets: Vec<i64> = of(&events, "item.delta")
        .iter()
        .map(|e| e.body["offset"].as_i64().unwrap())
        .collect();
    assert_eq!(offsets, [0, 40, 80, 0]);
    let begun = of(&events, "item.started");
    let ended = of(&events, "item.completed");
    assert_eq!(
        ended[0].at - begun[0].at,
        ms(300),
        "three pieces, a pause after each"
    );
    assert_eq!(ended[1].at - begun[1].at, ms(100));
    assert_eq!(ended[1].at - ended[0].at, ms(100));
    assert_eq!(ended[0].body["payload"]["phase"], "commentary");
    assert_eq!(ended[1].body["payload"]["phase"], "final");
    assert_eq!(said(&events), [long, "bye".to_owned()]);
}

#[tokio::test(start_paused = true)]
async fn a_piece_is_never_cut_inside_a_character() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let text = "é".repeat(50);
    let events = fake.say(&session, &script!(format!("say {text}"))).await;
    let deltas = of(&events, "item.delta");
    assert_eq!(deltas.len(), 2);
    assert_eq!(deltas[0].body["data"].as_str().unwrap().chars().count(), 40);
    assert_eq!(deltas[1].body["offset"], 40, "offsets count characters");
    assert_eq!(said(&events), [text]);
}

#[tokio::test(start_paused = true)]
async fn a_think_is_a_reasoning_item_whose_summary_streams_like_a_message() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("think I should read the tests first", "say done"),
        )
        .await;
    let begun = of(&events, "item.started");
    assert_eq!(begun[0].body["kind"], "reasoning");
    let delta = of(&events, "item.delta")[0];
    assert_eq!(delta.body["field"], "summary");
    let ended = of(&events, "item.completed")[0];
    assert_eq!(
        ended.body["payload"]["summary"],
        "I should read the tests first"
    );
    assert_eq!(ended.at - begun[0].at, ms(100));
}

#[tokio::test(start_paused = true)]
async fn each_directive_after_a_plan_advances_it_and_the_last_step_completes_the_item() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!(
                "plan read | write | test",
                "say one",
                "say two",
                "say three"
            ),
        )
        .await;
    let plan = of(&events, "item.started")
        .into_iter()
        .find(|e| e.body["kind"] == "plan")
        .unwrap();
    assert_eq!(statuses(plan), ["in_progress", "pending", "pending"]);
    let updates = of(&events, "item.updated");
    assert_eq!(updates.len(), 3);
    assert_eq!(
        statuses(updates[0]),
        ["completed", "in_progress", "pending"]
    );
    assert_eq!(
        statuses(updates[1]),
        ["completed", "completed", "in_progress"]
    );
    assert_eq!(
        statuses(updates[2]),
        ["completed", "completed", "completed"]
    );
    let ended = completed(&events, plan.body["native_ref"].as_str().unwrap());
    assert_eq!(ended.body["status"], "completed");
    assert_eq!(statuses(ended), ["completed", "completed", "completed"]);
    // The step moves when the directive that follows has run, not before.
    let first_say_done = events
        .iter()
        .position(|e| e.is("item.completed") && e.body["payload"]["text"] == "one")
        .unwrap();
    let first_update = events.iter().position(|e| e.is("item.updated")).unwrap();
    assert_eq!(first_update, first_say_done + 1);
}

#[tokio::test(start_paused = true)]
async fn a_plan_with_steps_left_is_done_with_the_turn_and_stays_as_it_is_when_the_turn_is_not() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(&session, &script!("plan a | b | c", "say only one"))
        .await;
    let plan_ref = items(&events)[0].0.clone();
    let ended = completed(&events, &plan_ref);
    assert_eq!(ended.body["status"], "completed");
    assert_eq!(statuses(ended), ["completed", "completed", "completed"]);

    let events = fake
        .say(
            &session,
            &script!("plan a | b | c", "say first", "fail broke it"),
        )
        .await;
    let plan_ref = items(&events)[0].0.clone();
    let ended = completed(&events, &plan_ref);
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(statuses(ended), ["completed", "in_progress", "pending"]);
    assert_eq!(events.last().unwrap().body["status"], "failed");
}

#[tokio::test(start_paused = true)]
async fn a_run_streams_its_output_and_ends_by_its_exit_code() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!(
                "run cargo test => 0 ok: 3 passed",
                "run false => 1 boom",
                "run true"
            ),
        )
        .await;
    let all = items(&events);
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|(_, kind)| kind == "tool_call"));

    let first = started(&events, &all[0].0);
    let tool = &first.body["payload"]["tool"];
    assert_eq!(tool["kind"], "command");
    assert_eq!(tool["command"], "cargo test");
    assert_eq!(tool["cwd"], fake.work_path());
    assert_eq!(tool["exit_code"], Json::Null, "no exit code while it runs");
    assert_eq!(first.body["payload"]["title"], "cargo test");
    assert_eq!(first.body["payload"]["raw"]["name"], "fake_shell");
    let ended = completed(&events, &all[0].0);
    assert_eq!(ended.body["status"], "completed");
    assert_eq!(ended.body["payload"]["tool"]["exit_code"], 0);
    assert_eq!(
        ended.body["payload"]["tool"]["output"]["inline"],
        "ok: 3 passed"
    );
    assert_eq!(ended.body["payload"]["tool"]["output"]["total_len"], 12);
    assert_eq!(ended.body["payload"]["tool"]["output"]["truncated"], false);
    assert_eq!(ended.body["payload"]["raw"]["is_error"], false);
    assert!(ended.body["payload"]["duration_ms"].as_i64().is_some());

    let second = completed(&events, &all[1].0);
    assert_eq!(second.body["status"], "failed");
    assert_eq!(second.body["payload"]["tool"]["exit_code"], 1);
    assert_eq!(second.body["payload"]["tool"]["output"]["inline"], "boom");
    assert_eq!(second.body["payload"]["raw"]["is_error"], true);

    let third = completed(&events, &all[2].0);
    assert_eq!(
        third.body["status"], "completed",
        "exit code 0 and no output when left out"
    );
    assert_eq!(third.body["payload"]["tool"]["output"]["total_len"], 0);
    let deltas: Vec<&Seen> = of(&events, "item.delta");
    assert_eq!(
        deltas.len(),
        2,
        "one piece for each of the first two, none for the third"
    );
    assert_eq!(deltas[0].body["field"], "output");
}

#[tokio::test(start_paused = true)]
async fn the_output_of_a_run_comes_in_pieces_of_forty_bytes_larger_beyond_two_thousand_never_more_than_fifty()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    for (size, pieces, each) in [
        (1_000, 25, 40),
        (2_000, 50, 40),
        (4_000, 50, 80),
        (100_000, 50, 2_000),
    ] {
        let output = "x".repeat(size);
        let events = fake
            .say(&session, &script!(format!("run dump => 0 {output}")))
            .await;
        let deltas = of(&events, "item.delta");
        assert_eq!(deltas.len(), pieces, "{size} bytes");
        assert!(
            deltas
                .iter()
                .all(|e| e.body["data"].as_str().unwrap().len() == each),
            "{size} bytes in pieces of {each}"
        );
        let last = deltas.last().unwrap();
        assert_eq!(
            last.body["offset"].as_i64().unwrap() + i64::from(u32::try_from(each).unwrap()),
            i64::try_from(size).unwrap(),
            "the offsets count bytes"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_output_over_sixteen_kib_is_kept_whole_in_the_home_and_cited_by_its_hash() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let limit = usize::try_from(OUTPUT_INLINE_MAX_BYTES).unwrap();

    let fits = "a".repeat(limit);
    let events = fake
        .say(&session, &script!(format!("run cat => 0 {fits}")))
        .await;
    let ended = of(&events, "item.completed")[0];
    assert_eq!(ended.body["payload"]["tool"]["output"]["truncated"], false);
    assert_eq!(
        ended.body.get("output_blob"),
        None,
        "what fits is not kept apart"
    );

    let big = format!("{}{}", "b".repeat(limit), "tail");
    let events = fake
        .say(&session, &script!(format!("run cat => 0 {big}")))
        .await;
    let ended = of(&events, "item.completed")[0];
    let output = &ended.body["payload"]["tool"]["output"];
    assert_eq!(output["truncated"], true);
    assert_eq!(output["total_len"], big.len());
    assert_eq!(output["inline"].as_str().unwrap().len(), limit);
    assert!(
        output["inline"].as_str().unwrap().ends_with("tail"),
        "the end is what stays inline"
    );
    let hash = output["blob"].as_str().unwrap().to_owned();
    assert_eq!(ended.body["output_blob"]["hash"], hash);
    assert_eq!(ended.body["output_blob"]["size"], big.len());
    let kept = std::fs::read_to_string(fake.home_path().join("outputs").join(&hash)).unwrap();
    assert_eq!(kept, big);
}

#[tokio::test(start_paused = true)]
async fn an_edit_begins_without_a_diff_and_completes_a_hundred_milliseconds_later_with_one() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake.say(&session, &script!("edit src/lib.rs +3 -2")).await;
    let begun = of(&events, "item.started")[0];
    let change = &begun.body["payload"]["tool"]["changes"][0];
    assert_eq!(begun.body["payload"]["tool"]["kind"], "file_change");
    assert_eq!(change["path"], "src/lib.rs");
    assert_eq!(change["diff"], Json::Null);
    let ended = of(&events, "item.completed")[0];
    assert_eq!(ended.at - begun.at, ms(100));
    assert_eq!(ended.body["status"], "completed");
    let change = &ended.body["payload"]["tool"]["changes"][0];
    assert_eq!(change["added"], 3);
    assert_eq!(change["removed"], 2);
    assert_eq!(change["truncated"], false);
    let diff = change["diff"].as_str().unwrap();
    assert_eq!(count_diff_lines(diff), (3, 2));
    assert!(diff.starts_with("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,8 +1,9 @@\n"));
    let context = diff.lines().filter(|line| line.starts_with(' ')).count();
    assert_eq!(context, 6, "three lines of context on each side");
}

#[tokio::test(start_paused = true)]
async fn an_edit_of_more_than_the_diff_can_hold_is_cut_at_a_line_and_says_so() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake.say(&session, &script!("edit big.rs +50000 -0")).await;
    let change = &of(&events, "item.completed")[0].body["payload"]["tool"]["changes"][0];
    assert_eq!(change["truncated"], true);
    let diff = change["diff"].as_str().unwrap();
    assert!(diff.ends_with('\n'), "cut at a line");
    let (added, _) = count_diff_lines(diff);
    assert_eq!(
        change["added"], added,
        "the counts are those of what is shown"
    );
    assert!(added < 50_000);
}

#[tokio::test(start_paused = true)]
async fn an_mcp_call_completes_for_a_server_the_session_was_given_and_fails_for_one_it_was_not() {
    let mut fake = Fake::start().await;
    let mut spec = fake.spec(orch_agent_contract::session::SessionMode::New);
    spec.mcp = vec!["docs".to_owned()];
    let session = fake.open(spec).await;
    let events = fake
        .say(
            &session,
            &script!(r#"mcp docs search {"q":"rust"}"#, r#"mcp other tool {}"#),
        )
        .await;
    let all = items(&events);
    let first = started(&events, &all[0].0);
    assert_eq!(first.body["payload"]["tool"]["kind"], "mcp");
    assert_eq!(
        first.body["payload"]["tool"]["arguments"],
        json!({ "q": "rust" })
    );
    assert_eq!(first.body["payload"]["raw"]["name"], "mcp__docs__search");
    let ok = completed(&events, &all[0].0);
    assert_eq!(ok.body["status"], "completed");
    assert_eq!(
        ok.body["payload"]["tool"]["result"],
        json!({ "content": [{ "type": "text", "text": "docs.search ok" }], "isError": false })
    );
    let refused = completed(&events, &all[1].0);
    assert_eq!(refused.body["status"], "failed");
    assert!(
        refused.body["payload"]["tool"]["error"]
            .as_str()
            .unwrap()
            .contains("no MCP server named \"other\"")
    );
    assert_eq!(refused.body["payload"]["raw"]["is_error"], true);
}

#[tokio::test(start_paused = true)]
async fn an_ask_for_a_command_waits_as_long_as_it_takes_and_runs_the_command_once_it_is_allowed() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("say before", "ask command rm -rf target", "say after"),
    )
    .await;
    let events = fake.until(|e| e.is("approval.requested")).await;
    let request = events.last().unwrap();
    assert_eq!(request.body["request_ref"], "req_1");
    assert_eq!(request.body["title"], "Run rm -rf target");
    assert_eq!(request.body["request"]["request_kind"], "command");
    let options: Vec<(&str, &str)> = request.body["request"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["id"].as_str().unwrap(), o["style"].as_str().unwrap()))
        .collect();
    assert_eq!(
        options,
        [("once", "allow"), ("session", "allow"), ("deny", "deny")]
    );
    assert_eq!(
        request.body["request"]["detail"]["command"],
        "rm -rf target"
    );

    let later = fake.wait(3_600_000).await;
    assert!(later.is_empty(), "the turn waits: {:?}", names(&later));

    fake.adapter
        .resolve_approval(session.thread, "req_1", option("once"))
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let ran = of(&rest, "item.completed")
        .into_iter()
        .find(|e| e.body["payload"]["tool"]["command"] == "rm -rf target")
        .unwrap();
    assert_eq!(ran.body["status"], "completed");
    assert_eq!(ran.body["payload"]["tool"]["exit_code"], 0);
    assert_eq!(said(&rest), ["after"]);
    assert_eq!(rest.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_command_that_is_denied_has_a_cancelled_call_and_did_not_run() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("ask command rm -rf target", "say went on"),
    )
    .await;
    fake.until(|e| e.is("approval.requested")).await;
    fake.adapter
        .resolve_approval(session.thread, "req_1", option("deny"))
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let call = of(&rest, "item.completed")[0];
    assert_eq!(call.body["status"], "cancelled");
    assert_eq!(
        call.body["payload"]["tool"]["exit_code"],
        Json::Null,
        "it never ran"
    );
    assert_eq!(call.body["payload"]["raw"]["is_error"], true);
    assert_eq!(said(&rest), ["went on"]);
    assert_eq!(rest.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_command_allowed_for_the_session_is_not_asked_again_in_it_but_is_in_another() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("ask command make", "ask command make"))
        .await;
    fake.until(|e| e.is("approval.requested")).await;
    fake.adapter
        .resolve_approval(session.thread, "req_1", option("session"))
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert!(
        of(&rest, "approval.requested").is_empty(),
        "the second is not asked: {:?}",
        names(&rest)
    );
    assert_eq!(of(&rest, "item.completed").len(), 2, "both ran");

    // Nor in a later turn of the same session.
    let later = fake.say(&session, &script!("ask command make")).await;
    assert!(of(&later, "approval.requested").is_empty());

    // Another session asks.
    let other = fake.session().await;
    fake.begin(&other, &script!("ask command make")).await;
    let asked = fake.until(|e| e.is("approval.requested")).await;
    assert_eq!(asked.last().unwrap().thread, Some(other.thread));
}

#[tokio::test(start_paused = true)]
async fn an_ask_for_input_poses_one_question_and_says_the_answer_back() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("ask input Which colour?"))
        .await;
    let asked = fake.until(|e| e.is("approval.requested")).await;
    let request = &asked.last().unwrap().body["request"];
    assert_eq!(request["request_kind"], "user_input");
    assert_eq!(request["detail"]["questions"][0]["id"], "q1");
    assert_eq!(
        request["detail"]["questions"][0]["question"],
        "Which colour?"
    );
    fake.adapter
        .resolve_approval(
            session.thread,
            "req_1",
            ApprovalDecision::Answers {
                answers: [("q1".to_owned(), vec!["blue".to_owned()])].into(),
                note: None,
            },
        )
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(said(&rest), ["You answered: blue"]);
}

#[tokio::test(start_paused = true)]
async fn a_decision_that_does_not_fit_is_refused_and_the_request_stays_open() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("ask command ls")).await;
    fake.until(|e| e.is("approval.requested")).await;

    let wrong_option = fake
        .adapter
        .resolve_approval(session.thread, "req_1", option("maybe"))
        .await
        .unwrap_err();
    assert_eq!(wrong_option.code(), "invalid");
    let wrong_kind = fake
        .adapter
        .resolve_approval(
            session.thread,
            "req_1",
            ApprovalDecision::Answers {
                answers: Default::default(),
                note: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_kind.code(), "invalid");
    let unknown = fake
        .adapter
        .resolve_approval(session.thread, "req_9", option("once"))
        .await
        .unwrap_err();
    assert!(matches!(unknown, AdapterError::NotFound(_)));

    fake.adapter
        .resolve_approval(session.thread, "req_1", option("once"))
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(rest.last().unwrap().body["status"], "completed");
    // It was answered once; it cannot be answered again.
    let again = fake
        .adapter
        .resolve_approval(session.thread, "req_1", option("once"))
        .await
        .unwrap_err();
    assert!(matches!(again, AdapterError::NotFound(_)));
}

#[tokio::test(start_paused = true)]
async fn a_request_that_waits_is_withdrawn_when_the_turn_is_interrupted() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let turn = fake
        .begin(&session, &script!("ask command ls", "say never"))
        .await;
    fake.until(|e| e.is("approval.requested")).await;
    fake.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert!(
        rest.iter()
            .any(|e| e.is("approval.closed") && e.body["request_ref"] == "req_1")
    );
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    assert!(said(&rest).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_sub_agent_runs_its_directives_in_a_stream_of_its_own_and_its_call_ends_with_them() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("sub scout: say found it; run ls => 0 a.txt", "say back"),
        )
        .await;
    let all = items(&events);
    let (call_ref, _) = &all[0];
    let call = started(&events, call_ref);
    assert_eq!(call.body["stream"], "main");
    assert_eq!(call.body["payload"]["tool"]["kind"], "subagent");
    assert_eq!(call.body["payload"]["tool"]["child_stream"], "sub:scout");
    assert_eq!(call.body["payload"]["tool"]["state"], "running");
    assert_eq!(call.body["payload"]["tool"]["name"], "scout");

    let inside: Vec<&(String, String)> = all[1..3].iter().collect();
    for (native_ref, _) in &inside {
        let item = started(&events, native_ref);
        assert_eq!(item.body["stream"], "sub:scout");
        assert_eq!(item.body["parent_native_ref"], *call_ref);
    }
    assert_eq!(inside[0].1, "agent_message");
    assert_eq!(inside[1].1, "tool_call");

    let update = of(&events, "item.updated")[0];
    assert_eq!(update.body["native_ref"], *call_ref);
    assert_eq!(update.body["payload"]["tool"]["state"], "completed");
    let ended = completed(&events, call_ref);
    assert_eq!(ended.body["status"], "completed");
    // What the sub-agent says is not the final message: the main stream's last say is.
    let finals: Vec<_> = of(&events, "item.completed")
        .into_iter()
        .filter(|e| e.body["payload"]["phase"] == "final")
        .collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(finals[0].body["payload"]["text"], "back");
}

#[tokio::test(start_paused = true)]
async fn a_sub_agent_that_is_interrupted_ends_interrupted_and_so_does_the_turn() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let turn = fake
        .begin(&session, &script!("sub slow: sleep 10000; say never"))
        .await;
    fake.until(|e| e.is("item.started")).await;
    fake.wait(1_000).await;
    fake.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let ended = of(&rest, "item.completed")[0];
    assert_eq!(ended.body["status"], "cancelled");
    assert_eq!(ended.body["payload"]["tool"]["state"], "interrupted");
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
}

#[tokio::test(start_paused = true)]
async fn a_tool_of_orch_that_a_directive_calls_waits_for_its_result_and_completes_with_it() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let cases: [(&str, &str, Json); 10] = [
        (
            "announce Heads up | the build is done",
            "announce",
            json!({ "title": "Heads up", "body": "the build is done" }),
        ),
        (
            "milestone Shipped",
            "announce",
            json!({ "title": "Shipped", "category": "milestone" }),
        ),
        (
            "capability exec needed to run the tests",
            "request_capability",
            json!({ "capability": "exec", "reason": "needed to run the tests" }),
        ),
        ("shared", "list_shared", json!({})),
        ("shared notes/", "list_shared", json!({ "path": "notes/" })),
        (
            "read notes/a.md 3 9 brief",
            "read_shared",
            json!({ "path": "notes/a.md", "from": 3, "to": 9, "detail": "brief" }),
        ),
        (
            "search build failures",
            "search_shared",
            json!({ "query": "build failures" }),
        ),
        (
            "download notes/a.md",
            "download_shared",
            json!({ "path": "notes/a.md" }),
        ),
        (
            "doc create docs/plan.md | # Plan",
            "edit_document",
            json!({ "command": "create", "path": "docs/plan.md", "markdown": "# Plan" }),
        ),
        (
            "chat team/general | hello all | a.txt | b.txt",
            "send_message",
            json!({ "target": { "kind": "chat", "path": "team/general" }, "text": "hello all", "files": ["a.txt", "b.txt"] }),
        ),
    ];
    for (n, (line, tool, arguments)) in cases.iter().enumerate() {
        fake.begin(&session, &script!(line)).await;
        let events = fake.until(|e| e.is("orch.tool_call")).await;
        let call = events.last().unwrap();
        assert_eq!(call.body["tool"], *tool, "{line}");
        assert_eq!(call.body["arguments"], *arguments, "{line}");
        assert_eq!(call.body["call_ref"], format!("call_{}", n + 1));
        let item = events.iter().find(|e| e.is("item.started")).unwrap();
        assert_eq!(item.body["payload"]["tool"]["kind"], "orch", "{line}");
        assert_eq!(item.body["payload"]["tool"]["tool"], *tool);
        assert_eq!(
            item.body["payload"]["raw"]["name"],
            format!("mcp__orch__{tool}")
        );
        // Nothing happens until the result comes.
        assert!(fake.wait(100_000).await.is_empty(), "{line}");
        fake.adapter
            .tool_result(
                session.thread,
                &format!("call_{}", n + 1),
                Ok(json!({ "ok": n })),
            )
            .await
            .unwrap();
        let rest = fake.until(|e| e.is("turn.completed")).await;
        let ended = of(&rest, "item.completed")[0];
        assert_eq!(ended.body["status"], "completed", "{line}");
        assert_eq!(ended.body["payload"]["tool"]["result"], json!({ "ok": n }));
        assert_eq!(rest.last().unwrap().body["status"], "completed");
    }
    // The tenth way to call one: a message to a thread.
    fake.begin(&session, &script!("tell thread-7 | see the other chat"))
        .await;
    let events = fake.until(|e| e.is("orch.tool_call")).await;
    let call = events.last().unwrap();
    assert_eq!(call.body["tool"], "send_message");
    assert_eq!(
        call.body["arguments"],
        json!({ "target": { "kind": "thread", "path": "thread-7" }, "text": "see the other chat" })
    );
}

#[tokio::test(start_paused = true)]
async fn a_refusal_of_a_tool_completes_the_call_as_failed_and_the_turn_goes_on() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("shared", "say still here"))
        .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    fake.adapter
        .tool_result(
            session.thread,
            "call_1",
            Err(orch_agent_contract::session::ToolError {
                code: "forbidden".to_owned(),
                message: "this thread shares nothing".to_owned(),
            }),
        )
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let ended = of(&rest, "item.completed")[0];
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(
        ended.body["payload"]["tool"]["result"]["error"]["code"],
        "forbidden"
    );
    assert_eq!(
        ended.body["payload"]["raw"]["output"]["inline"],
        "forbidden: this thread shares nothing"
    );
    assert_eq!(said(&rest), ["still here"]);
    assert_eq!(rest.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_call_of_a_tool_that_gets_no_result_in_two_minutes_fails_as_unreachable() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("shared", "say next")).await;
    let started = fake.until(|e| e.is("orch.tool_call")).await;
    let asked_at = started.last().unwrap().at;
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let ended = of(&rest, "item.completed")[0];
    assert_eq!(ended.at - asked_at, Duration::from_secs(120));
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(
        ended.body["payload"]["raw"]["output"]["inline"],
        "Orch is unreachable; try again later"
    );
    assert_eq!(rest.last().unwrap().body["status"], "completed");
    // A result that comes too late has no call to answer.
    let late = fake
        .adapter
        .tool_result(session.thread, "call_1", Ok(json!({})))
        .await
        .unwrap_err();
    assert!(matches!(late, AdapterError::NotFound(_)));
}

#[tokio::test(start_paused = true)]
async fn effort_model_mode_params_and_where_say_what_reached_the_harness() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("effort", "model", "mode", "params", "where"),
        )
        .await;
    assert_eq!(
        said(&events),
        [
            "effort: high".to_owned(),
            "model: fake-large".to_owned(),
            "approval: on_request, sandbox: workspace".to_owned(),
            r#"params: {"tenant":"acme"}"#.to_owned(),
            format!("directory: {0}, cwd: {0}", fake.work_path()),
        ]
    );
    let mut turn = fake.turn_of(&session, &script!("effort", "mode"));
    turn.effort = None;
    turn.approval_mode = orch_agent_contract::descriptor::ApprovalMode::Bypass;
    turn.sandbox_mode = orch_agent_contract::descriptor::SandboxMode::ReadOnly;
    fake.adapter.start_turn(turn).await.unwrap();
    let events = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(
        said(&events),
        ["effort: none", "approval: bypass, sandbox: read_only"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_sleep_waits_the_time_it_says_and_an_interrupt_ends_it() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(&session, &script!("say a", "sleep 250", "say b"))
        .await;
    let ends = of(&events, "item.completed");
    // `a` takes 100 ms, the sleep 250 ms, and `b` starts after both.
    let b_started = of(&events, "item.started")[1];
    assert_eq!(b_started.at - ends[0].at, ms(250));

    let turn = fake
        .begin(&session, &script!("sleep 3600000", "say never"))
        .await;
    fake.until(|e| e.is("turn.started")).await;
    fake.wait(5_000).await;
    fake.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    assert!(said(&rest).is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_interrupt_completes_the_item_in_the_making_as_cancelled_with_what_it_had() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let text = "0123456789".repeat(20);
    let turn = fake
        .begin(&session, &script!(format!("say {text}"), "say never"))
        .await;
    let seen = fake.until(|e| e.is("item.delta")).await;
    assert_eq!(
        seen.last().unwrap().body["data"].as_str().unwrap().len(),
        40
    );
    fake.wait(150).await;
    fake.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    let ended = of(&rest, "item.completed")[0];
    assert_eq!(ended.body["status"], "cancelled");
    let had = ended.body["payload"]["text"].as_str().unwrap();
    assert!(
        !had.is_empty() && had.len() < text.len() && text.starts_with(had),
        "{had:?}"
    );
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    // The session is whole: another turn can be started.
    let again = fake.say(&session, "again").await;
    assert_eq!(said(&again), ["You said: again"]);
}

#[tokio::test(start_paused = true)]
async fn usage_replaces_the_use_of_the_turn_and_adds_it_to_the_use_of_the_turns_before() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(&session, &script!("usage 100 50", "usage 300 60"))
        .await;
    let usage = of(&events, "usage.updated");
    assert_eq!(usage.len(), 2);
    assert_eq!(usage[0].body["turn"]["input_tokens"], 100);
    assert_eq!(usage[0].body["turn"]["output_tokens"], 50);
    assert_eq!(usage[0].body["turn"]["total_tokens"], 150);
    assert_eq!(usage[0].body["thread"]["total_tokens"], 150);
    assert_eq!(usage[0].body["context_window"], 200_000);
    assert_eq!(usage[0].body["context_used"], 100);
    assert_eq!(
        usage[1].body["turn"]["input_tokens"], 300,
        "it replaces, it does not add"
    );
    assert_eq!(usage[1].body["thread"]["total_tokens"], 360);

    let events = fake.say(&session, &script!("usage 10 5")).await;
    let usage = of(&events, "usage.updated")[0];
    assert_eq!(usage.body["turn"]["total_tokens"], 15);
    assert_eq!(
        usage.body["thread"]["total_tokens"], 375,
        "the turn before, as it ended, and this one"
    );
}

#[tokio::test(start_paused = true)]
async fn an_exhaust_fails_the_turn_and_the_providers_facts_say_the_plan_is_used_up() {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let before = fake.adapter.provider_facts().await.unwrap();
    assert!(!before.is_exhausted());
    let session = fake.session().await;
    let events = fake
        .say(&session, &script!("say begun", "exhaust", "say never"))
        .await;
    let ended = events.last().unwrap();
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(ended.body["error"]["code"], "provider_exhausted");
    assert_eq!(ended.body["error"]["retryable"], false);
    assert_eq!(said(&events), ["begun"]);
    let told = of(&events, "provider.facts");
    assert_eq!(
        told.len(),
        1,
        "the host is told that the provider's facts changed"
    );
    assert_eq!(told[0].body["limit_reached"]["type"], "usage_limit");
    assert_eq!(told[0].body["windows"][0]["used_percent"], 100.0);
    let after = fake.adapter.provider_facts().await.unwrap();
    assert!(after.is_exhausted());
}

#[tokio::test(start_paused = true)]
async fn a_fail_ends_the_turn_with_the_code_and_the_message_it_was_given() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!(
                "say starting",
                "fail rate_limited slow down, please",
                "say never"
            ),
        )
        .await;
    let ended = events.last().unwrap();
    assert!(ended.is("turn.completed"));
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(ended.body["error"]["code"], "rate_limited");
    assert_eq!(ended.body["error"]["message"], "slow down, please");
    assert_eq!(ended.body["error"]["retryable"], false);
    assert_eq!(said(&events), ["starting"]);

    let events = fake.say(&session, &script!("fail some_code")).await;
    assert_eq!(events.last().unwrap().body["error"]["code"], "some_code");
    assert!(
        !events.last().unwrap().body["error"]["message"]
            .as_str()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(start_paused = true)]
async fn an_exhaust_with_minutes_names_the_moment_the_plan_is_whole_again_in_the_error_and_in_the_facts()
 {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let session = fake.session().await;
    let events = fake.say(&session, &script!("exhaust 90")).await;
    let ended = events.last().unwrap();
    let error = &ended.body["error"];
    assert_eq!(error["code"], "provider_exhausted");
    assert_eq!(error["retryable"], false);
    // The clock of the rig stands at 2026-10-11 12:00 UTC.
    assert_eq!(error["resets_at"], "2026-10-11T13:30:00.000Z");
    assert!(
        error["said"].as_str().unwrap().contains("13:30"),
        "the vendor's words say the time: {error}"
    );
    let told = of(&events, "provider.facts");
    assert_eq!(
        told[0].body["limit_reached"]["resets_at"],
        "2026-10-11T13:30:00.000Z"
    );
    // The time stands, it does not move with the question; and when it comes the plan is whole.
    fake.clock.advance(chrono::Duration::minutes(60));
    let facts = fake.adapter.provider_facts().await.unwrap();
    assert!(facts.is_exhausted());
    assert_eq!(
        facts.limit_reached.unwrap().resets_at.unwrap().to_string(),
        "2026-10-11T13:30:00.000Z"
    );
    fake.clock.advance(chrono::Duration::minutes(31));
    assert!(!fake.adapter.provider_facts().await.unwrap().is_exhausted());
}

#[tokio::test(start_paused = true)]
async fn an_exhaust_without_minutes_lasts_one_window_of_the_plan() {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let session = fake.session().await;
    let events = fake.say(&session, &script!("exhaust")).await;
    assert_eq!(
        events.last().unwrap().body["error"]["resets_at"],
        "2026-10-11T17:00:00.000Z"
    );
}

#[tokio::test(start_paused = true)]
async fn a_limit_of_one_part_of_the_plan_fails_the_turn_and_leaves_the_plan_and_its_facts_as_they_were()
 {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("say begun", "limit opus 45", "say never"),
        )
        .await;
    let ended = events.last().unwrap();
    assert_eq!(ended.body["status"], "failed");
    let error = &ended.body["error"];
    assert_eq!(error["code"], "provider_exhausted");
    assert_eq!(error["retryable"], true, "another model may work");
    assert_eq!(error["resets_at"], "2026-10-11T12:45:00.000Z");
    assert!(error["said"].as_str().unwrap().contains("opus"), "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("opus"),
        "{error}"
    );
    assert_eq!(said(&events), ["begun"]);
    assert!(
        of(&events, "provider.facts").is_empty(),
        "the plan is not used up, so the host is told nothing of it"
    );
    assert!(!fake.adapter.provider_facts().await.unwrap().is_exhausted());
}

#[tokio::test(start_paused = true)]
async fn a_busy_and_an_unreachable_provider_fail_the_turn_with_their_own_code_and_may_be_tried_again()
 {
    let mut fake = Fake::start().await;
    fake.sign_in().await;
    let session = fake.session().await;

    let events = fake
        .say(&session, &script!("busy 529 overloaded_error"))
        .await;
    let error = &events.last().unwrap().body["error"];
    assert_eq!(error["code"], "provider_busy");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["said"], "529 overloaded_error");
    assert!(error["resets_at"].is_null());
    assert!(
        error["incomplete"].is_null(),
        "an answer that was whole says nothing of it"
    );

    let events = fake.say(&session, &script!("unreachable")).await;
    let error = &events.last().unwrap().body["error"];
    assert_eq!(error["code"], "provider_unreachable");
    assert_eq!(error["retryable"], true);
    assert!(
        error["said"]
            .as_str()
            .unwrap()
            .contains("connection refused"),
        "without words of its own it says some: {error}"
    );

    assert!(
        !fake.adapter.provider_facts().await.unwrap().is_exhausted(),
        "neither of them is held against the plan"
    );
    // And the thread tries again.
    let events = fake.say(&session, "again").await;
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn a_cut_breaks_the_stream_in_the_middle_of_an_answer_and_says_the_answer_may_be_cut_short() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let text = "The answer began well and then the connection was gone";
    let events = fake.say(&session, &script!(format!("cut {text}"))).await;
    let ended = events.last().unwrap();
    assert_eq!(ended.body["status"], "failed");
    let error = &ended.body["error"];
    assert_eq!(error["code"], "provider_unreachable");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["incomplete"], true);
    let item = events
        .iter()
        .find(|event| event.is("item.completed"))
        .unwrap();
    assert_eq!(
        item.body["status"], "cancelled",
        "the item is left as an interrupt leaves it"
    );
    let kept = item.body["payload"]["text"].as_str().unwrap();
    assert_eq!(kept, &text[..text.len() / 2]);
}

#[tokio::test(start_paused = true)]
async fn a_fail_with_words_after_a_bar_carries_them_as_what_the_vendor_said() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("fail rate_limited slow down | the weekly limit is reached"),
        )
        .await;
    let error = &events.last().unwrap().body["error"];
    assert_eq!(error["message"], "slow down");
    assert_eq!(error["said"], "the weekly limit is reached");
}

#[tokio::test(start_paused = true)]
async fn a_lose_reports_the_session_lost_ends_the_turn_without_completing_it_and_closes_the_session()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(&session, &script!("say one", "lose", "say never"))
        .await;
    assert_eq!(events.last().unwrap().name, "session.lost");
    assert!(events.iter().all(|e| !e.is("turn.completed")));
    assert_eq!(said(&events), ["one"]);
    assert!(fake.wait(10_000).await.is_empty(), "nothing follows");
    // The session is no longer open: a turn cannot be started in it...
    let turn = fake.turn_of(&session, "again");
    let refused = fake.adapter.start_turn(turn).await.unwrap_err();
    assert!(matches!(refused, AdapterError::SessionNotFound(_)));
    // ...but the harness still has its file, and the thread can be resumed from it.
    let mut spec = fake.spec(orch_agent_contract::session::SessionMode::Resume);
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    let again = fake.open(spec).await;
    assert_eq!(again.native_ref, session.native_ref);
}

#[tokio::test(start_paused = true)]
async fn a_title_a_compaction_and_a_crash_do_what_they_say() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!("title A tidy name", "compact 500", "compacted"),
        )
        .await;
    let title = of(&events, "session.title")[0];
    assert_eq!(title.body["title"], "A tidy name");
    let phases = of(&events, "turn.phase");
    assert_eq!(phases.len(), 2);
    assert_eq!(phases[0].body["kind"], "compacting");
    assert_eq!(phases[1].body["kind"], "working");
    assert_eq!(phases[1].at - phases[0].at, ms(500));
    let markers: Vec<_> = of(&events, "item.started")
        .into_iter()
        .filter(|e| e.body["kind"] == "marker")
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "only the `compacted` directive makes a marker here"
    );
    assert_eq!(markers[0].body["payload"]["marker"], "compacted");

    // A harness that does not report phases says it compacted afterwards.
    let mut quiet = Fake::start_with(json!({ "capabilities": { "phases": false } })).await;
    let session = quiet.session().await;
    let events = quiet.say(&session, &script!("compact 200")).await;
    assert!(of(&events, "turn.phase").is_empty());
    let marker = of(&events, "item.started")
        .into_iter()
        .find(|e| e.body["kind"] == "marker")
        .unwrap();
    assert_eq!(marker.at - of(&events, "turn.started")[0].at, ms(200));
}

#[tokio::test(start_paused = true)]
async fn a_crash_kills_the_harness_the_sessions_are_gone_and_it_can_be_started_again() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let other = fake.session().await;
    fake.begin(&session, &script!("say before", "crash", "say never"))
        .await;
    let events = fake.until(|e| e.is("harness.status")).await;
    assert_eq!(said(&events), ["before"]);
    let status = events.last().unwrap();
    assert_eq!(status.body["status"], "error");
    assert_eq!(status.body["sessions_open"], 0);
    assert!(
        fake.wait(10_000)
            .await
            .iter()
            .all(|e| !e.is("turn.completed")),
        "the turn ends without a word"
    );
    // Every call finds the harness gone.
    let probe = fake.adapter.probe().await.unwrap_err();
    assert_eq!(probe.code(), "harness_unavailable");
    let turn = fake.turn_of(&other, "hi");
    assert_eq!(
        fake.adapter.start_turn(turn).await.unwrap_err().code(),
        "harness_unavailable"
    );
    // Started again, it runs with the home it had, and the session it held can be resumed.
    let again = Fake::agent_start(fake.provider, fake.agent, &fake.home_path(), json!({}));
    fake.adapter.start(again, fake.sink()).await.unwrap();
    let mut spec = fake.spec(orch_agent_contract::session::SessionMode::Resume);
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    let reopened = fake.open(spec).await;
    assert_eq!(reopened.native_ref, session.native_ref);
}

#[tokio::test(start_paused = true)]
async fn something_added_to_a_running_turn_is_said_back_and_only_the_running_turn_takes_it() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let turn = fake
        .begin(&session, &script!("sleep 5000", "say done"))
        .await;
    fake.until(|e| e.is("turn.started")).await;
    fake.adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: session.thread,
            turn_id: turn,
            text: "also check the docs".to_owned(),
            images: vec![],
        })
        .await
        .unwrap();
    let steered = fake.until(|e| e.is("item.completed")).await;
    assert_eq!(
        steered.last().unwrap().body["payload"]["text"],
        "Steered: also check the docs"
    );
    assert_eq!(
        steered.last().unwrap().body["payload"]["phase"],
        "commentary"
    );
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(said(&rest), ["done"]);
    // The turn is over: there is nothing to steer.
    let late = fake
        .adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: session.thread,
            turn_id: turn,
            text: "late".to_owned(),
            images: vec![],
        })
        .await
        .unwrap_err();
    assert_eq!(late.code(), "conflict");
}

#[tokio::test(start_paused = true)]
async fn a_turn_cannot_start_while_another_runs_in_the_session() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("sleep 5000")).await;
    let second = fake.turn_of(&session, "hello");
    let refused = fake.adapter.start_turn(second).await.unwrap_err();
    assert_eq!(refused.code(), "conflict");
}

#[tokio::test(start_paused = true)]
async fn a_mistake_in_a_script_stops_the_turn_before_anything_runs_and_names_its_line() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!(
                "say this must not be said",
                "# a comment",
                "",
                "dance wildly"
            ),
        )
        .await;
    assert_eq!(
        names(&events),
        [
            "turn.started",
            "item.started",
            "item.completed",
            "turn.completed"
        ]
    );
    assert!(said(&events).is_empty());
    assert!(
        model_requests(&events).is_empty(),
        "a script rejected before execution makes no model attempt"
    );
    let error = of(&events, "item.started")[0];
    assert_eq!(error.body["kind"], "error");
    let message = error.body["payload"]["message"].as_str().unwrap();
    assert!(
        message.contains("line 5"),
        "the marker is line 1: {message}"
    );
    assert!(
        message.contains("\"dance\" is not a directive"),
        "{message}"
    );
    let ended = events.last().unwrap();
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(ended.body["error"]["code"], "harness_error");
}

#[tokio::test(start_paused = true)]
async fn the_events_of_a_busy_turn_come_in_order_and_every_item_begins_before_it_grows_and_ends() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let events = fake
        .say(
            &session,
            &script!(
                "plan look | change | check",
                "think about it",
                "say starting",
                "run ls => 0 a b c",
                "edit src/a.rs +2 -1",
                "sub helper: say hi; run pwd => 0 /w",
                "usage 10 20",
                "say finished"
            ),
        )
        .await;
    assert_eq!(events.first().unwrap().name, "turn.started");
    assert_eq!(events.last().unwrap().name, "turn.completed");
    let mut open: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for event in &events {
        let native_ref = event.body["native_ref"].as_str().map(str::to_owned);
        match event.name.as_str() {
            "item.started" => {
                let native_ref = native_ref.unwrap();
                assert!(!seen.contains(&native_ref), "{native_ref} began twice");
                seen.push(native_ref.clone());
                open.push(native_ref);
            }
            "item.delta" | "item.updated" => {
                assert!(
                    open.contains(&native_ref.unwrap()),
                    "grew before it began or after it ended"
                );
            }
            "item.completed" => {
                let native_ref = native_ref.unwrap();
                let at = open
                    .iter()
                    .position(|r| *r == native_ref)
                    .expect("ended without beginning");
                open.remove(at);
            }
            _ => {}
        }
    }
    assert!(open.is_empty(), "every item ended: {open:?}");
    let times: Vec<_> = events.iter().map(|e| e.at).collect();
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[tokio::test(start_paused = true)]
async fn params_says_the_speed_the_program_and_the_context_the_turn_was_started_with_and_the_window_it_was_given()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let mut turn = fake.turn_of(&session, &script!("params", "usage 10 5"));
    turn.params = json!({
        "service_tier": "fast", "access_program": "early", "context": 1_000_000, "context_window": 200_000
    });
    fake.adapter.start_turn(turn).await.unwrap();
    let events = fake.until(|e| e.is("turn.completed")).await;
    let said = said(&events);
    let told: serde_json::Value = serde_json::from_str(
        said[0]
            .strip_prefix("params: ")
            .expect("it says its params"),
    )
    .unwrap();
    assert_eq!(
        told,
        json!({
            "service_tier": "fast", "access_program": "early", "context": 1_000_000, "context_window": 200_000
        })
    );
    // Its usage is reported against the context it was started with, so that a meter is drawn against it.
    let usage = of(&events, "usage.updated");
    assert_eq!(usage[0].body["context_window"], 1_000_000);

    // Without a context, the window the model was given; without either, the fake's own.
    let mut turn = fake.turn_of(&session, &script!("usage 10 5"));
    turn.params = json!({ "context_window": 64_000 });
    fake.adapter.start_turn(turn).await.unwrap();
    let events = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(
        of(&events, "usage.updated")[0].body["context_window"],
        64_000
    );
}
