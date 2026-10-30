//! Sessions that are resumed in a new process, forked, adopted and closed, on the real Codex in Docker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Rig, kinds, prompt_texts, script, session_events};
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter};
use orch_agent_contract::events::{ItemContent, ItemStatus, SessionEvent, TurnOutcome};
use orch_agent_contract::session::SessionMode;
use orch_types::new_id;

fn assert_supervised_home(rig: &Rig) {
    let home = rig.home.path();
    assert!(
        !home.join("packages/app-server-daemon").exists(),
        "no daemon release is installed"
    );
    for pidfile in ["app-server.pid", "app-server-updater.pid"] {
        assert!(
            !home.join("app-server-daemon").join(pidfile).exists(),
            "no managed daemon owns this home"
        );
    }
    let settings: orch_types::Json = serde_json::from_slice(
        &std::fs::read(home.join("app-server-daemon/settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(settings["updater"]["autoUpdateEnabled"], false);
    let containers = rig.launcher.containers.lock().unwrap().clone();
    if let Some(name) = containers.last() {
        let listing = std::process::Command::new("docker")
            .args(["top", name, "-eo", "pid,args"])
            .output()
            .unwrap();
        assert!(
            listing.status.success(),
            "the pinned app-server is running: {}",
            String::from_utf8_lossy(&listing.stderr)
        );
        let processes = String::from_utf8_lossy(&listing.stdout);
        assert!(
            processes.lines().any(|line| line.contains("codex")
                && line.contains("app-server")
                && line.contains("stdio://")),
            "the actual pinned stdio app-server must be present"
        );
        assert!(!processes.contains("app-server daemon"));
        assert!(!processes.contains("codex-app-server-daemon"));
        assert!(!processes.contains("app-server-updater"));
        let features = std::process::Command::new("docker")
            .args([
                "exec",
                "-e",
                &format!("CODEX_HOME={}", home.display()),
                name,
                "codex",
                "features",
                "list",
            ])
            .output()
            .unwrap();
        assert!(
            features.status.success(),
            "the pinned build accepts the managed feature flags"
        );
        assert!(
            String::from_utf8_lossy(&features.stdout)
                .lines()
                .any(|line| line.starts_with("daemon_auto_start")
                    && line.split_whitespace().last() == Some("false")),
            "the pinned build recognizes and disables daemon auto-start"
        );
    }
}

fn record_home_footprint(home: &std::path::Path) {
    let Ok(directory) = std::env::var("ORCH_HISTORY_FOOTPRINT_DIR") else {
        return;
    };
    let mut totals = std::collections::BTreeMap::<String, u64>::new();
    for top in std::fs::read_dir(home).unwrap() {
        let top = top.unwrap();
        let name = top.file_name().to_string_lossy().into_owned();
        let mut pending = vec![top.path()];
        let mut bytes = 0;
        while let Some(path) = pending.pop() {
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.extend(
                    std::fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else if metadata.is_file() {
                bytes += metadata.len();
            }
        }
        totals.insert(name, bytes);
    }
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(std::path::Path::new(&directory).join("codex-home.json"),serde_json::to_vec_pretty(&serde_json::json!({"program":"Codex","sample":"short scripted run","at":chrono::Utc::now().to_rfc3339(),"by_directory":totals})).unwrap()).unwrap();
}

#[tokio::test]
async fn a_session_is_resumed_in_a_new_process_and_remembers_the_conversation() {
    let Some(mut rig) = Rig::start("resume").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    let first = rig.open_new(thread).await;
    rig.run_turn(thread, "remember the word banana").await;
    assert_supervised_home(&rig);
    assert!(
        !rig.home.path().join("history.jsonl").exists(),
        "prompt recall is disabled while rollouts remain resumable"
    );
    rig.adapter.close_session(thread).await.unwrap();
    rig.adapter.stop().await.unwrap();
    rig.drain();

    // A new process, a new key for the session, the same conversation.
    rig.boot().await;
    let spec = rig.spec(thread, SessionMode::Resume, Some(&first.native_session_ref));
    let key = spec.session_key.clone();
    let reopened = rig.adapter.open_session(spec).await.unwrap();
    assert_eq!(reopened.native_session_ref, first.native_session_ref);
    assert!(reopened.resumed);
    assert!(matches!(rig.next_event().await, AdapterEvent::Status(_)));
    assert!(matches!(
        rig.next_event().await,
        AdapterEvent::Session { event: SessionEvent::SessionOpened(event), .. } if event.resumed
    ));
    let events = rig
        .run_turn(thread, "what word did I ask you to remember?")
        .await;
    assert_supervised_home(&rig);
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    let requests = rig.model_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].path.contains(&key),
        "the second process reaches the model under the new key"
    );
    let said = prompt_texts(&requests[1]);
    assert!(
        said.contains(&"remember the word banana".to_owned()),
        "{said:?}"
    );
    assert!(
        said.contains(&"You said: remember the word banana".to_owned()),
        "{said:?}"
    );
    assert_eq!(
        rig.launcher.containers.lock().unwrap().len(),
        2,
        "two processes of the harness ran"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn deleting_an_owned_idle_history_frees_the_rollout_and_keeps_another_session_resumable() {
    let Some(mut rig) = Rig::start("managed-history").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    let spec = rig.spec(thread, SessionMode::New, None);
    let segment = spec.settings.segment_id;
    let opened = rig.adapter.open_session(spec).await.unwrap();
    let source = orch_agent_contract::history::NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: opened.native_session_ref.clone(),
    };
    let fresh = rig
        .adapter
        .manage_history(
            vec![orch_agent_contract::history::NativeHistorySource {
                source: source.clone(),
                started_by_orch: true,
                inventory_pass: None,
            }],
            false,
        )
        .await
        .unwrap();
    assert!(
        !fresh[0].removed,
        "a live opening is not lost before its first rollout flush: {fresh:?}"
    );
    assert!(
        rig.model_requests().is_empty(),
        "a cache observation submits no model turn"
    );
    rig.run_turn(thread, "history that may be removed").await;
    let other = new_id();
    let kept = rig.open_new(other).await;
    rig.run_turn(other, "keep this conversation").await;
    let refused = rig
        .adapter
        .manage_history(vec![source.clone().into()], true)
        .await
        .unwrap();
    assert_eq!(refused[0].error.as_deref(), Some("history.not_owned"));
    assert!(!refused[0].removed);
    let measured = rig
        .adapter
        .manage_history(
            vec![orch_agent_contract::history::NativeHistorySource {
                source: source.clone(),
                started_by_orch: true,
                inventory_pass: None,
            }],
            false,
        )
        .await
        .unwrap();
    assert!(measured[0].bytes.unwrap() > 0, "{measured:?}");
    assert!(measured[0].error.is_none(), "{measured:?}");
    record_home_footprint(rig.home.path());
    let removed = rig
        .adapter
        .manage_history(
            vec![orch_agent_contract::history::NativeHistorySource {
                source,
                started_by_orch: true,
                inventory_pass: None,
            }],
            true,
        )
        .await
        .unwrap();
    assert!(removed[0].removed, "{removed:?}");
    assert!(removed[0].error.is_none(), "{removed:?}");
    let missing = rig.spec(
        thread,
        SessionMode::Resume,
        Some(&opened.native_session_ref),
    );
    assert_eq!(
        rig.adapter.open_session(missing).await.unwrap_err().code(),
        "session_not_found"
    );
    rig.adapter.close_session(other).await.unwrap();
    let resume = rig.spec(other, SessionMode::Resume, Some(&kept.native_session_ref));
    assert!(rig.adapter.open_session(resume).await.unwrap().resumed);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_session_that_the_harness_does_not_hold_is_not_found() {
    let Some(mut rig) = Rig::start("lost-session").await else {
        return;
    };
    rig.boot().await;
    let spec = rig.spec(
        new_id(),
        SessionMode::Resume,
        Some("01a12ab3-0000-7000-8000-000000000000"),
    );
    let error = rig.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(error.code(), "session_not_found", "{error}");
    let no_ref = rig.spec(new_id(), SessionMode::Resume, None);
    assert_eq!(
        rig.adapter.open_session(no_ref).await.unwrap_err().code(),
        "not_found"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_second_session_forks_from_the_first_and_the_two_go_on_apart() {
    let Some(mut rig) = Rig::start("fork").await else {
        return;
    };
    rig.boot().await;
    let original = new_id();
    let first = rig.open_new(original).await;
    rig.run_turn(original, "first thing").await;
    rig.run_turn(original, "second thing").await;

    let side = new_id();
    let spec = rig.spec(side, SessionMode::Fork, Some(&first.native_session_ref));
    let forked = rig.adapter.open_session(spec).await.unwrap();
    assert_ne!(
        forked.native_session_ref, first.native_session_ref,
        "a fork is a session of its own"
    );
    assert!(!forked.resumed);
    rig.run_turn(side, "only in the fork").await;
    rig.run_turn(original, "back in the original").await;

    let requests = rig.model_requests();
    assert_eq!(requests.len(), 4);
    let in_fork = prompt_texts(&requests[2]);
    for earlier in ["first thing", "second thing", "only in the fork"] {
        assert!(in_fork.contains(&earlier.to_owned()), "{in_fork:?}");
    }
    let in_original = prompt_texts(&requests[3]);
    assert!(in_original.contains(&"second thing".to_owned()));
    assert!(
        !in_original.contains(&"only in the fork".to_owned()),
        "what is said in the fork does not reach the original: {in_original:?}"
    );

    // A fork through the first turn leaves the second out.
    let early = new_id();
    let mut spec = rig.spec(early, SessionMode::Fork, Some(&first.native_session_ref));
    spec.settings.upto_turn_ordinal = Some(1);
    rig.adapter.open_session(spec).await.unwrap();
    rig.run_turn(early, "from the early fork").await;
    let from_early = prompt_texts(&rig.model_requests()[4]);
    assert!(
        from_early.contains(&"first thing".to_owned()),
        "{from_early:?}"
    );
    assert!(
        !from_early.contains(&"second thing".to_owned()),
        "{from_early:?}"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_session_the_harness_already_had_is_listed_adopted_and_shown_on_the_timeline() {
    let Some(mut rig) = Rig::start("adopt").await else {
        return;
    };
    rig.boot().await;
    let earlier = new_id();
    let first = rig.open_new(earlier).await;
    rig.run_turn(earlier, "a conversation from before").await;
    rig.adapter.close_session(earlier).await.unwrap();
    rig.adapter.stop().await.unwrap();
    rig.drain();

    rig.boot().await;
    let found = rig.adapter.discover_sessions().await.unwrap();
    let ours = found
        .iter()
        .find(|session| session.native_session_ref == first.native_session_ref)
        .expect("the harness lists the thread it already has");
    assert_eq!(ours.title.as_deref(), Some("a conversation from before"));
    assert_eq!(
        ours.cwd.as_deref(),
        Some(&*rig.workspace_path().to_string_lossy())
    );
    assert!(ours.updated_at.is_some() && ours.adopted_thread_id.is_none());

    rig.drain();
    let adopting = new_id();
    let spec = rig.spec(
        adopting,
        SessionMode::Adopt,
        Some(&first.native_session_ref),
    );
    let adopted = rig.adapter.open_session(spec).await.unwrap();
    assert!(adopted.resumed);
    let events = session_events(&rig.drain());
    let kinds_seen = kinds(&events);
    assert_eq!(
        kinds_seen.first(),
        Some(&"session.opened"),
        "{kinds_seen:?}"
    );
    let message = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item
                    .payload
                    .get("text")
                    .and_then(|text| text.as_str())
                    .is_some() =>
            {
                Some(item)
            }
            _ => None,
        })
        .expect("what the agent said is shown again");
    assert_eq!(
        message.payload["text"],
        "You said: a conversation from before"
    );
    assert_eq!(message.status, ItemStatus::Completed);
    assert!(events.iter().any(|event| matches!(event, SessionEvent::ItemStarted(item)
        if item.native_ref == message.native_ref && matches!(item.content, ItemContent::AgentMessage(_)))));
    assert!(
        matches!(events.last(), Some(SessionEvent::TurnCompleted(ended)) if ended.status == TurnOutcome::Completed)
    );
    // Told again, it comes to the same: the receiver applies each event once.
    let again = rig.adapter.resync_session(adopting).await.unwrap();
    assert_eq!(again, 1);
    let events_again = session_events(&rig.drain());
    assert_eq!(
        kinds(&events_again),
        kinds(&events)[1..].to_vec(),
        "the same events, with the same identities"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_closed_session_takes_no_more_turns_and_a_turn_for_an_unknown_one_is_refused() {
    let Some(mut rig) = Rig::start("closed").await else {
        return;
    };
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    rig.adapter.close_session(thread).await.unwrap();
    let error = rig
        .adapter
        .start_turn(rig.turn(thread, "hello"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "session_not_found");
    rig.adapter.close_session(thread).await.unwrap();
    assert_eq!(
        rig.adapter
            .interrupt(thread, new_id())
            .await
            .unwrap_err()
            .code(),
        "session_not_found"
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn two_sessions_of_one_harness_run_turns_at_the_same_time_and_their_events_stay_apart() {
    let Some(mut rig) = Rig::start("concurrent").await else {
        return;
    };
    rig.boot().await;
    let (first, second) = (new_id(), new_id());
    rig.open_new(first).await;
    rig.open_new(second).await;
    rig.drain();

    // Both talk slowly, in small pieces, so that their streams overlap.
    let slow = |word: &str| {
        script(
            "go on",
            &[
                "chunk 4",
                "delay 60",
                &format!("say {word} {word} {word} {word} {word}"),
            ],
        )
    };
    let turn_a = rig.turn(first, &slow("alpha"));
    let turn_b = rig.turn(second, &slow("beta"));
    let (id_a, id_b) = (turn_a.turn_id, turn_b.turn_id);
    rig.adapter.start_turn(turn_a).await.unwrap();
    rig.adapter.start_turn(turn_b).await.unwrap();

    let mut seen: std::collections::HashMap<orch_types::Id, Vec<SessionEvent>> = Default::default();
    let mut ended = 0;
    while ended < 2 {
        if let AdapterEvent::Session {
            thread_id, event, ..
        } = rig.next_event().await
        {
            ended += usize::from(matches!(event, SessionEvent::TurnCompleted(_)));
            seen.entry(thread_id).or_default().push(event);
        }
    }
    let text_of = |events: &[SessionEvent]| -> String {
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::ItemDelta(delta) => Some(delta.data.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(text_of(&seen[&first]), "alpha alpha alpha alpha alpha");
    assert_eq!(text_of(&seen[&second]), "beta beta beta beta beta");
    for (thread, turn) in [(first, id_a), (second, id_b)] {
        let events = &seen[&thread];
        let turns: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::TurnStarted(began) => Some(began.turn_id),
                SessionEvent::TurnCompleted(done) => Some(done.turn_id),
                _ => None,
            })
            .collect();
        assert_eq!(turns, [turn, turn], "{:?}", kinds(events));
    }
    rig.adapter.stop().await.unwrap();
}
