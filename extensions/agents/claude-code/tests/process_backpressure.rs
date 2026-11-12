//! Exit/control must settle while the bounded native output lane still belongs to a slow consumer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]
mod common;

use common::corpus::{Scripts, WAIT};
use orch_adapter_claude_code::process::{LaunchSpec, ProcessEvent, event_channel, spawn};
use orch_agent_contract::flow::QUEUE_ITEMS;
use orch_testkit::harness_corpus::{Bindings, Frame};
use serde_json::json;

async fn pressure(natural_exit: bool) {
    // Twice the real node capacity forces the production stdout reader to wait before it sees EOF.
    let count = QUEUE_ITEMS * 2;
    let mut frames: Vec<_> = (0..count)
        .map(|position| Frame::FromHarness {
            frame: json!({"type":"keep_alive","position":position}),
        })
        .collect();
    frames.push(Frame::Stderr {
        line: "Final fixture diagnostic".into(),
    });
    if natural_exit {
        frames.push(Frame::Exit { code: Some(7) });
    }
    let scripts = Scripts::new(vec![frames], orch_testkit::clock::manual(), Bindings::new());
    let home = tempfile::tempdir().unwrap();
    let launch = LaunchSpec {
        program: "corpus-never-executed".into(),
        args: vec![],
        env: Default::default(),
        cwd: home.path().to_owned(),
        label: "native-output-pressure".into(),
    };
    let (send, mut events) = event_channel();
    let retained = send.clone();
    let mut running = spawn(scripts.as_ref(), &launch, 7, send).unwrap();
    tokio::time::timeout(WAIT, async {
        while retained.usage().items != QUEUE_ITEMS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the native reader must actually reach its configured capacity");
    scripts.completed().await;
    let ((generation, first), held) = events.recv_held().await.unwrap();
    assert_eq!(generation, 7);
    let ProcessEvent::Line(first) = first else {
        panic!("the first native line must be retained");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&first).unwrap(),
        json!({"type":"keep_alive","position":0})
    );
    assert_eq!(
        retained.usage().items,
        QUEUE_ITEMS,
        "detaching a line cannot release its held capacity"
    );
    if !natural_exit {
        running.kill();
    }
    tokio::time::timeout(WAIT, async {
        while !running.has_exited() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("process exit/control cannot wait behind native output delivery");
    assert_eq!(
        retained.usage().items,
        QUEUE_ITEMS,
        "the consumer still owns the full lane when exit settles"
    );
    drop(held);

    let mut position = 1;
    let ended = tokio::time::timeout(WAIT, async {
        loop {
            let (generation, event) = events
                .recv()
                .await
                .expect("the ordered exit must follow every admitted native line");
            assert_eq!(generation, 7);
            match event {
                ProcessEvent::Line(line) => {
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&line).unwrap(),
                        json!({"type":"keep_alive","position":position})
                    );
                    position += 1;
                }
                ProcessEvent::Exited(ended) => break ended,
                other => panic!("the native transport invented an output error: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        position, count,
        "no line may be discarded to publish the exit"
    );
    assert_eq!(ended.code, if natural_exit { Some(7) } else { None });
    assert_eq!(
        ended.signal, None,
        "an in-memory transport cannot invent an operating-system signal"
    );
    assert_eq!(
        ended.stderr_tail,
        vec!["Final fixture diagnostic".to_owned()]
    );
    assert!(
        !running.write("after exit\n".into()).await,
        "a dead native process cannot take another input"
    );
    scripts.finish().await;
    assert_eq!(retained.usage().items, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_exit_is_visible_before_held_output_capacity_is_released_and_nothing_is_dropped() {
    pressure(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn priority_cancellation_reaches_the_exact_native_process_while_output_capacity_is_held() {
    pressure(false).await;
}
