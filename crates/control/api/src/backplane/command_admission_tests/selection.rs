//! The late-adoption cut composed with the actual claimed continuation job.
//! Agent and environment machines are distinct recorded identities, each with one native server.

#[path = "../../../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../../../engine/tests/native_continuations/mod.rs"]
mod native_prefix;

use super::*;
use crate::thread_engine::DispatchCommands;
use orch_agent_contract::{descriptor::AgentDescriptor, history::NativePrefixSource};
use orch_api_contract::threads::{ContentPart, ThreadPromptVersionCreateBody, TokenUsage};
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::BlobStore;
use orch_blob_local::LocalBlobStore;
use orch_db::queries::{storage, threads};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing};
use orch_link_contract::{
    commands::{OrchTool, OrchToolResult},
    events::{Event, OrchToolCall, TurnOutcome, UsageUpdated},
    streams::Stream,
};
use orch_types::Json;

struct SourceGate {
    reached: oneshot::Sender<Id>,
    release: oneshot::Receiver<()>,
}
static ADMISSIONS: OnceLock<Mutex<HashMap<Id, SourceGate>>> = OnceLock::new();

pub(super) async fn adoption_read(link: &Link, id: Id) {
    if ADMISSIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
    {
        return;
    }
    let answer = {
        let mut conn = link
            .inner
            .db
            .acquire()
            .await
            .expect("the source fixture can read its actual admission");
        sqlx::query_scalar::<_, Id>(
            "SELECT e.answer_id FROM thread_tool_commands e JOIN commands c ON c.id=e.command_id
            WHERE e.command_id=$1 AND c.kind='fs.write'",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .expect("the source fixture can identify its actual command")
    };
    let Some(answer) = answer else { return };
    let gate = ADMISSIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&answer);
    if let Some(gate) = gate {
        let _ = gate.reached.send(id);
        let _ = gate.release.await;
    }
}

/// Route only the separately recorded environment machine through production Dispatcher.
/// The agent's existing real link, journal, ingress applier and Process witness are retained.
struct Split {
    original: Arc<dyn Commands>,
    environment_machine: Id,
    dispatcher: Dispatcher,
    native: DispatchCommands,
}

impl Split {
    async fn is_environment(&self, id: Id) -> Result<bool, CommandsError> {
        self.dispatcher
            .status(id)
            .await
            .map(|row| row.is_some_and(|row| row.machine_id == self.environment_machine))
            .map_err(|_| {
                CommandsError::Other("the command's recorded machine could not be read".into())
            })
    }
}

#[async_trait::async_trait]
impl Commands for Split {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if command.machine == self.environment_machine {
            self.native.issue(command).await
        } else {
            self.original.issue(command).await
        }
    }
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        if command.machine == self.environment_machine {
            self.native.observe(command).await
        } else {
            self.original.observe(command).await
        }
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        if self.is_environment(id).await? {
            self.native.retained_body(id).await
        } else {
            self.original.retained_body(id).await
        }
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        if self.is_environment(id).await? {
            self.native.retained_command(id).await
        } else {
            self.original.retained_command(id).await
        }
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        if self.is_environment(id).await? {
            self.native.status(id).await
        } else {
            self.original.status(id).await
        }
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        if self.is_environment(id).await? {
            self.native.wait(id).await
        } else {
            self.original.wait(id).await
        }
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        if self.is_environment(id).await? {
            self.native.cancel(id).await
        } else {
            self.original.cancel(id).await
        }
    }
    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        if machine == self.environment_machine {
            self.native.hears_behind(machine, stream)
        } else {
            self.original.hears_behind(machine, stream)
        }
    }
}

async fn answer(world: &common::World, thread: Id, call: &str) -> Proof<OrchToolResult> {
    answer_attempt(world, thread, call, None).await
}

async fn answer_attempt(
    world: &common::World,
    thread: Id,
    call: &str,
    attempt: Option<Id>,
) -> Proof<OrchToolResult> {
    within(async {
        loop {
            let bodies = world
                .commands
                .issued_bodies(CommandKind::OrchToolResult.as_str());
            if let Some((_, body)) = bodies.into_iter().find(|(_, body)| {
                body["call_ref"].as_str() == Some(call)
                    && body["thread_id"] == json!(thread)
                    && body["attempt"] == json!(attempt)
            }) {
                return serde_json::from_value(body).map_err(Into::into);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

async fn selected_snapshot(world: &common::World, thread: Id, document: Id) -> Proof<Json> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('thread',to_jsonb(t),
        'inputs',(SELECT jsonb_agg(to_jsonb(i) ORDER BY i.id) FROM thread_inputs i WHERE i.thread_id=t.node_id),
        'document',(SELECT to_jsonb(d) FROM documents d WHERE d.id=$2),
        'blocks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM document_blocks b WHERE b.document_id=$2),
        'ledger',(SELECT jsonb_agg(to_jsonb(u) ORDER BY u.id) FROM usage_events u WHERE u.thread_id=t.node_id))
        FROM threads t WHERE t.node_id=$1")
        .bind(thread).bind(document).fetch_one(world.pool.pool()).await?)
}

#[tokio::test]
async fn a_real_guarded_selection_and_gc_can_finish_before_the_old_cancelled_dispatcher_adoption_without_a_late_effect()
-> Proof {
    let world = common::World::new().await;
    let directory = Arc::new(tempfile::tempdir()?);
    tokio::fs::create_dir(directory.path().join("web")).await?;
    let store = Arc::new(LocalBlobStore::open(directory.path().join("blobs")).await?);
    let mut config = ServerConfig::default();
    config.retention.blob_grace_hours = 0;
    let link = Link::detached(world.pool.clone(), world.clock.clone(), Arc::new(config))?;
    let dispatcher = link.dispatcher();
    let machine = resources::testing::insert_machine(
        &mut *world.pool.acquire().await?,
        "separate-environment-peer",
        "connected",
    )
    .await?;
    assert_ne!(machine, world.machine_id);
    sqlx::query("UPDATE machines SET roles=ARRAY['environment'],online=true,facts=$2 WHERE id=$1")
        .bind(machine).bind(json!({"policy":{"exec.offers":["orch-exec"],"environment.remote_write":true,
            "environment.roots":[directory.path().to_str().ok_or("the owned root is not UTF-8")?]}})).execute(world.pool.pool()).await?;
    // A separated arrangement consumes the complete typed environment report. The
    // together-only World's abbreviated facts are deliberately not reused here.
    let facts = orch_agent_contract::facts::EnvironmentFacts {
        looked_at: None,
        browser: Default::default(),
        vnc: Default::default(),
        os: orch_agent_contract::facts::Os::Linux,
        arch: orch_agent_contract::facts::Arch::X86_64,
        shell: orch_agent_contract::facts::ShellInfo {
            name: "sh".into(),
            path: "/bin/sh".into(),
        },
        user: "owned-proof".into(),
        home: directory
            .path()
            .to_str()
            .ok_or("the owned root is not UTF-8")?
            .into(),
        workspace_root: directory
            .path()
            .to_str()
            .ok_or("the owned root is not UTF-8")?
            .into(),
        path_style: orch_agent_contract::facts::PathStyle::Posix,
        case_sensitive: true,
        executors: vec![orch_agent_contract::facts::ExecutorFact {
            kind: "orch-exec".into(),
            version: "1.0.0".into(),
            available: true,
            reason: None,
            sandbox_modes: vec![
                orch_agent_contract::descriptor::SandboxMode::ReadOnly,
                orch_agent_contract::descriptor::SandboxMode::Workspace,
                orch_agent_contract::descriptor::SandboxMode::Open,
            ],
            tool_sets: Some(vec!["plain".into()]),
        }],
        mcp_found: Vec::new(),
        index: orch_agent_contract::facts::IndexMode::Live,
        mounts: Vec::new(),
        private_views: false,
        suspend: false,
        git: false,
        generation: 1,
    };
    sqlx::query("UPDATE environments SET machine_id=$2,workspace_root=$3,facts=$4 WHERE id=$1")
        .bind(world.environment)
        .bind(machine)
        .bind(&facts.workspace_root)
        .bind(serde_json::to_value(&facts)?)
        .execute(world.pool.pool())
        .await?;
    let endpoint = Arc::new(Native {
        _directory: directory.clone(),
        environment: world.environment,
        payload: directory.path().join("unused-local-input"),
        destination: directory.path().join("web/copied-doc.md"),
        clock: world.clock.clone(),
        expected_path: "web/copied-doc.md".into(),
        blobs: Some(store.clone()),
        hold: false,
        entered: AtomicBool::new(false),
        release: Semaphore::new(0),
        reads: AtomicUsize::new(0),
        writes: AtomicUsize::new(0),
        probes: AtomicUsize::new(0),
        starts: AtomicUsize::new(0),
        wire: Mutex::new(Vec::new()),
    });
    let server = CommandServer::new(
        Journal::open(
            &directory.path().join("journal"),
            Duration::from_secs(24 * 3600),
            world.clock.time(),
        )?,
        world.clock.clone(),
        ServerOptions {
            roles: vec![Role::Environment],
            connected: true,
        },
    );
    server.serve(endpoint.clone());
    let (peer, ready) = peer(
        link.clone(),
        machine,
        server.clone(),
        endpoint.clone(),
        false,
    );
    within(ready).await??;
    let dispatch = dispatcher.clone();
    world
        .restart_api_using(move |original| {
            Arc::new(Split {
                original,
                environment_machine: machine,
                dispatcher: dispatch.clone(),
                native: DispatchCommands::new(dispatch),
            })
        })
        .await;
    let mut described = common::descriptor();
    described["capabilities"]["native_input_prefix"] = json!("turn_start");
    described["capabilities"]["orch_tools"] = json!("mcp");
    described["capabilities"]["mcp"] =
        json!({"stdio":true,"http":true,"reload":false,"strict":true});
    let descriptor: AgentDescriptor = serde_json::from_value(described.clone())?;
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(world.agent)
        .bind(described)
        .execute(world.pool.pool())
        .await?;
    let declared = descriptor.clone();
    world.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(
            orch_link_contract::commands::AgentStartResult {
                reused: false,
                harness_version: "1.0.0".into(),
                descriptor: declared.clone(),
            },
        ))
    });
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic: world.topic,
                name: "Real dispatcher and guarded selection".into(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".into(),
                arrangement: Some(orch_agent_contract::descriptor::ArrangementType::Separated),
                model: Some(common::MODEL.into()),
                effort: None,
            },
        )
        .await?
        .thread;
    let (start, submitted) = world
        .running_turn(thread, "Original actual authored input")
        .await;
    let segment = world.segment_of(thread).await;
    let native_ref: String = world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            segment,
        )
        .await;
    let source = NativePrefixSource {
        thread_id: thread,
        segment_id: segment,
        native_session_ref: native_ref,
    };
    native_prefix::install(
        &world.machine.handler,
        world.agent,
        world.provider,
        source.clone(),
        start.turn_id,
        descriptor,
    );
    world.engine().attach_blobs(store.clone());
    world
        .engine()
        .attach_parcels(Arc::new(orch_engine::parcels::WorkspaceParcels::new(
            &world.engine(),
        )));
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    world.machine.publish(&stream,&Event::OrchToolCall(OrchToolCall {turn_id:start.turn_id,call_ref:"create-dispatch-source".into(),attempt:None,
        tool:OrchTool::EditDocument,arguments:json!({"command":"create","path":"docs/dispatcher.md","markdown":"Original source material."})})).await;
    let created = answer(&world, thread, "create-dispatch-source").await?;
    assert!(created.error.is_none());
    let document: Id = serde_json::from_value(
        created
            .result
            .as_ref()
            .ok_or("the actual document result is absent")?["id"]
            .clone(),
    )?;
    let call = "stale-production-dispatch";
    let canonical = orch_engine::ids::tool_result(thread, call);
    let (reached, waiting) = oneshot::channel();
    let (release, resume) = oneshot::channel();
    assert!(
        ADMISSIONS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                canonical,
                SourceGate {
                    reached,
                    release: resume
                }
            )
            .is_none()
    );
    let arguments = json!({"path":format!("id:{document}"),"save_to":"copied-doc.md"});
    world
        .machine
        .publish(
            &stream,
            &Event::OrchToolCall(OrchToolCall {
                turn_id: start.turn_id,
                call_ref: call.into(),
                attempt: None,
                tool: OrchTool::DownloadShared,
                arguments: arguments.clone(),
            }),
        )
        .await;
    let command = within(waiting).await??;
    let saved = rows::command(&mut *world.pool.acquire().await?, command)
        .await?
        .ok_or("the admitted original is absent")?;
    assert_eq!(saved.machine_id, machine);
    assert_eq!(saved.state, "queued");
    assert!(server.entry(command).is_none());
    let blob: Hash = serde_json::from_value(saved.body["blob"].clone())?;
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(blob.as_bytes().as_slice())
            .fetch_one(world.pool.pool())
            .await?,
        1
    );
    assert!(dispatcher.cancel(command).await?);
    let usage = TokenUsage {
        input_tokens: 111,
        total_tokens: 111,
        ..Default::default()
    };
    world
        .say_in(
            thread,
            segment,
            Event::UsageUpdated(UsageUpdated {
                turn_id: start.turn_id,
                turn: usage,
                thread: usage,
                context_window: Some(100_000),
                context_used: Some(111),
                report: None,
            }),
        )
        .await;
    world
        .say_in(
            thread,
            segment,
            common::turn_ended(start.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    common::eventually("the real original terminal work settles", || async {
        !threads::turn_end_work_pending(
            &mut world
                .pool
                .acquire()
                .await
                .expect("the fixture database is live"),
            thread,
        )
        .await
        .unwrap_or(true)
    })
    .await;
    let accepted = {
        let mut tx = world.pool.begin().await?;
        let current = threads::thread(&mut tx, thread)
            .await?
            .ok_or("the actual thread is absent")?;
        let principal =
            Principal::load(&mut tx, Caller::session(world.ada), world.clock.time()).await?;
        world
            .engine()
            .create_prompt_version_guarded(
                tx,
                principal,
                thread,
                submitted.input.id,
                ThreadPromptVersionCreateBody {
                    client_id: "guarded-selection-after-controller-cancel".into(),
                    source_continuation_id: current.active_continuation_id,
                    content: vec![ContentPart::Text {
                        text: "The selected revised input.".into(),
                    }],
                    expected_active_continuation_id: current.active_continuation_id,
                    expected_execution_revision: current.execution_revision,
                    expected_queue_revision: current.queue_revision,
                    active_turn: None,
                },
            )
            .await?
    };
    let next = world.turn_start(thread, 1).await;
    world
        .answer(
            thread,
            &next,
            "selected-dispatch-answer",
            "The selected native lane runs.",
        )
        .await;
    world.wait_turn(next.turn_id, "running").await;
    common::eventually("the actual guarded job commits", || async {
        world
            .scalar::<bool>(
                "SELECT state='succeeded' FROM jobs WHERE id=$1",
                accepted.operation_id,
            )
            .await
    })
    .await;
    // World::answer publishes TurnStarted, ItemStarted, ItemDelta and ItemCompleted.
    // The running-turn/job checks can observe only the first of those commits.
    // Capture the immutable baseline after this actual selected answer is complete,
    // before releasing the old issuer; never discard its activity/sequence fields.
    common::eventually("the selected native answer is fully admitted before the stale-issuer baseline",||async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items i JOIN turns t ON t.id=i.turn_id
            JOIN threads h ON h.node_id=t.thread_id AND h.current_turn_id=t.id
            WHERE h.node_id=$1 AND t.id=$2 AND i.segment_id=t.segment_id
              AND i.continuation_id=h.active_continuation_id AND t.continuation_id=h.active_continuation_id
              AND i.native_ref='selected-dispatch-answer' AND i.kind='agent_message'
              AND i.status='completed' AND i.payload->>'text'='The selected native lane runs.')")
            .bind(thread).bind(next.turn_id).fetch_one(world.pool.pool()).await
            .expect("the selected native answer's actual row can be read")
    }).await;
    let (active,owner,input,new_segment,selected_segment):(Id,Id,Id,Id,Id)=sqlx::query_as("SELECT h.active_continuation_id,t.continuation_id,t.input_id,t.segment_id,h.current_segment_id
        FROM threads h JOIN turns t ON t.id=h.current_turn_id WHERE h.node_id=$1").bind(thread).fetch_one(world.pool.pool()).await?;
    assert_eq!(
        (active, owner),
        (accepted.continuation_id, accepted.continuation_id)
    );
    assert_eq!(Some(input), accepted.input_id);
    assert_ne!(new_segment, segment);
    assert_eq!(selected_segment, new_segment);
    let child_native: String = world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            new_segment,
        )
        .await;
    assert!(!child_native.is_empty());
    assert_ne!(child_native, source.native_session_ref);
    assert_eq!(world.began(CommandKind::SessionPrefixOpen), 1);
    assert!(
        world
            .scalar::<bool>(
                "SELECT settled FROM thread_tool_commands WHERE command_id=$1",
                command
            )
            .await
    );
    assert!(
        endpoint.probes.load(Ordering::SeqCst) > 0,
        "actual native readback must release the old effect hold before selection"
    );
    assert!(server.entry(command).is_none());
    assert_eq!(endpoint.reads.load(Ordering::SeqCst), 0);
    let fence: Id = world
        .scalar(
            "SELECT fence_id FROM thread_tool_commands WHERE command_id=$1",
            command,
        )
        .await;
    assert!(
        matches!(server.entry(fence).ok_or("the actual completion probe is absent")?.step,Step::Done {ref result} if result.outcome.is_ok())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(blob.as_bytes().as_slice())
            .fetch_one(world.pool.pool())
            .await?,
        0
    );
    assert!(
        storage::forget_blob(&mut *world.pool.acquire().await?, &blob, world.clock.now()).await?
    );
    assert!(store.delete(&blob).await?);
    let before = selected_snapshot(&world, thread, document).await?;
    release
        .send(())
        .map_err(|_| "the original dispatcher issuer was lost")?;
    let original = answer(&world, thread, call).await?;
    let after = dispatcher
        .issue(
            machine,
            &SerialKey::Environment(world.environment),
            &MachineProbe {},
        )
        .await?
        .id;
    assert!(matches!(
        within(dispatcher.wait(after)).await??,
        CommandOutcome::Succeeded(_)
    ));
    assert!(server.entry(command).is_none());
    assert_eq!(endpoint.reads.load(Ordering::SeqCst), 0);
    assert_eq!(endpoint.writes.load(Ordering::SeqCst), 0);
    assert!(!endpoint.destination.exists());
    let retained = rows::command(&mut *world.pool.acquire().await?, command)
        .await?
        .ok_or("the original command was lost")?;
    assert_eq!(
        (
            retained.state,
            retained.body,
            retained.serial_key,
            retained.deadline_at
        ),
        (
            "cancelled".into(),
            saved.body,
            saved.serial_key,
            saved.deadline_at
        )
    );
    assert_eq!(selected_snapshot(&world, thread, document).await?, before);
    assert_eq!(world.turn_starts(thread).len(), 2);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM usage_events WHERE turn_id=$1",
                start.turn_id
            )
            .await,
        1
    );
    let attempt = new_id();
    world
        .machine
        .publish(
            &stream,
            &Event::OrchToolCall(OrchToolCall {
                turn_id: start.turn_id,
                call_ref: call.into(),
                attempt: Some(attempt),
                tool: OrchTool::DownloadShared,
                arguments,
            }),
        )
        .await;
    let mut expected = original;
    expected.attempt = Some(attempt);
    assert_eq!(
        answer_attempt(&world, thread, call, Some(attempt)).await?,
        expected
    );
    assert_eq!(selected_snapshot(&world, thread, document).await?, before);
    link.shutdown();
    peer.join().await?;
    drop(server);
    drop(endpoint);
    world.shutdown().await;
    Ok(())
}
