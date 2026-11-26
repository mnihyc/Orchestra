//! A pipelined input keeps its real session-opening receipt as a local prerequisite.
use super::*;
use orch_link::commands::CommandHandle;
use orch_link_contract::commands::{
    ApprovalMode, ArrangementType, Empty, SandboxMode, SessionMode, SessionOpen, SessionOpenExec,
    SessionOpenProvider, SessionOpenResult, TurnStart,
};
use orch_types::Id;

fn opening(rig: &Rig, thread: Id) -> Cmd {
    let segment = new_id();
    let mut command = Cmd::new(
        new_id(),
        &SerialKey::Thread(thread),
        Cmd::work_budget(rig.clock.time(), deadline(&rig.clock, 300)),
        &SessionOpen {
            program_work: None,
            output_capture_bytes: 1024,
            native_model: None,
            transport_only: false,
            turn_changes_require_restart: false,
            agent_id: new_id(),
            thread_id: thread,
            segment_id: segment,
            mode: SessionMode::New,
            native_session_ref: None,
            archive: None,
            history: None,
            upto_turn_ordinal: None,
            workspace_root: "/workspace".to_owned(),
            instruction_root_marker: false,
            directory: "work".to_owned(),
            cwd: String::new(),
            mounts: Vec::new(),
            model: "scripted".to_owned(),
            effort: None,
            params: Default::default(),
            approval_mode: ApprovalMode::Ask,
            available_models: vec!["scripted".to_owned()],
            subagent_limits: Default::default(),
            prompt_cache_ttl: None,
            background_wake: false,
            web_action_wait_secs: 120,
            image_generation: None,
            sandbox_mode: SandboxMode::Workspace,
            instructions: String::new(),
            memory: None,
            arrangement: ArrangementType::Together,
            provider: SessionOpenProvider::Native {
                provider_id: new_id(),
            },
            exec: SessionOpenExec {
                executor: None,
                tool_set: None,
                environment_id: new_id(),
                generation: 1,
                ticket: None,
            },
            mcp: Vec::new(),
            skills: Vec::new(),
            orch_tools: Vec::new(),
            raw_events: false,
            raw_after: 0,
            side_reading: None,
            workspace_configuration: None,
            orch_tool_descriptions: Default::default(),
            orch_tool_schemas: Default::default(),
        },
    )
    .unwrap();
    command.native_target = Some(orch_link_contract::commands::NativeTarget::Session(
        orch_link_contract::commands::SessionTarget {
            thread_id: thread,
            segment_id: Some(segment),
            opening_id: Some(command.id),
            opening_epoch: Some(1),
        },
    ));
    command
}

fn input(rig: &Rig, thread: Id, opening: &Cmd) -> Cmd {
    let command = Cmd::new(
        new_id(),
        &SerialKey::Thread(thread),
        Cmd::work_budget(rig.clock.time(), deadline(&rig.clock, 300)),
        &TurnStart {
            native_model: None,
            native_wake: None,
            thread_id: thread,
            turn_id: new_id(),
            text: "Only after this session opened".to_owned(),
            images: Vec::new(),
            model: "scripted".to_owned(),
            effort: None,
            params: Default::default(),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::Workspace,
            native_action: None,
            plan: false,
        },
    )
    .unwrap();
    // Exercise the envelope's actual wire member.
    let mut wire = serde_json::to_value(command).unwrap();
    wire["after"] = json!(opening.id);
    wire["native_target"] = json!(opening.native_target);
    serde_json::from_value(wire).unwrap()
}

fn answer(rig: &Rig) {
    rig.machine.handler.on(CommandKind::SessionOpen, |_| {
        Ok(CommandResult::SessionOpen(SessionOpenResult {
            mcp: None,
            native_session_ref: "scripted-session".to_owned(),
            resumed: false,
            import_report: None,
            instruction_sources: Vec::new(),
            approval_reviewer: None,
        }))
    });
    rig.machine.handler.on(CommandKind::TurnStart, |_| {
        Ok(CommandResult::TurnStart(Empty {}))
    });
}

async fn accepted(handle: &CommandHandle) {
    within(10, async {
        while matches!(handle.progress(), Progress::Queued | Progress::Sent) {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test]
async fn an_input_received_first_waits_for_its_opening_without_another_system_exchange() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    rig.machine.handler.hold(CommandKind::SessionOpen);
    let thread = new_id();
    let opening = opening(&rig, thread);
    let input = input(&rig, thread, &opening);
    let link = rig.controller.link();
    let issuer = link.commands();
    let mut started = issuer.issue(input.clone());
    accepted(&started).await;
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
    let mut opened = issuer.issue(opening.clone());
    within(10, rig.machine.handler.started(opening.id)).await;
    assert_eq!(
        rig.machine.handler.runs_of(input.id),
        0,
        "durable input acceptance does not run it before the opening succeeded"
    );
    rig.machine.handler.release();
    assert!(matches!(
        within(10, opened.wait()).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        within(10, started.wait()).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.machine.handler.runs_of(opening.id), 1);
    assert_eq!(rig.machine.handler.runs_of(input.id), 1);
}

#[tokio::test]
async fn a_refused_reopening_never_delivers_the_queued_input_to_the_previous_session() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    let thread = new_id();
    let link = rig.controller.link();
    let issuer = link.commands();
    let mut previous = issuer.issue(opening(&rig, thread));
    assert!(matches!(
        within(10, previous.wait()).await,
        Progress::Succeeded(_)
    ));
    rig.machine.handler.on(CommandKind::SessionOpen, |_| {
        Err(CommandError::new(
            ErrorCode::Unsupported,
            "the requested opening was refused",
        ))
    });
    let opening = opening(&rig, thread);
    let input = input(&rig, thread, &opening);
    let mut opened = issuer.issue(opening);
    let mut started = issuer.issue(input.clone());
    let refused = within(10, opened.wait()).await;
    assert!(matches!(refused, Progress::Failed(_)));
    assert_eq!(within(10, started.wait()).await, refused);
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
}

#[tokio::test]
async fn stopping_an_input_whose_opening_has_not_arrived_releases_its_thread_queue() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    let thread = new_id();
    let link = rig.controller.link();
    let issuer = link.commands();
    let input = input(&rig, thread, &opening(&rig, thread));
    let mut started = issuer.issue(input.clone());
    accepted(&started).await;
    issuer.cancel(input.id);
    assert_eq!(within(10, started.wait()).await, Progress::Cancelled);
    let mut next = issuer.issue(
        Cmd::new(
            new_id(),
            &SerialKey::Thread(thread),
            Cmd::work_budget(rig.clock.time(), deadline(&rig.clock, 300)),
            &SessionClose { thread_id: thread },
        )
        .unwrap(),
    );
    assert!(matches!(
        within(10, next.wait()).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
}

#[tokio::test]
async fn an_opening_of_another_thread_cannot_release_an_input() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    let link = rig.controller.link();
    let issuer = link.commands();
    let opening = opening(&rig, new_id());
    let mut opened = issuer.issue(opening.clone());
    assert!(matches!(
        within(10, opened.wait()).await,
        Progress::Succeeded(_)
    ));
    let input = input(&rig, new_id(), &opening);
    let mut started = issuer.issue(input.clone());
    assert!(matches!(
        within(10, started.wait()).await,
        Progress::Failed(_)
    ));
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
}

#[tokio::test]
async fn an_input_for_a_different_opening_epoch_never_runs_in_the_same_thread() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    let thread = new_id();
    let opening = opening(&rig, thread);
    let mut input = input(&rig, thread, &opening);
    let Some(orch_link_contract::commands::NativeTarget::Session(target)) =
        &mut input.native_target
    else {
        panic!("the fixture supplies the accepted native opening");
    };
    target.opening_epoch = Some(2);
    let link = rig.controller.link();
    let issuer = link.commands();
    let mut opened = issuer.issue(opening);
    let mut started = issuer.issue(input.clone());
    assert!(matches!(
        within(10, opened.wait()).await,
        Progress::Succeeded(_)
    ));
    assert!(matches!(
        within(10, started.wait()).await,
        Progress::Failed(_)
    ));
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
}

#[tokio::test]
async fn a_cycle_in_prerequisites_fails_without_running_either_native_command() {
    let rig = rig();
    rig.machine.link.connected().await;
    answer(&rig);
    let link = rig.controller.link();
    let issuer = link.commands();
    let thread = new_id();
    let opening = opening(&rig, thread);
    let input = input(&rig, thread, &opening);
    let mut wire = serde_json::to_value(opening).unwrap();
    wire["after"] = json!(input.id);
    let opening: Cmd = serde_json::from_value(wire).unwrap();
    let opening_id = opening.id;
    let mut started = issuer.issue(input.clone());
    let mut opened = issuer.issue(opening);
    assert!(matches!(
        within(10, opened.wait()).await,
        Progress::Failed(_)
    ));
    assert!(matches!(
        within(10, started.wait()).await,
        Progress::Failed(_)
    ));
    assert_eq!(rig.machine.handler.runs_of(opening_id), 0);
    assert_eq!(rig.machine.handler.runs_of(input.id), 0);
}
