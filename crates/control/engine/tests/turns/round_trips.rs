//! Long lines charge only the answers that the next native step needs.
use super::*;
use async_trait::async_trait;
use orch_api_contract::workspace::{DirEntry, DirEntryKind, FileStat};
use orch_engine::commands::{
    BeforeCommand, Commands, CommandsError, Info, Outcome, Outgoing, RetainedCommand,
};
use orch_link_contract::commands::CommandResult;
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::link::{Direction, Shape};
use orch_types::{Id, Json, Time};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// The owner's delayed-line profile, independently in each direction.
const ONE_WAY: Duration = Duration::from_millis(700);

#[derive(Default)]
struct Exchanges {
    /// The deepest answer actually consumed by the engine, not one round per command.
    answered: usize,
    issued: HashMap<Id, usize>,
    order: Vec<(Id, String, usize)>,
}

impl Exchanges {
    fn issue(&mut self, command: &Outgoing) {
        if self.issued.contains_key(&command.id) {
            return;
        }
        let depth = self.answered + 1;
        self.issued.insert(command.id, depth);
        self.order
            .push((command.id, command.kind.as_str().to_owned(), depth));
    }

    fn answer(&mut self, id: Id) {
        if let Some(depth) = self.issued.get(&id) {
            self.answered = self.answered.max(*depth);
        }
    }

    fn rounds(&self) -> usize {
        self.issued.values().copied().max().unwrap_or_default()
    }

    fn of_kind(&self, kind: &str) -> Vec<usize> {
        self.order
            .iter()
            .filter_map(|(_, name, depth)| (name == kind).then_some(*depth))
            .collect()
    }

    fn report(&self) -> Vec<(&str, usize)> {
        self.order
            .iter()
            .map(|(_, kind, depth)| (kind.as_str(), *depth))
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Count consumed native answers while retaining the existing issuer, guards and real link.
struct Counted {
    inner: Arc<dyn Commands>,
    exchanges: Arc<Mutex<Exchanges>>,
}

#[async_trait]
impl Commands for Counted {
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        let id = command.id;
        lock(&self.exchanges).issue(&command);
        let result = self.inner.observe(command).await;
        if result.is_ok() {
            lock(&self.exchanges).answer(id);
        }
        result
    }

    async fn issue_prepared(
        &self,
        command: Outgoing,
        before: Option<BeforeCommand<'_>>,
    ) -> Result<(), CommandsError> {
        lock(&self.exchanges).issue(&command);
        self.inner.issue_prepared(command, before).await
    }

    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }

    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }

    async fn retained_return_source(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::credentials::handover_authority::ReturnSource>, CommandsError>
    {
        self.inner.retained_return_source(id).await
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        let result = self.inner.status(id).await?;
        if result.as_ref().is_some_and(|info| info.outcome.is_some()) {
            lock(&self.exchanges).answer(id);
        }
        Ok(result)
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let result = self.inner.wait(id).await;
        if result.is_ok() {
            lock(&self.exchanges).answer(id);
        }
        result
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }

    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.stopped(id).await
    }

    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}

async fn counted_world() -> (World, Arc<Mutex<Exchanges>>) {
    counted_world_for(false).await
}

async fn counted_world_for(keyed: bool) -> (World, Arc<Mutex<Exchanges>>) {
    // Frozen domain time prevents the Stop force timer or a provider sweep from
    // substituting for a genuine reply on this deliberately delayed transport.
    let world = World::from_setup(common::Setup {
        clock: Some(Arc::new(orch_testkit::clock::manual())),
        observed: true,
        key: keyed.then_some("scripted-latency-key-not-a-model-credential"),
        ..common::Setup::default()
    })
    .await;
    let exchanges = Arc::new(Mutex::new(Exchanges::default()));
    let counter = Arc::clone(&exchanges);
    world
        .restart_api_using(move |inner| {
            Arc::new(Counted {
                inner,
                exchanges: counter,
            })
        })
        .await;
    let clock = Arc::clone(&world.clock);
    world
        .machine
        .handler
        .on(CommandKind::FsMkdir, move |command| {
            let Command::FsMkdir(body) = command else {
                panic!("the native directory request arrives");
            };
            Ok(CommandResult::FsMkdir(FileStat {
                facts: None,
                entry: DirEntry {
                    name: body.path.rsplit('/').next().unwrap_or_default().to_owned(),
                    path: body.path.clone(),
                    kind: DirEntryKind::Dir,
                    size: 0,
                    mtime: Time::new(clock.now()),
                    mode: 0o755,
                    link_target: None,
                    cached: false,
                },
                hash: None,
                media_type: "inode/directory".to_owned(),
                is_text: false,
            }))
        });
    (world, exchanges)
}

fn long_line(world: &World) {
    for direction in [Direction::Down, Direction::Up] {
        world.hub.faults().shape(
            direction,
            Some(Shape {
                delay: ONE_WAY,
                // Isolate propagation: this case gives the pipe no bandwidth ceiling or loss.
                bytes_per_second: u64::MAX,
                loss: 0.0,
            }),
        );
    }
}

async fn begin(world: &World, thread: Id) -> orch_link_contract::commands::TurnStart {
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Begin over the long line"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    // Report from the native peer without the fixture's optional session-opening helper.
    world
        .machine
        .publish(
            &Stream::Session {
                thread_id: thread,
                segment_id: world.segment_of(thread).await,
            },
            &common::turn_began(start.turn_id),
        )
        .await;
    world.wait_turn(start.turn_id, "running").await;
    world.wait_state(thread, "running").await;
    start
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cold_turn_reaches_running_in_three_answer_dependent_exchanges_on_a_700ms_each_way_line()
{
    let (world, exchanges) = counted_world().await;
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["instruction_root_marker"] = serde_json::json!(true);
    world.declares(descriptor).await;
    let thread = world.thread("Start over a long line").await;
    *lock(&exchanges) = Exchanges::default();
    long_line(&world);
    let start = begin(&world, thread).await;
    assert!(start.text.contains("Begin over the long line"));
    let (rounds, report) = {
        let counted = lock(&exchanges);
        (counted.rounds(), format!("{:?}", counted.report()))
    };
    world.hub.faults().clear();
    world.shutdown().await;
    eprintln!("start exchanges={rounds}; 700ms each way; native sequence={report}");
    assert!(rounds > 0, "the real native start was counted");
    assert!(
        rounds <= 3,
        "Running needs at most three native round trips plus the harness's own start: {report}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_stops_in_one_exchange_on_a_700ms_each_way_line() {
    let (world, exchanges) = counted_world().await;
    let thread = world.thread("Stop over a long line").await;
    let start = begin(&world, thread).await;
    *lock(&exchanges) = Exchanges::default();
    long_line(&world);
    world
        .engine()
        .stop_thread(&Caller::session(world.ada), thread, Some(start.turn_id))
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    world
        .machine
        .publish(
            &Stream::Session {
                thread_id: thread,
                segment_id: world.segment_of(thread).await,
            },
            &turn_ended(start.turn_id, TurnOutcome::Interrupted),
        )
        .await;
    world.wait_turn(start.turn_id, "interrupted").await;
    let rounds = lock(&exchanges).of_kind("turn.interrupt");
    world.hub.faults().clear();
    world.shutdown().await;
    assert_eq!(rounds, [1], "Stop has one direct native exchange");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_provider_and_its_lease_prepare_with_the_harness_in_three_exchanges() {
    let (world, exchanges) = counted_world_for(true).await;
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["instruction_root_marker"] = serde_json::json!(true);
    world.declares(descriptor).await;
    let thread = world
        .thread("Start with a forwarder over the long line")
        .await;
    *lock(&exchanges) = Exchanges::default();
    long_line(&world);
    let start = begin(&world, thread).await;
    let (rounds, report) = {
        let counted = lock(&exchanges);
        (counted.rounds(), format!("{:?}", counted.report()))
    };
    assert!(world.ran(CommandKind::LeaseOpen) > 0);
    assert!(start.text.contains("Begin over the long line"));
    world.hub.faults().clear();
    world.shutdown().await;
    eprintln!("key-provider exchanges={rounds}; native sequence={report}");
    assert!(
        rounds > 0 && rounds <= 3,
        "provider setup and lease do not add another API round trip: {report}"
    );
}
