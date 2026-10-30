//! The directives `manage` and `confirm` of the scripted adapter: the first call of the tool that deletes, moves and brings back documents, and the second made with the word the first was given.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fake, Seen, of};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::session::ToolError;
use serde_json::{Value, json};

/// The input of a script: the marker, then the lines.
macro_rules! script {
    ($($line:expr),+ $(,)?) => {
        format!("#!fake\n{}", [$($line.to_string()),+].join("\n"))
    };
}

const WORD: &str = "0123456789abcdef0123456789abcdef";

/// The call of `manage_shared` that a script made, as the event of the adapter says it.
fn calls(events: &[Seen]) -> Vec<&Seen> {
    events
        .iter()
        .filter(|event| event.is("orch.tool_call"))
        .collect()
}

async fn answer(fake: &mut Fake, thread: orch_types::Id, call_ref: &str, result: Value) {
    fake.adapter
        .tool_result(thread, call_ref, Ok(result))
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn manage_makes_the_first_call_of_manage_shared_with_the_command_the_paths_and_where_a_move_goes()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("manage move docs/Plan.md | docs/Rollout plan.md => ../Other/docs"),
    )
    .await;
    let events = fake.until(|e| e.is("orch.tool_call")).await;
    let call = calls(&events)[0];
    assert_eq!(call.body["tool"], "manage_shared");
    assert_eq!(
        call.body["arguments"],
        json!({
            "command": "move",
            "paths": ["docs/Plan.md", "docs/Rollout plan.md"],
            "to": "../Other/docs"
        }),
        "no word the first time"
    );
    let item = events.iter().find(|e| e.is("item.started")).unwrap();
    assert_eq!(item.body["payload"]["tool"]["kind"], "orch");
    assert_eq!(item.body["payload"]["tool"]["tool"], "manage_shared");
    assert_eq!(
        item.body["payload"]["raw"]["name"],
        "mcp__orch__manage_shared"
    );
    // Nothing happens until the answer comes.
    assert!(fake.wait(100_000).await.is_empty());
    answer(
        &mut fake,
        session.thread,
        "call_1",
        json!({ "done": false }),
    )
    .await;
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(of(&rest, "item.completed")[0].body["status"], "completed");
}

#[tokio::test(start_paused = true)]
async fn confirm_makes_the_second_call_with_the_same_arguments_and_the_word_the_first_was_given() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("manage delete docs/Plan.md", "confirm", "say done"),
    )
    .await;
    let first = fake.until(|e| e.is("orch.tool_call")).await;
    assert_eq!(calls(&first)[0].body["call_ref"], "call_1");
    answer(
        &mut fake,
        session.thread,
        "call_1",
        json!({ "done": false, "confirm": WORD, "affects": [], "refused": [] }),
    )
    .await;
    let second = fake.until(|e| e.is("orch.tool_call")).await;
    let call = calls(&second)[0];
    assert_eq!(call.body["call_ref"], "call_2");
    assert_eq!(
        call.body["arguments"],
        json!({ "command": "delete", "paths": ["docs/Plan.md"], "confirm": WORD }),
        "the same command and paths, and the word that was given"
    );
    answer(
        &mut fake,
        session.thread,
        "call_2",
        json!({ "done": true, "results": [] }),
    )
    .await;
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(common::said(&rest), ["done"]);
}

#[tokio::test(start_paused = true)]
async fn a_word_is_kept_after_it_was_used_so_that_a_script_can_use_it_twice() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("manage restore docs/Plan.md", "confirm", "confirm"),
    )
    .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    answer(
        &mut fake,
        session.thread,
        "call_1",
        json!({ "done": false, "confirm": WORD }),
    )
    .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    answer(&mut fake, session.thread, "call_2", json!({ "done": true })).await;
    let third = fake.until(|e| e.is("orch.tool_call")).await;
    assert_eq!(
        calls(&third)[0].body["arguments"]["confirm"],
        WORD,
        "the second confirm sends the word again, which the system then refuses as used"
    );
}

#[tokio::test(start_paused = true)]
async fn a_confirm_after_an_answer_that_was_refused_takes_the_new_word_from_the_plan_the_refusal_holds()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(
        &session,
        &script!("manage delete docs/Plan.md", "confirm", "confirm"),
    )
    .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    answer(
        &mut fake,
        session.thread,
        "call_1",
        json!({ "done": false, "confirm": WORD }),
    )
    .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    let new_word = "fedcba9876543210fedcba9876543210";
    fake.adapter
        .tool_result(
            session.thread,
            "call_2",
            Err(ToolError {
                code: "not_confirmed".to_owned(),
                message: format!(
                    "Nothing was done. The plan as it stands now: {{\"done\":false,\"confirm\":\"{new_word}\",\"expires_in\":600}}."
                ),
            }),
        )
        .await
        .unwrap();
    let third = fake.until(|e| e.is("orch.tool_call")).await;
    assert_eq!(calls(&third)[0].body["arguments"]["confirm"], new_word);
}

#[tokio::test(start_paused = true)]
async fn a_confirm_with_nothing_to_confirm_says_so_and_calls_nothing() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    // No manage before it.
    let events = fake.script(&session, &["confirm"]).await;
    assert!(calls(&events).is_empty());
    assert_eq!(
        common::said(&events),
        ["confirm: there was no manage before it."]
    );
    // A manage whose answer gave no word.
    fake.begin(&session, &script!("manage delete docs/Plan.md", "confirm"))
        .await;
    fake.until(|e| e.is("orch.tool_call")).await;
    fake.adapter
        .tool_result(
            session.thread,
            "call_1",
            Err(ToolError {
                code: "not_manageable".to_owned(),
                message: "You may read docs/Plan.md and not manage it.".to_owned(),
            }),
        )
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert!(calls(&rest).is_empty(), "no second call was made");
    assert_eq!(
        common::said(&rest),
        ["confirm: the last answer of manage_shared gave no word to confirm with."]
    );
}

#[tokio::test(start_paused = true)]
async fn a_mistake_in_a_manage_line_stops_the_turn_before_anything_runs_and_names_its_line() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    for (line, why) in [
        ("manage", "manage is:"),
        ("manage delete", "manage is:"),
        ("confirm now", "confirm takes nothing more"),
    ] {
        let events = fake.say(&session, &script!("say never said", line)).await;
        assert!(
            common::said(&events).is_empty(),
            "{line}: nothing ran before the mistake"
        );
        let error = of(&events, "item.started")[0];
        assert_eq!(error.body["kind"], "error", "{line}");
        let message = error.body["payload"]["message"].as_str().unwrap();
        assert!(
            message.contains("line 3") && message.contains(why),
            "{line}: {message}"
        );
        assert_eq!(
            events.last().unwrap().body["error"]["code"],
            "harness_error"
        );
    }
}
