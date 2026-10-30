//! A link that breaks while content moves or a shell is attached: each goes on where it stopped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::time::Duration;

use orch_link::machine::LinkStatus;
use orch_link_contract::commands::{FsFetch, FsList, FsWrite, PtyOpen, WatchSet};
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_testkit::doubles::link::LinkShape;
use orch_types::{Hash, new_id};
use rig_environment::{EnvRig, PATIENCE};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn pattern(size: usize) -> Vec<u8> {
    (0..size)
        .map(|n| ((n * 31 + n / 251) % 251) as u8)
        .collect()
}

#[tokio::test]
async fn a_file_that_goes_up_while_the_content_line_fails_goes_on_from_where_it_stopped() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    let content = pattern(3 * 1024 * 1024);
    std::fs::write(workspace.join("big.bin"), &content).unwrap();
    rig.start().await;
    rig.attach(&workspace).await;
    // The third and the sixth piece are lost on the way, after they were sent.
    rig.blobs.cut_put(2);
    rig.blobs.cut_put(5);
    let fetched = rig
        .succeed(&FsFetch {
            environment_id: rig.environment,
            mount: None,
            path: "big.bin".to_owned(),
            expect: None,
            max_bytes: Some(i64::try_from(content.len()).unwrap()),
        })
        .await;
    let hash: Hash = serde_json::from_value(fetched["hash"].clone()).unwrap();
    assert_eq!(hash, Hash::of(&content));
    assert_eq!(
        rig.blobs.content(&hash).unwrap().as_ref(),
        content.as_slice()
    );
    // Every byte was taken once: the pieces that were lost were sent again, none that arrived was.
    let taken: usize = rig
        .blobs
        .requests()
        .iter()
        .filter(|(kind, _)| kind == "put")
        .map(|(_, bytes)| *bytes)
        .sum();
    assert_eq!(
        taken,
        content.len(),
        "the pieces that arrived add up to the file and no more"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_file_that_comes_down_while_the_session_is_cut_is_written_whole_and_the_command_answers_once()
 {
    let mut rig = EnvRig::new();
    // A thin line: the file takes seconds to come, long enough for the session to be cut meanwhile.
    rig.content_line = Some(LinkShape {
        round_trip: Duration::from_millis(20),
        up_bytes_per_second: 1_000_000,
        down_bytes_per_second: 600_000,
    });
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let content = pattern(1_500_000);
    let blob = rig.stored(&content);

    let command = new_id();
    rig.blobs.open_command(command);
    rig.controller.issue_with_id(
        command,
        &rig.key_of::<FsWrite>(),
        rig.deadline(),
        &FsWrite {
            work: None,
            environment_id: rig.environment,
            path: "came.bin".to_owned(),
            blob,
            mode: None,
            expected_hash: None,
        },
    );
    tokio::time::sleep(Duration::from_millis(700)).await;
    rig.hub.faults().sever_now();
    let ended = tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
        .await
        .expect("the command never ended");
    assert!(
        matches!(ended, orch_link::commands::Progress::Succeeded(_)),
        "{ended:?}"
    );
    assert_eq!(std::fs::read(workspace.join("came.bin")).unwrap(), content);
    // The command ran once; asked again by its identifier it is answered from the journal.
    let listing = rig
        .succeed(&FsList {
            environment_id: rig.environment,
            mount: None,
            path: String::new(),
            watch_id: None,
            watch_instance: None,
            page: None,
        })
        .await;
    assert_eq!(listing["entries"].as_array().unwrap().len(), 1);
    rig.stop().await;
}

#[tokio::test]
async fn a_shell_goes_on_while_the_link_is_cut_and_is_taken_up_again_with_what_it_printed_meanwhile()
 {
    let mut rig = EnvRig::new();
    rig.config.environment.terminal = true;
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let terminal_id = new_id();
    rig.succeed(&PtyOpen {
        environment_id: rig.environment,
        terminal_id,
        cols: 100,
        rows: 30,
        cwd: String::new(),
        record: false,
    })
    .await;
    let ticket = || rig.ticket(TicketSource::Api, Service::Pty { terminal_id }, None, None);
    let mut first = rig
        .hub
        .terminals()
        .open(rig.machine_id, &ticket())
        .await
        .unwrap();
    first
        .write_all(b"sleep 3; echo after-the-cut-$((20+22))\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The link is cut while the command runs: the tunnel ends, the shell does not.
    let before = match &*rig.connector.as_ref().unwrap().link().status().borrow() {
        LinkStatus::Connected { session_id, .. } => *session_id,
        other => panic!("{other:?}"),
    };
    rig.hub.faults().sever_now();
    let mut ended = Vec::new();
    let gone = tokio::time::timeout(PATIENCE, first.read_to_end(&mut ended)).await;
    assert!(gone.is_ok(), "the tunnel of the cut session stayed open");
    let link = rig.connector.as_ref().unwrap().link().clone();
    tokio::time::timeout(PATIENCE, async {
        loop {
            if let LinkStatus::Connected { session_id, .. } = &*link.status().borrow()
                && *session_id != before
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the link never came back");

    // Taken up again, the screen shows what came while nobody was there.
    let mut second = rig
        .hub
        .terminals()
        .open(rig.machine_id, &ticket())
        .await
        .unwrap();
    let mut seen = String::new();
    let mut chunk = [0u8; 4096];
    tokio::time::timeout(PATIENCE, async {
        while !seen.contains("after-the-cut-42") {
            let count = second.read(&mut chunk).await.unwrap();
            assert!(count > 0, "the tunnel ended; the screen showed {seen:?}");
            seen.push_str(&String::from_utf8_lossy(&chunk[..count]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("what the shell printed meanwhile never came: {seen:?}"));
    rig.stop().await;
}

#[tokio::test]
async fn what_the_host_measures_and_watches_goes_beside_the_streams_as_reports() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let watch_id = new_id();
    rig.succeed(&FsList {
        environment_id: rig.environment,
        mount: None,
        path: String::new(),
        watch_id: Some(watch_id),
        watch_instance: None,
        page: None,
    })
    .await;
    rig.succeed(&WatchSet {
        environment_id: rig.environment,
        stats: true,
        procs: true,
        ports: false,
        terminals: false,
        interval_ms: 1000,
    })
    .await;
    std::fs::write(workspace.join("appeared.txt"), "now").unwrap();
    let arrived = rig
        .controller
        .wait_until(PATIENCE, |c| {
            let reports = c.reports();
            ["fs.changed", "stats.sample", "procs.snapshot"]
                .iter()
                .all(|kind| reports.iter().any(|seen| seen == kind))
        })
        .await;
    assert!(arrived, "{:?}", rig.controller.reports());
    let _ = json!(null);
    rig.stop().await;
}
