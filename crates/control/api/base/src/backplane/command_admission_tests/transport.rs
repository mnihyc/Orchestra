//! A retained native session gets its next forwarded transport through the actual API command port.

use super::*;

use crate::thread_engine::DispatchCommands;

use orch_engine::commands::Commands;

use orch_engine::commands::Outcome;

use orch_engine::commands::Outgoing;

use orch_link::commands::CommandIssuer;

use orch_link::commands::Progress;

use orch_link_contract::commands::CmdRes;

use orch_link_contract::commands::SessionMode;

use orch_link_contract::commands::SessionOpen;

use orch_link_contract::commands::SessionOpenProvider;

use orch_link_contract::commands::SessionOpenResult;

use orch_link_contract::commands::TicketSlot;

use orch_link_contract::continuations::SessionOpenFenced;

use orch_testkit::doubles::controller::ScriptedController;

use orch_testkit::doubles::link::Hub;

use orch_testkit::doubles::link::TestPki;

use orch_testkit::doubles::machine::MachineSpec;

use orch_testkit::doubles::machine::ScriptedMachine;
/// This gateway cut forwards the real machine link's replies, including non-journaled calls.
fn connect(fixture: &mut Fixture, issuer: CommandIssuer) -> oneshot::Receiver<()> {
    let link = fixture.link.clone();
    let machine = fixture.machine;
    let native = fixture.native.clone();
    let stop = CancellationToken::new();
    let ended = stop.clone();
    let owned_link = link.clone();
    let (ready, waiting) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (out, mut incoming) = mpsc::unbounded_channel();
        let gateway = Arc::new(GatewayConn {
            generation: 1,
            out,
            closed: CancellationToken::new(),
            stats: Mutex::new(None),
            started_at: link.inner.clock.time(),
            #[cfg(feature = "test-observers")]
            observed: orch_link::test_observers::QueueGauge::new("api.backplane.transport"),
        });
        let session = new_id();
        link.attach(&gateway, machine, session, None).await;
        ready.send(()).map_err(|_| "the gateway owner ended")?;
        let mut replies = JoinSet::<Proof>::new();
        loop {
            let message = tokio::select! {
                () = ended.cancelled() => break,
                message = incoming.recv() => message,
            };
            let Some(Outbound::Frame(frame)) = message else {
                return Err("the transport gateway ended unexpectedly".into());
            };
            let BackplaneFrame::Down(Down { machine_id, frame }) = *frame else {
                return Err("the transport gateway received another frame kind".into());
            };
            if machine_id != machine {
                return Err("the transport gateway received another machine's work".into());
            }
            match frame {
                DownFrame::Cmd(command) => {
                    let id = command.id;
                    native
                        .wire
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(command.clone());
                    let mut handle = issuer.issue(command);
                    let link = link.clone();
                    let gateway = gateway.clone();
                    replies.spawn(async move {
                        let outcome = match within(handle.wait()).await? {
                            Progress::Succeeded(value) => Ok(value),
                            Progress::Failed(error) => Err(error),
                            other => {
                                return Err(
                                    format!("the native peer did not finish: {other:?}").into()
                                );
                            }
                        };
                        link.route_up(&gateway, machine, session, UpFrame::CmdAck(CmdAck { id }))
                            .await;
                        link.route_up(
                            &gateway,
                            machine,
                            session,
                            UpFrame::CmdRes(CmdRes { id, outcome }),
                        )
                        .await;
                        Ok(())
                    });
                }
                DownFrame::CmdCancel(cancel) => issuer.cancel(cancel.id),
                _ => return Err("the transport gateway received an undeclared request".into()),
            }
        }
        while let Some(reply) = replies.join_next().await {
            reply??;
        }
        Ok(())
    });
    fixture.peers.push(Peer {
        stop,
        task: Some(task),
        link: owned_link,
    });
    waiting
}

#[derive(Default)]
struct Session {
    source: Option<(Id, Id)>,
    ticket: Option<TicketSlot>,
    opens: usize,
    rebinds: usize,
    turns: Vec<TicketSlot>,
}

#[derive(Default)]
struct Harness {
    session: Mutex<Session>,
}

#[async_trait::async_trait]
impl CommandHandler for Harness {
    async fn handle(
        &self,
        _context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        match command {
            Command::SessionOpen(open) => {
                let SessionOpenProvider::Forwarded { ticket, .. } = open.provider else {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "not the forwarded fixture",
                    ));
                };
                if open.transport_only {
                    if session.source != Some((open.thread_id, open.segment_id))
                        || open.native_session_ref.as_deref() != Some("retained-conversation")
                    {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "another native session",
                        ));
                    }
                    session.rebinds += 1;
                } else {
                    session.source = Some((open.thread_id, open.segment_id));
                    session.opens += 1;
                }
                session.ticket = Some(ticket);
                Ok(CommandResult::SessionOpen(SessionOpenResult {
                    native_session_ref: "retained-conversation".into(),
                    resumed: open.transport_only,
                    import_report: None,
                    instruction_sources: Vec::new(),
                    approval_reviewer: None,
                }))
            }
            Command::TurnStart(turn) => {
                if session.source.map(|source| source.0) != Some(turn.thread_id) {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "no open native session",
                    ));
                }
                let ticket = session.ticket.clone().ok_or_else(|| {
                    CommandError::new(ErrorCode::Conflict, "the live transport is missing")
                })?;
                session.turns.push(ticket);
                Ok(CommandResult::TurnStart(Empty {}))
            }
            _ => Err(CommandError::new(
                ErrorCode::Unsupported,
                "not a session fixture command",
            )),
        }
    }
}

fn opening(fixture: &Fixture, thread: Id) -> Proof<SessionOpen> {
    Ok(serde_json::from_value(serde_json::json!({
        "agent_id":new_id(), "thread_id":thread, "segment_id":new_id(), "mode":"new",
        "native_session_ref":null, "archive":null, "history":null, "upto_turn_ordinal":null,
        "workspace_root":"/workspace", "instruction_root_marker":true,
        "directory":"/workspace", "cwd":"/workspace", "mounts":[],
        "model":"fixture-model", "effort":null, "params":{},
        "approval_mode":"ask", "sandbox_mode":"read_only", "instructions":"", "memory":null,
        "arrangement":"together",
        "provider":{"path":"forwarded", "provider_id":new_id(), "protocol":"openai-responses", "ticket":"synthetic-first-transport"},
        "exec":{"executor":null, "environment_id":fixture.environment, "generation":1},
        "mcp":[], "orch_tools":[], "raw_events":false
    }))?)
}

fn turn(machine: Id, thread: Id, text: &str) -> Proof<Outgoing> {
    Ok(Outgoing::new(
        new_id(),
        machine,
        SerialKey::Thread(thread),
        &TurnStart {
            native_model: None,
            native_wake: None,
            native_action: None,
            thread_id: thread,
            turn_id: new_id(),
            text: text.into(),
            images: Vec::new(),
            model: "fixture-model".into(),
            effort: None,
            params: Default::default(),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::ReadOnly,
            plan: false,
        },
    )?)
}

#[cfg(any(test, feature = "test-support"))]
async fn two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body()
-> Proof {
    let mut fixture = Fixture::new(false).await?;
    let harness = Arc::new(Harness::default());
    let clock: Arc<dyn Clock> = Arc::new(fixture.clock.clone());
    let pki = TestPki::new(clock.clone());
    let hub = Hub::new(clock.clone(), pki.certificate());
    let identity = pki.machine(fixture.machine);
    let spec = MachineSpec::default();
    let controller =
        ScriptedController::attach(&hub, clock.clone(), fixture.machine, spec.roles.clone());
    let machine = ScriptedMachine::start(&hub, &clock, &identity, &spec);
    machine.link.serve(harness.clone());
    within(machine.link.connected()).await?;
    within(connect(&mut fixture, controller.link().commands().clone())).await??;
    let commands = DispatchCommands::new(fixture.dispatcher.clone());
    let thread = new_id();
    let mut open = opening(&fixture, thread)?;
    let initial = Outgoing::new(new_id(), fixture.machine, SerialKey::Thread(thread), &open)?;
    // A real opening cannot acquire the transient route merely by calling observe.
    assert!(commands.observe(initial.clone()).await.is_err());
    commands.issue(initial.clone()).await?;
    assert!(matches!(
        within(commands.wait(initial.id)).await??,
        Outcome::Succeeded(_)
    ));
    let first = turn(fixture.machine, thread, "First turn")?;
    commands.issue(first.clone()).await?;
    assert!(matches!(
        within(commands.wait(first.id)).await??,
        Outcome::Succeeded(_)
    ));

    open.transport_only = true;
    open.mode = SessionMode::Resume;
    open.native_session_ref = Some("retained-conversation".into());
    let SessionOpenProvider::Forwarded { ticket, .. } = &mut open.provider else {
        return Err("the fixture lost its forwarded provider".into());
    };
    *ticket = TicketSlot::Signed("synthetic-second-transport-not-for-storage".into());
    let rebind = Outgoing::new(new_id(), fixture.machine, SerialKey::Thread(thread), &open)?;
    assert!(
        commands.issue(rebind.clone()).await.is_err(),
        "a rebind never joins durable storage"
    );
    let fenced = Outgoing::new(
        new_id(),
        fixture.machine,
        SerialKey::Thread(thread),
        &SessionOpenFenced {
            thread_id: thread,
            session: Box::new(open.clone()),
            predecessor: None,
        },
    )?;
    assert!(
        commands.issue(fenced).await.is_err(),
        "a fence cannot make the transport body durable"
    );
    let result = within(commands.observe(rebind.clone())).await??;
    let Outcome::Succeeded(result) = result else {
        return Err("the live rebind failed".into());
    };
    let result: SessionOpenResult = serde_json::from_value(result)?;
    assert_eq!(result.native_session_ref, "retained-conversation");
    assert!(result.resumed);
    let sent = fixture
        .native
        .wire
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let live = sent
        .iter()
        .find(|command| command.kind == "session.open" && command.body["transport_only"] == true)
        .ok_or("the rebind never reached the actual wire")?;
    assert!(
        rows::command(&mut *fixture.test.pool.acquire().await?, live.id)
            .await?
            .is_none()
    );
    assert!(
        rows::command(&mut *fixture.test.pool.acquire().await?, rebind.id)
            .await?
            .is_none()
    );
    let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commands WHERE body::text LIKE '%synthetic-second-transport-not-for-storage%')")
        .fetch_one(&fixture.test.pool).await?;
    assert!(
        !retained,
        "the new forwarder ticket is never an API command body"
    );
    let journal = Journal::open(
        &machine.data_dir().join("journal"),
        Duration::from_secs(24 * 3600),
        fixture.clock.time(),
    )?;
    assert!(
        journal.get(initial.id).is_some(),
        "the actual native journal still records ordinary opens"
    );
    assert!(journal.get(first.id).is_some());
    assert!(
        journal.get(live.id).is_none(),
        "the native journal never records the transport rebind"
    );

    let second = turn(fixture.machine, thread, "Second turn")?;
    commands.issue(second.clone()).await?;
    assert!(matches!(
        within(commands.wait(second.id)).await??,
        Outcome::Succeeded(_)
    ));
    {
        let session = harness
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            (session.opens, session.rebinds, session.turns.len()),
            (1, 1, 2)
        );
        assert_eq!(session.source, Some((thread, open.segment_id)));
        assert_eq!(
            session.turns[0],
            TicketSlot::Signed("synthetic-first-transport".into())
        );
        assert_eq!(
            session.turns[1],
            TicketSlot::Signed("synthetic-second-transport-not-for-storage".into())
        );
    }
    let completed_journal = Journal::open(
        &machine.data_dir().join("journal"),
        Duration::from_secs(24 * 3600),
        fixture.clock.time(),
    )?;
    assert!(completed_journal.get(second.id).is_some());
    assert!(completed_journal.get(live.id).is_none());

    let entry = fixture.link.entry(fixture.machine).await?;
    assert!(!entry.owns_observation(live.id));
    // A kind/flag without ownership is not allowed to bypass durable admission.
    let id = new_id();
    let mut unowned = entry.controller.commands().issue_typed(
        id,
        &SerialKey::Thread(thread),
        Time::new(fixture.clock.now() + chrono::Duration::seconds(15)),
        &open,
    )?;
    assert!(
        matches!(within(unowned.wait()).await?, Progress::Failed(ref error) if error.code==ErrorCode::NotFound.as_str())
    );
    assert!(journal.get(id).is_none());
    assert_eq!(
        harness
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .rebinds,
        1
    );
    let result = fixture.close().await;
    machine.link.shutdown().await;
    result
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body()
-> ::futures::future::LocalBoxFuture<'static, impl ::std::process::Termination> {
    ::std::boxed::Box::pin(self::two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body())
}
