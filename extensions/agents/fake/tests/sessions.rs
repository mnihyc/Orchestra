//! Sessions of the scripted adapter: each is a file of the harness's home, so it persists, resumes, forks, exports, imports and says itself again, and two of them never see each other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fake, Seen, completed, items, model_requests, names, of, said};
use orch_agent_contract::adapter::{AdapterError, AgentAdapter};
use orch_agent_contract::descriptor::MemoryKind;
use orch_agent_contract::history::{ExportKind, Fidelity, NEUTRAL_FORMAT};
use orch_agent_contract::session::{
    LocalFile, NoteKeeper, SessionMemory, SessionMode, memory_directory,
};
use orch_types::Json;
use serde_json::json;

/// The input of a script: the marker, then the lines.
macro_rules! script {
    ($($line:expr),+ $(,)?) => {
        format!("#!fake\n{}", [$($line.to_string()),+].join("\n"))
    };
}

/// What a list of events comes to, to compare two histories said by two sessions.
fn gist(events: &[Seen]) -> Vec<(String, Json)> {
    events
        .iter()
        .map(|event| (event.name.clone(), event.body.clone()))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_session_is_a_file_of_the_home_written_as_its_items_end() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    assert_eq!(session.native_ref, "fake_1");
    let file = fake.file("fake_1");
    assert_eq!(file["format"], "fake-v1");
    assert_eq!(
        file["turns"],
        json!([]),
        "a session just opened has said nothing"
    );
    assert_eq!(file["directory"], fake.work_path());
    assert_eq!(file["model"], "fake-large");
    assert_eq!(file["instructions"], "You are working inside Orch.");

    fake.say(&session, "hello").await;
    let file = fake.file("fake_1");
    assert_eq!(file["turns"].as_array().unwrap().len(), 1);
    assert_eq!(file["turns"][0]["input"], "hello");
    assert_eq!(file["turns"][0]["status"], "completed");
    assert_eq!(file["turns"][0]["native_turn_ref"], "turn_1");
    assert_eq!(file["items"].as_array().unwrap().len(), 1);
    assert_eq!(file["items"][0]["kind"], "agent_message");
    assert_eq!(file["items"][0]["payload"]["text"], "You said: hello");
    assert_eq!(file["counters"]["item"], 1);

    // An item is kept when it is final, not when the turn is over.
    fake.begin(&session, &script!("say first", "sleep 3600000"))
        .await;
    fake.until(|e| e.is("item.completed")).await;
    let file = fake.file("fake_1");
    assert_eq!(file["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        file["turns"].as_array().unwrap().len(),
        1,
        "the turn has not ended"
    );
}

#[tokio::test(start_paused = true)]
async fn native_references_go_on_from_where_the_home_was_left() {
    let mut first = Fake::start().await;
    let a = first.session().await;
    let b = first.session().await;
    assert_eq!(
        (a.native_ref.as_str(), b.native_ref.as_str()),
        ("fake_1", "fake_2")
    );
    first.say(&a, "kept").await;
    first.adapter.stop().await.unwrap();
    let Fake { home, .. } = first;

    let mut second = Fake::start_in(home, json!({})).await;
    let c = second.session().await;
    assert_eq!(c.native_ref, "fake_3", "numbered on, not from the start");
    // What the first run kept is there to resume.
    let again = second.resume("fake_1").await;
    assert_eq!(again.native_ref, "fake_1");
}

#[tokio::test(start_paused = true)]
async fn a_resumed_session_has_its_history_and_the_numbering_goes_on_where_it_stopped() {
    let mut first = Fake::start().await;
    let session = first.session().await;
    first.say(&session, "one").await;
    first
        .say(&session, &script!("run ls => 0 a", "say two"))
        .await;
    let before = first.file("fake_1");
    assert_eq!(before["counters"]["item"], 3);
    first.adapter.close_session(session.thread).await.unwrap();
    let Fake { home, .. } = first;

    let mut second = Fake::start_in(home, json!({})).await;
    let mut spec = second.spec(SessionMode::Resume);
    spec.settings.native_session_ref = Some("fake_1".to_owned());
    let (resumed, opened) = second.open_full(spec).await;
    assert!(opened.resumed);
    assert_eq!(opened.native_session_ref, "fake_1");
    assert!(opened.import_report.is_none());

    let events = second.say(&resumed, "three").await;
    assert_eq!(
        events[0].body["native_turn_ref"], "turn_3",
        "the third turn of the session"
    );
    assert_eq!(items(&events)[0].0, "item_4", "no reference is used twice");
    let after = second.file("fake_1");
    assert_eq!(after["turns"].as_array().unwrap().len(), 3);
    assert_eq!(after["items"].as_array().unwrap().len(), 4);
    assert_eq!(
        after["items"][0], before["items"][0],
        "what was said stays as it was"
    );
}

#[tokio::test(start_paused = true)]
async fn a_session_that_was_closed_is_resumed_in_the_same_adapter_and_what_is_told_to_it_replaces_what_it_was_told()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.say(&session, "hello").await;
    fake.adapter.close_session(session.thread).await.unwrap();
    let mut spec = fake.spec(SessionMode::Resume);
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    spec.settings.model = "fake-small".to_owned();
    spec.settings.cwd = format!("{}/sub", fake.work_path());
    let resumed = fake.open(spec).await;
    let events = fake.say(&resumed, &script!("model", "where")).await;
    assert_eq!(
        said(&events),
        [
            "model: fake-large".to_owned(),
            format!("directory: {0}, cwd: {0}/sub", fake.work_path())
        ],
        "the turn states its model; the session its place"
    );
    let file = fake.file(&session.native_ref);
    assert_eq!(file["model"], "fake-small");
}

#[tokio::test(start_paused = true)]
async fn a_session_the_home_does_not_have_cannot_be_resumed() {
    let mut fake = Fake::start().await;
    let mut spec = fake.spec(SessionMode::Resume);
    spec.settings.native_session_ref = Some("fake_99".to_owned());
    let error = fake.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(error, AdapterError::SessionNotFound("fake_99".to_owned()));
    let spec = fake.spec(SessionMode::Resume);
    let error = fake.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(error.code(), "not_found", "a resume names the session");
    let mut spec = fake.spec(SessionMode::Fork);
    spec.settings.native_session_ref = Some("fake_99".to_owned());
    let error = fake.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(error.code(), "session_not_found");
}

#[tokio::test(start_paused = true)]
async fn an_adopted_session_is_one_the_harness_already_had() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.say(&session, "from before").await;
    fake.adapter.close_session(session.thread).await.unwrap();
    let mut spec = fake.spec(SessionMode::Adopt);
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    let (adopted, opened) = fake.open_full(spec).await;
    assert!(opened.resumed);
    assert_eq!(adopted.native_ref, session.native_ref);
}

#[tokio::test(start_paused = true)]
async fn opening_a_session_for_a_thread_that_has_one_ends_the_turn_the_first_was_running() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("sleep 3600000")).await;
    fake.until(|e| e.is("turn.started")).await;
    let mut spec = fake.spec(SessionMode::Resume);
    spec.settings.thread_id = session.thread;
    spec.settings.segment_id = session.segment;
    spec.settings.native_session_ref = Some(session.native_ref.clone());
    fake.adapter.open_session(spec).await.unwrap();
    let events = fake.quiet().await;
    assert_eq!(
        names(&events),
        [
            "model.request",
            "model.request",
            "turn.completed",
            "session.opened"
        ]
    );
    assert_eq!(
        of(&events, "turn.completed")[0].body["status"],
        "interrupted"
    );
    let requests = model_requests(&events);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].id, requests[1].id);
    assert!(requests[0].outcome.is_none());
    assert_eq!(requests[1].outcome.as_ref().unwrap().code(), "forced");
    assert!(requests[1].first_output.is_none());
}

#[tokio::test(start_paused = true)]
async fn closing_a_session_ends_its_turn_keeps_its_file_and_is_done_once_or_twice_alike() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    fake.begin(&session, &script!("say begun", "sleep 3600000"))
        .await;
    fake.until(|e| e.is("item.completed")).await;
    fake.adapter.close_session(session.thread).await.unwrap();
    let events = fake.quiet().await;
    assert_eq!(events.last().unwrap().name, "turn.completed");
    assert_eq!(events.last().unwrap().body["status"], "interrupted");
    assert!(fake.file_of(&session.native_ref).is_file());
    let turn = fake.turn_of(&session, "hello");
    assert!(matches!(
        fake.adapter.start_turn(turn).await.unwrap_err(),
        AdapterError::SessionNotFound(_)
    ));
    fake.adapter.close_session(session.thread).await.unwrap();
    let file = fake.file(&session.native_ref);
    assert_eq!(
        file["turns"][0]["status"], "interrupted",
        "the interrupted turn is kept as it ended"
    );
}

#[tokio::test(start_paused = true)]
async fn a_fork_copies_the_history_through_the_turn_asked_for_into_a_session_of_its_own() {
    let mut fake = Fake::start().await;
    let source = fake.session().await;
    for input in ["one", "two", "three"] {
        fake.say(&source, input).await;
    }
    let mut spec = fake.spec(SessionMode::Fork);
    spec.settings.native_session_ref = Some(source.native_ref.clone());
    spec.settings.upto_turn_ordinal = Some(2);
    let (fork, opened) = fake.open_full(spec).await;
    assert!(!opened.resumed, "a fork is a new session");
    assert_ne!(fork.native_ref, source.native_ref);
    let copy = fake.file(&fork.native_ref);
    assert_eq!(copy["turns"].as_array().unwrap().len(), 2);
    let kept: Vec<&str> = copy["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["payload"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(kept, ["You said: one", "You said: two"]);

    // It goes on from there, with a number of its own, and the source does not hear of it.
    let events = fake.say(&fork, "four").await;
    assert_eq!(events[0].body["native_turn_ref"], "turn_3");
    assert_eq!(
        items(&events)[0].0,
        "item_4",
        "nothing said later in the source is numbered again"
    );
    assert_eq!(
        fake.file(&source.native_ref)["turns"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        fake.file(&fork.native_ref)["turns"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        fake.file(&source.native_ref)["items"][2]["payload"]["text"],
        "You said: three"
    );
    assert_eq!(
        fake.file(&fork.native_ref)["items"][2]["payload"]["text"],
        "You said: four"
    );
}

#[tokio::test(start_paused = true)]
async fn a_fork_without_a_turn_to_stop_at_has_the_whole_history() {
    let mut fake = Fake::start().await;
    let source = fake.session().await;
    fake.say(&source, "one").await;
    fake.say(&source, "two").await;
    let mut spec = fake.spec(SessionMode::Fork);
    spec.settings.native_session_ref = Some(source.native_ref.clone());
    let fork = fake.open(spec).await;
    assert_eq!(
        fake.file(&fork.native_ref)["turns"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn a_native_export_imports_into_a_session_of_another_home_with_the_same_history() {
    let mut source = Fake::start().await;
    let session = source.session().await;
    source
        .say(
            &session,
            &script!(
                "title Kept",
                "say one",
                "run ls => 0 a b",
                "sub helper: say hi",
                "edit src/a.rs +1 -1"
            ),
        )
        .await;
    source.say(&session, "second").await;
    let archive = source
        .adapter
        .export_session(session.thread, ExportKind::Native, None)
        .await
        .unwrap();
    assert_eq!(archive.format, "fake-v1");
    assert!(archive.path.contains("exports"), "{}", archive.path);
    assert_eq!(
        usize::try_from(archive.size).unwrap(),
        std::fs::metadata(&archive.path).unwrap().len() as usize
    );

    let mut target = Fake::start().await;
    let mut spec = target.spec(SessionMode::ImportNative);
    spec.archive = Some(archive.clone());
    let (imported, opened) = target.open_full(spec).await;
    assert!(!opened.resumed);
    let report = opened.import_report.unwrap();
    assert_eq!(report.fidelity, Fidelity::Lossless);
    let items_in_source = source.file(&session.native_ref)["items"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(report.items_total, i64::try_from(items_in_source).unwrap());
    assert!(report.dropped.is_empty());

    // Said again, the two histories are the same.
    source.adapter.resync_session(session.thread).await.unwrap();
    let said_by_source = source.queued();
    target
        .adapter
        .resync_session(imported.thread)
        .await
        .unwrap();
    let said_by_target = target.queued();
    assert!(!said_by_source.is_empty());
    assert_eq!(gist(&said_by_source), gist(&said_by_target));

    // And the conversation goes on in the new home.
    let events = target.say(&imported, "third").await;
    assert_eq!(events[0].body["native_turn_ref"], "turn_3");
    assert_eq!(said(&events), ["You said: third"]);
}

#[tokio::test(start_paused = true)]
async fn a_native_import_can_stop_at_a_turn_and_refuses_what_is_no_session_of_the_fake() {
    let mut source = Fake::start().await;
    let session = source.session().await;
    for input in ["one", "two", "three"] {
        source.say(&session, input).await;
    }
    let archive = source
        .adapter
        .export_session(session.thread, ExportKind::Native, Some(2))
        .await
        .unwrap();
    let exported: Json = serde_json::from_slice(&std::fs::read(&archive.path).unwrap()).unwrap();
    assert_eq!(
        exported["turns"].as_array().unwrap().len(),
        2,
        "the export stops at the turn it is asked to"
    );

    let mut target = Fake::start().await;
    let mut spec = target.spec(SessionMode::ImportNative);
    spec.archive = Some(archive);
    spec.settings.upto_turn_ordinal = Some(1);
    let imported = target.open(spec).await;
    assert_eq!(
        target.file(&imported.native_ref)["turns"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let mut spec = target.spec(SessionMode::ImportNative);
    let error = target.adapter.open_session(spec.clone()).await.unwrap_err();
    assert_eq!(error.code(), "import_native_failed", "no archive was given");
    let bogus = target.home_path().join("bogus.json");
    std::fs::write(&bogus, r#"{"format":"codex-rollout"}"#).unwrap();
    spec.archive = Some(orch_agent_contract::history::SessionArchive {
        format: "codex-rollout".to_owned(),
        path: bogus.display().to_string(),
        size: 0,
    });
    let error = target.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(
        error.code(),
        "import_native_failed",
        "the engine falls back to a neutral import"
    );
}

#[tokio::test(start_paused = true)]
async fn a_neutral_export_is_the_main_stream_as_lines_and_imports_as_items_with_a_report() {
    let mut source = Fake::start().await;
    let session = source.session().await;
    source
        .say(
            &session,
            &script!("say one", "sub helper: say hidden", "run ls => 0 a b"),
        )
        .await;
    source.say(&session, "two").await;
    let archive = source
        .adapter
        .export_session(session.thread, ExportKind::Neutral, None)
        .await
        .unwrap();
    assert_eq!(archive.format, NEUTRAL_FORMAT);
    let text = std::fs::read_to_string(&archive.path).unwrap();
    let lines: Vec<Json> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let kinds: Vec<&str> = lines
        .iter()
        .map(|line| line["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "user_message",
            "agent_message",
            "tool_call",
            "tool_call",
            "user_message",
            "agent_message"
        ],
        "oldest first, a turn's input before what it said, the sub-agent's own items not at all"
    );
    assert!(lines.iter().all(|line| line["stream"] == "main"));
    assert_eq!(
        lines[0]["payload"]["parts"][0]["text"],
        script!("say one", "sub helper: say hidden", "run ls => 0 a b")
    );

    let mut target = Fake::start().await;
    let mut spec = target.spec(SessionMode::ImportNeutral);
    spec.history = Some(LocalFile {
        path: archive.path.clone(),
        size: archive.size,
    });
    let (imported, opened) = target.open_full(spec).await;
    let report = opened.import_report.unwrap();
    assert_eq!(report.fidelity, Fidelity::Structured);
    assert_eq!(report.by_kind["user_message"].kept, 2);
    assert_eq!(report.by_kind["agent_message"].converted, 2);
    assert_eq!(report.by_kind["tool_call"].converted, 2);
    assert!(
        !report.warnings.is_empty(),
        "it says that what the model saw is not there"
    );
    let file = target.file(&imported.native_ref);
    assert_eq!(file["turns"].as_array().unwrap().len(), 2);
    assert_eq!(file["turns"][1]["input"], "two");
    assert_eq!(file["items"].as_array().unwrap().len(), 4);

    let events = target.say(&imported, "three").await;
    assert_eq!(events[0].body["native_turn_ref"], "turn_3");
    assert_eq!(said(&events), ["You said: three"]);
}

#[tokio::test(start_paused = true)]
async fn a_neutral_history_with_what_the_fake_cannot_keep_says_what_was_left_out_and_why() {
    let mut fake = Fake::start().await;
    let path = fake.home_path().join("history.jsonl");
    let lines = [
        json!({ "kind": "user_message", "status": "completed", "stream": "main", "payload": { "parts": [{ "type": "text", "text": "hi" }] } }),
        json!({ "kind": "agent_message", "status": "completed", "stream": "main", "payload": { "text": "hello" } }),
        json!({ "kind": "agent_message", "status": "completed", "stream": "sub:x", "payload": { "text": "inside" } }),
        json!({ "kind": "web_search", "status": "completed", "stream": "main", "payload": {} }),
    ];
    std::fs::write(
        &path,
        lines
            .iter()
            .map(Json::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let mut spec = fake.spec(SessionMode::ImportNeutral);
    spec.history = Some(LocalFile {
        path: path.display().to_string(),
        size: 0,
    });
    let (imported, opened) = fake.open_full(spec).await;
    let report = opened.import_report.unwrap();
    let dropped: Vec<(&str, &str)> = report
        .dropped
        .iter()
        .map(|d| (d.kind.as_str(), d.reason.as_str()))
        .collect();
    assert_eq!(
        dropped,
        [
            (
                "agent_message",
                "the neutral history is the main stream only"
            ),
            ("web_search", "the fake harness keeps no item of this kind"),
        ]
    );
    assert_eq!(
        fake.file(&imported.native_ref)["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn a_resync_says_again_every_item_of_every_turn_and_the_items_of_a_turn_that_is_running() {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let first = fake.say(&session, &script!("say a", "run ls => 0 x")).await;
    let second = fake.say(&session, "b").await;
    let seen_live: Vec<_> = first
        .iter()
        .chain(second.iter())
        .filter(|e| e.is("item.completed") || e.is("turn.completed"))
        .map(|e| (e.name.clone(), e.body.clone()))
        .collect();
    let said = fake.adapter.resync_session(session.thread).await.unwrap();
    let again = fake.queued();
    assert_eq!(said, 3, "one for each item");
    assert_eq!(
        gist(&again),
        seen_live,
        "exactly what was said when it was made, turn by turn"
    );

    // A turn under way: its items that are final are said, its end is not.
    fake.begin(&session, &script!("say c", "sleep 3600000"))
        .await;
    fake.until(|e| e.is("item.completed")).await;
    let said = fake.adapter.resync_session(session.thread).await.unwrap();
    let again = fake.queued();
    assert_eq!(said, 4);
    assert_eq!(
        names(&again)
            .iter()
            .filter(|n| **n == "turn.completed")
            .count(),
        2
    );
    assert_eq!(names(&again).last().copied(), Some("item.completed"));
}

#[tokio::test(start_paused = true)]
async fn the_sessions_of_a_home_are_listed_newest_first_with_their_titles_and_places() {
    let mut fake = Fake::start().await;
    let a = fake.session().await;
    fake.say(&a, &script!("title The first one", "say a")).await;
    fake.clock.advance(chrono::Duration::seconds(10));
    let b = fake.session().await;
    fake.say(&b, "b").await;
    let listed = fake.adapter.discover_sessions().await.unwrap();
    let order: Vec<&str> = listed
        .iter()
        .map(|s| s.native_session_ref.as_str())
        .collect();
    assert_eq!(order, [b.native_ref.as_str(), a.native_ref.as_str()]);
    assert_eq!(listed[1].title.as_deref(), Some("The first one"));
    assert_eq!(listed[0].title, None);
    assert_eq!(listed[0].cwd.as_deref(), Some(fake.work_path().as_str()));
    fake.clock.advance(chrono::Duration::seconds(10));
    fake.say(&a, "again").await;
    let listed = fake.adapter.discover_sessions().await.unwrap();
    assert_eq!(
        listed[0].native_session_ref, a.native_ref,
        "the one that changed last is first"
    );
}

#[tokio::test(start_paused = true)]
async fn two_sessions_of_one_home_are_two_files_and_neither_sees_the_other() {
    let mut fake = Fake::start().await;
    let a = fake.session().await;
    let b = fake.session().await;
    assert_ne!(fake.file_of(&a.native_ref), fake.file_of(&b.native_ref));

    fake.begin(&a, &script!("shared", "ask command ls")).await;
    let events = fake.until(|e| e.is("orch.tool_call")).await;
    assert!(
        events.iter().all(|e| e.thread == Some(a.thread)),
        "only A speaks"
    );
    assert!(!of(&events, "model.request").is_empty());
    assert!(
        of(&events, "model.request")
            .iter()
            .all(|event| event.segment == Some(a.segment)),
        "request reports keep A's segment too"
    );
    fake.say(&b, "b says this").await;
    assert_eq!(
        fake.file(&b.native_ref)["items"][0]["payload"]["text"],
        "You said: b says this"
    );
    assert!(
        fake.file(&a.native_ref)["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["payload"]["text"] != "You said: b says this"),
        "B's words are not in A's file"
    );

    // A's call and A's question are A's: B's thread finds neither.
    let result = fake
        .adapter
        .tool_result(b.thread, "call_1", Ok(json!({})))
        .await
        .unwrap_err();
    assert!(matches!(result, AdapterError::NotFound(_)));
    fake.adapter
        .tool_result(a.thread, "call_1", Ok(json!({})))
        .await
        .unwrap();
    fake.until(|e| e.is("approval.requested")).await;
    let decision = orch_agent_contract::session::ApprovalDecision::Option {
        option_id: "once".to_owned(),
        note: None,
    };
    assert!(matches!(
        fake.adapter
            .resolve_approval(b.thread, "req_1", decision.clone())
            .await
            .unwrap_err(),
        AdapterError::NotFound(_)
    ));
    fake.adapter
        .resolve_approval(a.thread, "req_1", decision)
        .await
        .unwrap();
    let rest = fake.until(|e| e.is("turn.completed")).await;
    assert_eq!(rest.last().unwrap().thread, Some(a.thread));

    // The counters are each session's own.
    let events = fake.say(&b, "again").await;
    assert_eq!(items(&events)[0].0, "item_2");
}

#[tokio::test(start_paused = true)]
async fn the_session_of_a_thread_is_told_what_it_was_opened_with_and_a_session_that_is_not_open_is_not_found()
 {
    let mut fake = Fake::start().await;
    let session = fake.session().await;
    let stranger = orch_types::new_id();
    let mut turn = fake.turn_of(&session, "hi");
    turn.thread_id = stranger;
    assert!(matches!(
        fake.adapter.start_turn(turn).await.unwrap_err(),
        AdapterError::SessionNotFound(_)
    ));
    let exported = fake
        .adapter
        .export_session(stranger, ExportKind::Native, None)
        .await
        .unwrap_err();
    assert!(matches!(exported, AdapterError::SessionNotFound(_)));
    assert!(matches!(
        fake.adapter.resync_session(stranger).await.unwrap_err(),
        AdapterError::SessionNotFound(_)
    ));
    assert!(matches!(
        fake.adapter
            .interrupt(stranger, orch_types::new_id())
            .await
            .unwrap_err(),
        AdapterError::SessionNotFound(_)
    ));
}

// ---------------------------------------------------------------------------------------------------------
// Notes

fn memory(fake: &Fake, kept_by: NoteKeeper) -> SessionMemory {
    SessionMemory {
        directory: memory_directory(&fake.work_path()),
        kept_by,
    }
}

fn notes_file(fake: &Fake) -> std::path::PathBuf {
    std::path::PathBuf::from(memory_directory(&fake.work_path())).join("MEMORY.md")
}

async fn session_with_notes(fake: &mut Fake, kept_by: Option<NoteKeeper>) -> common::Session {
    let mut spec = fake.spec(SessionMode::New);
    spec.settings.memory = kept_by.map(|keeper| memory(fake, keeper));
    fake.open(spec).await
}

#[tokio::test(start_paused = true)]
async fn a_session_told_nothing_of_notes_keeps_none_and_says_so() {
    let mut fake = Fake::start().await;
    let session = session_with_notes(&mut fake, None).await;
    let events = fake
        .say(&session, &script!("remember the build needs --release"))
        .await;
    assert_eq!(said(&events), ["Not kept: notes are off for this session."]);
    assert!(!notes_file(&fake).exists());
}

#[tokio::test(start_paused = true)]
async fn the_notes_the_harness_keeps_are_lines_of_the_index_and_the_next_session_reads_them_back() {
    let mut fake = Fake::start().await;
    let session = session_with_notes(&mut fake, Some(NoteKeeper::Harness)).await;
    let events = fake
        .say(
            &session,
            &script!(
                "notes",
                "remember the build needs --release",
                "remember tests are in tests/"
            ),
        )
        .await;
    assert_eq!(
        said(&events),
        [
            "notes: none",
            "Noted: the build needs --release",
            "Noted: tests are in tests/"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(notes_file(&fake)).unwrap(),
        "- the build needs --release\n- tests are in tests/\n"
    );
    // A later session in the same directory has them when it opens.
    let later = session_with_notes(&mut fake, Some(NoteKeeper::Harness)).await;
    let events = fake.say(&later, &script!("notes")).await;
    assert_eq!(
        said(&events),
        ["notes: 2 loaded: the build needs --release; tests are in tests/"]
    );
}

#[tokio::test(start_paused = true)]
async fn when_the_agent_keeps_the_notes_the_write_is_a_tool_call_it_makes_and_nothing_is_loaded_for_it()
 {
    let mut fake = Fake::start().await;
    let session = session_with_notes(&mut fake, Some(NoteKeeper::Agent)).await;
    let events = fake.say(&session, &script!("remember keep this")).await;
    assert!(
        said(&events).is_empty(),
        "no `Noted`: the agent's own tool did it"
    );
    let all = items(&events);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].1, "tool_call");
    let ended = completed(&events, &all[0].0);
    let change = &ended.body["payload"]["tool"]["changes"][0];
    assert_eq!(ended.body["payload"]["tool"]["kind"], "file_change");
    assert_eq!(
        change["path"],
        format!("{}/MEMORY.md", memory_directory(&fake.work_path()))
    );
    assert_eq!(change["added"], 1);
    assert_eq!(
        std::fs::read_to_string(notes_file(&fake)).unwrap(),
        "- keep this\n"
    );
    let later = session_with_notes(&mut fake, Some(NoteKeeper::Agent)).await;
    let events = fake.say(&later, &script!("notes")).await;
    assert_eq!(
        said(&events),
        ["notes: none"],
        "the harness loads nothing of notes it does not keep"
    );
}

#[tokio::test(start_paused = true)]
async fn a_harness_with_one_memory_for_its_whole_home_or_none_never_writes_the_notes_it_was_given()
{
    for (kind, kept_by, line) in [
        (
            "home",
            NoteKeeper::Harness,
            "Not kept: this harness has one memory for its whole home, and that is off.",
        ),
        (
            "home",
            NoteKeeper::Agent,
            "Not kept: this harness has one memory for its whole home, and that is off.",
        ),
        (
            "none",
            NoteKeeper::Harness,
            "Not kept: this harness has no memory.",
        ),
    ] {
        let mut fake = Fake::start_with(json!({ "memory": kind })).await;
        assert_eq!(
            fake.started.descriptor.capabilities.memory,
            match kind {
                "home" => MemoryKind::Home,
                _ => MemoryKind::None,
            }
        );
        let session = session_with_notes(&mut fake, Some(kept_by)).await;
        let events = fake.say(&session, &script!("remember something")).await;
        assert_eq!(said(&events), [line], "{kind} / {kept_by:?}");
        assert!(!notes_file(&fake).exists());
    }
    // A harness with no memory of its own still lets the agent write the notes with its tools.
    let mut fake = Fake::start_with(json!({ "memory": "none" })).await;
    let session = session_with_notes(&mut fake, Some(NoteKeeper::Agent)).await;
    fake.say(&session, &script!("remember by hand")).await;
    assert_eq!(
        std::fs::read_to_string(notes_file(&fake)).unwrap(),
        "- by hand\n"
    );
}
