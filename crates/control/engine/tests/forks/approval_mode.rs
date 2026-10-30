//! A switch previews and commits an offered approval mode without losing the carried conversation.

use super::*;
use orch_agent_contract::descriptor::ApprovalMode;
use orch_settings_contract::registry::keys;

async fn reduced_modes_rig() -> Rig {
    Rig::with_kinds(
        Can {
            export_native: true,
            import_native: true,
            ..Can::default()
        },
        "codex",
        "dsh",
        Can {
            import_neutral: true,
            ..Can::default()
        },
        None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_on_request_to_dsh_carry_uses_ask_and_keeps_the_model_words() {
    let rig = reduced_modes_rig().await;
    let _harness = Remembering::on(&rig);
    let thread = rig.world.thread("Carry to the reduced approval list").await;
    rig.world
        .set(thread, keys::APPROVAL_MODE, json!("on_request"))
        .await;
    assert_eq!(
        rig.asked(thread, "Remember blue comet", 1).await,
        "I know: Remember blue comet"
    );
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_engine::resolved::load(&mut conn, thread)
            .await
            .unwrap()
            .approval_mode,
        ApprovalMode::Ask
    );
    drop(conn);
    assert_eq!(
        rig.asked(thread, "What should you remember?", 2).await,
        "I know: Remember blue comet / I know: Remember blue comet / What should you remember?"
    );
    let opens = rig.world.seen.all();
    let target_open = opens
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SessionOpen(open) => Some(open),
            Command::SessionOpenFenced(open) => Some(open.session.as_ref()),
            Command::SessionPrefixOpen(open) => Some(open.session.as_ref()),
            Command::SessionContinuationResume(open) => Some(open.session.as_ref()),
            _ => None,
        })
        .expect("the carried session was opened");
    assert_eq!(target_open.mode, SessionMode::ImportNeutral);
    assert_eq!(
        target_open.approval_mode,
        orch_link_contract::commands::ApprovalMode::Ask
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reduced_mode_switch_does_not_widen_the_places_allowed_modes() {
    let rig = reduced_modes_rig().await;
    let thread = rig
        .thread_with_history("Approval policy stays in force")
        .await;
    rig.world
        .set(thread, keys::APPROVAL_MODE, json!("on_request"))
        .await;
    // The allowed modes are a policy of a topic and the places above it, never of one thread.
    rig.world
        .set(rig.world.topic, keys::APPROVAL_ALLOWED_MODES, json!(["on_request"]))
        .await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.code == "approval_mode_not_allowed")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reduced_mode_switch_preserves_a_supported_bypass_choice() {
    let rig = reduced_modes_rig().await;
    let thread = rig.thread_with_history("Keep an offered choice").await;
    rig.world
        .set(thread, keys::APPROVAL_MODE, json!("bypass"))
        .await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.to.approval_mode.as_deref(), Some("bypass"));
    assert!(
        !plan
            .blockers
            .iter()
            .any(|blocker| blocker.code.starts_with("approval_mode_"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reduced_mode_switch_cannot_replace_an_inherited_approval_lock() {
    let rig = reduced_modes_rig().await;
    let thread = rig
        .thread_with_history("Approval lock stays in force")
        .await;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    orch_settings::write::write(
        &mut conn,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(rig.world.ada),
            target: orch_settings::Target::Node(rig.world.topic),
            key: keys::APPROVAL_MODE,
            value: json!("on_request"),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        rig.world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.code == "approval_mode_locked")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_to_a_reduced_mode_program_cannot_override_its_topics_approval_lock() {
    let rig = reduced_modes_rig().await;
    let thread = rig
        .thread_with_history("Fork approval lock stays in force")
        .await;
    lock_topic_mode(&rig, "on_request").await;
    let mut request = rig.to_other();
    request.result = ForkResult::New;
    let plan = rig.plan(thread, &request).await;
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.code == "approval_mode_locked")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_can_inherit_an_offered_locked_approval_mode_without_writing_an_override() {
    let rig = reduced_modes_rig().await;
    let _harness = Remembering::on(&rig);
    let thread = rig.world.thread("Offered topic mode stays inherited").await;
    rig.asked(thread, "Remember blue comet", 1).await;
    lock_topic_mode(&rig, "ask").await;
    let mut request = rig.to_other();
    request.result = ForkResult::New;
    let plan = rig.plan(thread, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let fork = rig.new_thread_of(&job);
    let mut conn = rig.world.pool.acquire().await.unwrap();
    let mode = orch_settings::resolve(
        &mut conn,
        orch_settings::Place::Node(fork),
        keys::APPROVAL_MODE,
    )
    .await
    .unwrap();
    assert_eq!(mode.value(), Some(&json!("ask")));
    assert!(
        mode.own.is_none(),
        "an inherited lock needs no redundant write"
    );
    drop(conn);
    assert!(
        rig.asked(fork, "Which words?", 2)
            .await
            .contains("Remember blue comet")
    );
}

async fn lock_topic_mode(rig: &Rig, mode: &str) {
    let mut conn = rig.world.pool.acquire().await.unwrap();
    orch_settings::write::write(
        &mut conn,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(rig.world.ada),
            target: orch_settings::Target::Node(rig.world.topic),
            key: keys::APPROVAL_MODE,
            value: json!(mode),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        rig.world.clock.now(),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codex_on_request_to_dsh_without_history_uses_ask_on_the_next_turn() {
    let rig = reduced_modes_rig().await;
    let _harness = Remembering::on(&rig);
    let thread = rig.world.thread("No-history approval selection").await;
    rig.world
        .set(thread, keys::APPROVAL_MODE, json!("on_request"))
        .await;
    rig.asked(thread, "Remember blue comet", 1).await;
    let mut request = rig.to_other();
    request.history = Some(ForkHistory {
        mode: HistoryMode::Nothing,
        from_turn: None,
        to_turn: None,
    });
    let plan = rig.plan(thread, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.to.approval_mode.as_deref(), Some("ask"));
    assert_eq!(plan.history.method, HistoryMethod::Nothing);
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_engine::resolved::load(&mut conn, thread)
            .await
            .unwrap()
            .approval_mode,
        ApprovalMode::Ask
    );
    drop(conn);
    assert_eq!(
        rig.asked(thread, "Fresh side", 2).await,
        "I know: Fresh side"
    );
    let opens = rig.world.seen.all();
    let target_open = opens
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::SessionOpen(open) => Some(open),
            Command::SessionOpenFenced(open) => Some(open.session.as_ref()),
            Command::SessionPrefixOpen(open) => Some(open.session.as_ref()),
            Command::SessionContinuationResume(open) => Some(open.session.as_ref()),
            _ => None,
        })
        .expect("the no-history session was opened for the next turn");
    assert_eq!(target_open.mode, SessionMode::New);
    assert_eq!(
        target_open.approval_mode,
        orch_link_contract::commands::ApprovalMode::Ask
    );
}
