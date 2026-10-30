//! The sockets of shells: attached with a ticket that is good once, typing and output in both directions, a shell left and taken up again as it was, one screen at a time, a resize that reaches the program, the end of the process, a browser that is slow, a shell nobody came back to, a person who loses the right to a shell, an environment that stops, and the agent's own terminal watched and typed into.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;
mod terminals_support;

use chrono::Duration;
use orch_api_contract::terminals::TerminalState;
use orch_link_contract::events::{Event, PtyExited};
use orch_link_contract::streams::Stream;
use serde_json::json;
use std::sync::atomic::Ordering;
use terminals_support::{Floor, Heard, PATIENCE, Screen};

/// A shell of Dan's with a screen on it, ready.
async fn attached(floor: &Floor) -> (orch_types::Id, Screen) {
    let made = floor.shell_of(&floor.scene.dan).await;
    let mut screen = floor.screen(made.session.id, &made.ticket).await;
    screen.ready().await;
    (made.session.id, screen)
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_counts_exact_binary_and_utf8_bytes_across_a_gap_and_excludes_immediate_live_output()
{
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let shell = floor.fake.shell(made.session.id).unwrap();
    let payload = ["汉字 🦀\r\n".as_bytes(), &[0, 255, 254, 0, 17]].concat();
    shell.print_bytes_alone(&payload);
    *shell.replay_pause.lock().unwrap() = Some(std::time::Duration::from_millis(200));
    *shell.live_after_replay.lock().unwrap() = b"live after the captured boundary\r\n".to_vec();
    let mut screen = floor.screen(made.session.id, &made.ticket).await;
    let ready = screen.ready().await;
    assert_eq!(
        ready["replay_bytes"].as_u64(),
        Some(u64::try_from(payload.len()).unwrap())
    );
    assert_eq!(
        screen.shown, payload,
        "replay is measured in bytes and excludes the header and the live suffix"
    );
    screen.until("live after the captured boundary").await;
    screen.drop_it().await;
    let ticket = floor.ticket(&floor.scene.dan, made.session.id).await.ok();
    let mut again = floor.screen(made.session.id, &ticket.ticket).await;
    let ready = again.ready().await;
    assert_eq!(
        ready["replay_bytes"].as_u64(),
        Some(u64::try_from(payload.len()).unwrap())
    );
    assert_eq!(again.shown, payload);
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_page_shows_the_selected_native_foreground_state_and_returns_to_idle() {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let shell = floor.fake.shell(made.session.id).unwrap();
    assert!(!floor.list(&floor.scene.dan, Some("open")).await[0].busy);
    shell.busy.store(true, Ordering::SeqCst);
    assert!(floor.list(&floor.scene.dan, Some("open")).await[0].busy);
    shell.busy.store(false, Ordering::SeqCst);
    assert!(!floor.list(&floor.scene.dan, Some("open")).await[0].busy);
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_attached_with_its_ticket_carries_what_is_typed_and_what_it_prints() {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let mut screen = floor.screen(made.session.id, &made.ticket).await;
    let ready = screen.ready().await;
    assert_eq!(ready["cols"], 100);
    assert_eq!(ready["rows"], 30);
    assert_eq!(
        ready["replay_bytes"], 0,
        "a shell that has printed nothing has nothing to replay"
    );
    screen.type_("echo hello there\n").await;
    let shown = screen.until("hello there\r\n").await;
    assert!(
        shown.starts_with("echo hello there\n"),
        "the terminal echoes the keys: {shown:?}"
    );
    floor
        .wait_for(made.session.id, "the shell is attached", |row| row.attached)
        .await;
    let listed = floor.list(&floor.scene.dan, Some("open")).await;
    assert!(listed[0].attached && listed[0].closes_at.is_none() && listed[0].detached_at.is_none());
    // The use is written to its row, and ping is answered with the number it carries.
    screen.send(json!({ "op": "ping", "n": 41 })).await;
    let pong = screen.frame().await;
    assert_eq!(pong, json!({ "op": "pong", "n": 41 }));
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_is_good_once_for_the_shell_it_was_given_for_and_for_thirty_seconds() {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let other = floor.shell_of(&floor.scene.dan).await;
    let addr = floor.serve().await;
    // A ticket used once is spent.
    let mut first = floor.screen(made.session.id, &made.ticket).await;
    first.ready().await;
    let mut again = floor.screen(made.session.id, &made.ticket).await;
    assert_eq!(again.until_the_end().await, Some(4401));
    // A ticket for one shell opens no other.
    let mut wrong = floor
        .screen(
            other.session.id,
            &floor
                .ticket(&floor.scene.dan, made.session.id)
                .await
                .ok()
                .ticket,
        )
        .await;
    assert_eq!(wrong.until_the_end().await, Some(4401));
    // A ticket that ran out opens nothing.
    let late = floor.ticket(&floor.scene.dan, other.session.id).await.ok();
    floor.scene.rig.world.advance(Duration::seconds(31));
    let mut stale = floor.screen(other.session.id, &late.ticket).await;
    assert_eq!(stale.until_the_end().await, Some(4401));
    // A client that speaks none of the protocol is told so, with the code that says it.
    let fresh = floor.ticket(&floor.scene.dan, other.session.id).await.ok();
    let mut foreign = Screen::connect_offering(
        addr,
        &format!(
            "/api/v1/ws/terminals/{}?ticket={}",
            other.session.id, fresh.ticket
        ),
        &["orch.v1"],
    )
    .await;
    assert_eq!(foreign.until_the_end().await, Some(4409));
    // And the shell was none the worse for it all.
    assert!(floor.row(other.session.id).await.is_live());
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_its_person_left_runs_on_and_is_found_as_it_was_with_a_new_ticket() {
    let floor = Floor::new().await;
    let (id, mut first) = attached(&floor).await;
    first.type_("echo one\n").await;
    first.until("one\r\n").await;
    first.drop_it().await;
    floor
        .wait_for(id, "nobody has the shell", |row| {
            !row.attached && row.detached_at.is_some()
        })
        .await;
    assert!(
        floor.row(id).await.is_live(),
        "closing the tab ends nothing"
    );
    // It prints while nobody looks.
    floor
        .fake
        .shell(id)
        .unwrap()
        .print_alone("while you were away\r\n");
    let ticket = floor.ticket(&floor.scene.dan, id).await.ok();
    let mut second = floor.screen(id, &ticket.ticket).await;
    let ready = second.ready().await;
    let replay = usize::try_from(ready["replay_bytes"].as_i64().unwrap()).unwrap();
    assert!(replay > 0 && replay == second.shown.len());
    let shown = String::from_utf8_lossy(&second.shown).into_owned();
    assert!(
        shown.contains("one\r\n") && shown.contains("while you were away"),
        "{shown:?}"
    );
    assert!(
        shown.find("one").unwrap() < shown.find("while").unwrap(),
        "the screen is as it was, in order"
    );
    // What it prints from now on is new, and comes after.
    second.type_("echo two\n").await;
    second.until("two\r\n").await;
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_screen_takes_the_shell_and_the_first_is_told_that_it_went_elsewhere() {
    let floor = Floor::new().await;
    let (id, mut first) = attached(&floor).await;
    first.type_("echo before\n").await;
    first.until("before\r\n").await;
    let ticket = floor.ticket(&floor.scene.dan, id).await.ok();
    let mut second = floor.screen(id, &ticket.ticket).await;
    let ready = second.ready().await;
    assert!(
        ready["replay_bytes"].as_i64().unwrap() > 0,
        "the second screen is given the screen as it is"
    );
    assert!(String::from_utf8_lossy(&second.shown).contains("before"));
    assert_eq!(first.until_the_end().await, Some(1000));
    assert_eq!(first.closed_reason().as_deref(), Some("attached_elsewhere"));
    // The shell itself is untouched, and the second screen types into it.
    assert!(floor.row(id).await.is_live());
    second.type_("echo after\n").await;
    second.until("after\r\n").await;
    floor
        .wait_for(id, "the shell stays attached", |row| row.attached)
        .await;
    assert!(
        !String::from_utf8_lossy(&first.shown).contains("after"),
        "the first screen was cut off"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_size_of_the_screen_reaches_the_program_and_the_row_of_the_shell() {
    let floor = Floor::new().await;
    let (id, mut screen) = attached(&floor).await;
    screen
        .send(json!({ "op": "resize", "cols": 132, "rows": 43 }))
        .await;
    let mut seen = (0, 0);
    for _ in 0..200 {
        seen = *floor.fake.shell(id).unwrap().size.lock().unwrap();
        if seen == (132, 43) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(seen, (132, 43), "the program was told");
    floor
        .wait_for(id, "the row has the size", |row| {
            (row.cols, row.rows) == (132, 43)
        })
        .await;
    assert_eq!((floor.list(&floor.scene.dan, None).await[0].cols), 132);
    // A size no screen has is let go by, and the connection stands.
    screen
        .send(json!({ "op": "resize", "cols": 0, "rows": 5000 }))
        .await;
    screen.send(json!({ "op": "ping", "n": 1 })).await;
    assert_eq!(screen.frame().await["op"], "pong");
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frame_that_is_no_frame_of_the_protocol_ends_the_connection_and_not_the_shell() {
    let floor = Floor::new().await;
    let (id, mut screen) = attached(&floor).await;
    screen.send(json!({ "op": "dance" })).await;
    assert_eq!(screen.until_the_end().await, Some(4400));
    floor
        .wait_for(id, "the shell is left", |row| !row.attached)
        .await;
    assert!(floor.row(id).await.is_live());
    floor.stop().await;
}

/// The times of a server that notices a silent browser within a second or two.
fn quick_to_notice(tuning: &mut orch_api_core::realtime::Tuning) {
    tuning.silence = std::time::Duration::from_millis(1_200);
    tuning.tick = std::time::Duration::from_millis(100);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_says_nothing_for_too_long_is_let_go_and_its_shell_runs_on() {
    let floor = Floor::tuned(quick_to_notice).await;
    // The screen is neither read nor written to from here on: it answers nothing, as a tab on a machine
    // that went to sleep.
    let (id, silent) = attached(&floor).await;
    floor
        .wait_for(id, "nobody has the shell", |row| {
            !row.attached && row.detached_at.is_some()
        })
        .await;
    assert!(
        floor.row(id).await.is_live(),
        "a browser that went away ends nothing"
    );
    drop(silent);
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_keeps_pinging_is_never_let_go_for_silence() {
    let floor = Floor::tuned(quick_to_notice).await;
    let (id, mut screen) = attached(&floor).await;
    // Four times as long as the silence that lets a browser go, with something said every third of it.
    for n in 0..12 {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        screen.send(json!({ "op": "ping", "n": n })).await;
        assert_eq!(screen.frame().await, json!({ "op": "pong", "n": n }));
    }
    assert!(floor.row(id).await.attached);
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn when_the_process_exits_the_screen_is_told_the_code_and_the_list_shows_the_shell_gone() {
    let floor = Floor::new().await;
    let (id, mut screen) = attached(&floor).await;
    screen.type_("exit 3\n").await;
    let code = screen.until_the_end().await;
    assert_eq!(code, Some(1000));
    let frames: Vec<_> = screen
        .frames
        .iter()
        .filter(|frame| frame["op"] != "ready")
        .cloned()
        .collect();
    assert_eq!(
        frames,
        [
            json!({ "op": "exit", "code": 3 }),
            json!({ "op": "closed", "reason": "exited" })
        ]
    );
    let row = floor.row(id).await;
    assert_eq!(row.state, "closed");
    assert_eq!(
        (row.close_reason.as_deref(), row.exit_code),
        (Some("exited"), Some(3))
    );
    assert!(floor.list(&floor.scene.dan, Some("open")).await.is_empty());
    let gone = floor.list(&floor.scene.dan, Some("closed")).await;
    assert_eq!(gone[0].state, TerminalState::Closed);
    assert_eq!(gone[0].exit_code, Some(3));
    let lines = floor.history().await;
    assert!(
        lines
            .iter()
            .any(|(action, summary)| action == "terminal.closed"
                && summary.contains("it exited with 3")),
        "{lines:?}"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_closed_by_someone_else_tells_its_screen_who_ended_it() {
    let floor = Floor::new().await;
    let (id, mut screen) = attached(&floor).await;
    floor.close(&floor.scene.eve, id).await.ok();
    assert_eq!(screen.until_the_end().await, Some(1000));
    assert_eq!(screen.closed_reason().as_deref(), Some("closed_by_user"));
    assert!(
        !screen.ops().contains(&"exit".to_owned()),
        "it was closed, and not exited: {:?}",
        screen.ops()
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_that_does_not_read_holds_up_its_own_shell_and_nothing_else_and_loses_nothing() {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let addr = floor.serve().await;
    let mut slow = Screen::connect_cramped(
        addr,
        &format!(
            "/api/v1/ws/terminals/{}?ticket={}",
            made.session.id, made.ticket
        ),
        4096,
    )
    .await;
    slow.ready().await;
    let flood: usize = 24 * 1024 * 1024;
    slow.type_(&format!("flood {flood}\n")).await;
    let shell = floor.fake.shell(made.session.id).unwrap();
    // Wait for an actual pending tunnel write, not for enough wall-clock time to fill a buffer.
    tokio::time::timeout(PATIENCE, async {
        while !shell.waiting_for_tunnel.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the slow reader holds a tunnel write pending");
    let written = usize::try_from(shell.written_to_tunnel.load(Ordering::SeqCst)).unwrap();
    assert!(
        written < flood,
        "the shell waits to write when the browser is behind: it wrote {written} of {flood} bytes"
    );
    // Another person's shell is not held up by it.
    let other = floor.shell_of(&floor.scene.eve).await;
    let mut quick = floor.screen(other.session.id, &other.ticket).await;
    quick.ready().await;
    quick.type_("echo quick\n").await;
    quick.until("quick\r\n").await;
    assert!(
        shell.written_to_tunnel.load(Ordering::SeqCst) < flood as i64,
        "the other shell answers while the first remains backpressured"
    );
    // The browser catches up, and every byte is there, in order, once.
    let mut x = 0_usize;
    let mut text = Vec::new();
    loop {
        match slow.hear_within(PATIENCE).await {
            Some(Heard::Bytes(bytes)) => {
                x += bytes.iter().filter(|byte| **byte == b'x').count();
                text.extend_from_slice(&bytes[bytes.len().saturating_sub(16)..]);
                if String::from_utf8_lossy(&text).contains("flooded") {
                    break;
                }
            }
            Some(Heard::Text(_)) => {}
            other => panic!("the flood stopped short: {other:?} after {x} bytes"),
        }
    }
    // The command itself echoed one `x`... of its name: `flood`. Count only what the flood printed.
    let in_command = "flood".matches('x').count();
    assert_eq!(x - in_command, flood, "byte-exact under back-pressure");
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_nobody_came_back_to_is_closed_after_its_time_and_not_before_and_its_last_output_is_in_the_history()
 {
    let floor = Floor::new().await;
    floor.serve().await;
    let dan = &floor.scene.dan;
    floor.store.knows_secret("sekrit-value");
    let made = floor.shell_of(dan).await;
    let id = made.session.id;
    *floor.fake.kept.lock().unwrap() = Some(
        floor
            .store
            .uploaded(b"last words with sekrit-value in them\n"),
    );
    // Seven hours and fifty-nine minutes of nobody is not eight hours.
    floor.scene.rig.world.advance(Duration::minutes(8 * 60 - 1));
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
    assert!(floor.row(id).await.is_live(), "not before");
    floor.scene.rig.world.advance(Duration::minutes(2));
    floor
        .wait_for(id, "the shell nobody came back to is closed", |row| {
            row.state == "closed"
        })
        .await;
    let row = floor.row(id).await;
    assert_eq!(row.close_reason.as_deref(), Some("detached"));
    assert_eq!(
        floor.fake.closed.lock().unwrap().as_slice(),
        [id],
        "the machine closed it"
    );
    assert!(row.recording_blob.is_some());
    let lines = floor.history().await;
    let closed = &lines
        .iter()
        .find(|(action, _)| action == "terminal.closed")
        .unwrap()
        .1;
    assert!(
        closed.contains("nobody came back to it") && closed.contains("the last of it kept"),
        "{closed}"
    );
    let listed = floor.list(dan, Some("closed")).await;
    assert!(listed[0].recorded && listed[0].close_reason.as_deref() == Some("detached"));
    // Its opener reads what it last printed, with the secret taken out.
    let read = floor
        .scene
        .rig
        .world
        .app
        .call(orch_api_contract::terminals::TerminalRecordingGet, &[&id])
        .token(&dan.token)
        .send_bytes()
        .await;
    assert_eq!(
        String::from_utf8_lossy(&read.raw),
        "last words with [secret] in them\n"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_nobody_is_attached_to_is_not_closed_for_it_while_somebody_is() {
    let floor = Floor::new().await;
    let (id, _screen) = attached(&floor).await;
    floor.scene.rig.world.advance(Duration::hours(12));
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
    assert!(
        floor.row(id).await.is_live(),
        "a shell on a screen is not left alone"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_loses_what_opened_a_shell_has_it_closed_attached_or_not() {
    let floor = Floor::new().await;
    let (attached_one, mut screen) = attached(&floor).await;
    let left_alone = floor.shell_of(&floor.scene.dan).await.session.id;
    let eves = floor.shell_of(&floor.scene.eve).await.session.id;
    let org = floor.scene.tree.org.base.id;
    let mut conn = floor.pool().acquire().await.unwrap();
    let role: orch_types::Id = sqlx::query_scalar(
        "SELECT id FROM node_roles WHERE node_id = $1 AND subject_id = $2 AND removed_at IS NULL",
    )
    .bind(org)
    .bind(floor.scene.dan.id())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    orch_db::queries::access::testing::take_role_away(
        &mut conn,
        role,
        floor.scene.ada.id(),
        floor.scene.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::bump_epoch(&mut conn)
        .await
        .unwrap();
    drop(conn);
    for shell in [attached_one, left_alone] {
        floor
            .wait_for(shell, "the shell is closed", |row| row.state == "closed")
            .await;
        assert_eq!(
            floor.row(shell).await.close_reason.as_deref(),
            Some("revoked")
        );
    }
    assert_eq!(screen.until_the_end().await, Some(1000));
    assert_eq!(screen.closed_reason().as_deref(), Some("revoked"));
    assert!(
        floor.row(eves).await.is_live(),
        "somebody else's shell is none the worse for it"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shells_of_an_environment_that_stopped_are_closed_with_it() {
    let floor = Floor::new().await;
    let (attached_one, mut screen) = attached(&floor).await;
    let left_alone = floor.shell_of(&floor.scene.eve).await.session.id;
    sqlx::query("UPDATE environments SET status = 'stopped' WHERE id = $1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    for shell in [attached_one, left_alone] {
        floor
            .wait_for(shell, "the shell is closed", |row| row.state == "closed")
            .await;
        assert_eq!(
            floor.row(shell).await.close_reason.as_deref(),
            Some("environment_stopped")
        );
    }
    assert_eq!(screen.until_the_end().await, Some(1000));
    assert_eq!(
        screen.closed_reason().as_deref(),
        Some("environment_stopped")
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_made_again_with_its_environment_is_closed_and_one_whose_line_broke_is_not() {
    let floor = Floor::new().await;
    let left = floor.shell_of(&floor.scene.dan).await.session.id;
    sqlx::query("UPDATE environments SET status = 'offline' WHERE id = $1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    floor.serve().await;
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
    assert!(
        floor.row(left).await.is_live(),
        "an environment out of reach for now keeps its shells"
    );
    sqlx::query(
        "UPDATE environments SET status = 'running', generation = generation + 1 WHERE id = $1",
    )
    .bind(floor.environment)
    .execute(floor.pool())
    .await
    .unwrap();
    floor
        .wait_for(left, "closed", |row| row.state == "closed")
        .await;
    assert_eq!(
        floor.row(left).await.close_reason.as_deref(),
        Some("environment_stopped")
    );
    floor.stop().await;
}

/// The machine says, on the environment's stream, that a shell ended while nobody was attached.
async fn machine_says_exited(
    floor: &Floor,
    shell: orch_types::Id,
    exit_code: Option<i64>,
    bytes_out: i64,
    kept: Option<orch_link_contract::commands::BlobRef>,
) {
    floor
        .scene
        .rig
        .machine
        .publish(
            &Stream::Env {
                environment_id: floor.environment,
            },
            &Event::PtyExited(PtyExited {
                terminal_id: shell,
                exit_code,
                bytes_out,
                kept,
            }),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_exited_while_nobody_was_attached_is_closed_as_exited_with_its_code_and_what_it_kept()
 {
    let floor = Floor::new().await;
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    let kept = floor.store.uploaded(b"all done\n");
    machine_says_exited(&floor, id, Some(7), 9, Some(kept)).await;
    floor
        .wait_for(id, "closed by the machine's word", |row| {
            row.state == "closed"
        })
        .await;
    let row = floor.row(id).await;
    assert_eq!(
        (row.close_reason.as_deref(), row.exit_code, row.bytes_out),
        (Some("exited"), Some(7), 9)
    );
    assert!(row.recording_blob.is_some());
    let lines = floor.history().await;
    assert!(
        lines
            .iter()
            .any(|(action, summary)| action == "terminal.closed"
                && summary.contains("it exited with 7")
                && summary.contains("9 bytes printed")),
        "{lines:?}"
    );
    // Told again, it changes nothing: a shell is closed once.
    machine_says_exited(&floor, id, Some(9), 99, None).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(floor.row(id).await.exit_code, Some(7));
    assert_eq!(floor.details("terminal.closed").await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_its_connector_no_longer_has_is_closed_as_lost_by_the_link() {
    let floor = Floor::new().await;
    let id = floor.shell_of(&floor.scene.dan).await.session.id;
    machine_says_exited(&floor, id, None, 0, None).await;
    floor
        .wait_for(id, "closed", |row| row.state == "closed")
        .await;
    assert_eq!(
        floor.row(id).await.close_reason.as_deref(),
        Some("link_lost")
    );
    // And a shell whose connector was restarted under it, found out when its person comes back.
    let made = floor.shell_of(&floor.scene.dan).await;
    floor.fake.forget_shells();
    let mut screen = floor.screen(made.session.id, &made.ticket).await;
    assert_eq!(screen.until_the_end().await, Some(1000));
    assert_eq!(screen.closed_reason().as_deref(), Some("link_lost"));
    assert_eq!(
        floor.row(made.session.id).await.close_reason.as_deref(),
        Some("link_lost")
    );
    floor.stop().await;
}

/// Lets the agent of the scene's threads declare a terminal of its own.
async fn agent_declares(floor: &Floor, declared: &str) {
    sqlx::query(
        "UPDATE agents SET descriptor = jsonb_set(descriptor, '{capabilities,agent_terminal}', to_jsonb($2::text))
          WHERE id = $1",
    )
    .bind(floor.scene.on.agent)
    .bind(declared)
    .execute(floor.pool())
    .await
    .unwrap();
}

async fn agent_ticket(floor: &Floor, who: &places::Who, control: bool) -> String {
    floor
        .scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::terminals::AgentTerminalTicketCreate,
            &[&floor.thread],
        )
        .token(&who.token)
        .body(
            &orch_api_contract::terminals::AgentTerminalTicketCreateBody {
                control: Some(control),
            },
        )
        .send()
        .await
        .ok()
        .ticket
}

#[tokio::test(flavor = "multi_thread")]
async fn a_view_only_agent_terminal_ignores_typing_and_resize_and_closes_for_a_malformed_control_frame()
 {
    let floor = Floor::new().await;
    agent_declares(&floor, "interactive").await;
    let ticket = agent_ticket(&floor, &floor.scene.cai, false).await;
    let mut watcher = floor.agent_screen(&ticket).await;
    watcher.ready().await;
    let shell = floor.fake.shell(floor.thread).unwrap();
    let size = *shell.size.lock().unwrap();
    watcher.type_("ignored input\n").await;
    watcher
        .send(json!({"op": "resize", "cols": 132, "rows": 43}))
        .await;
    watcher.send(json!({"op": "ping", "n": 81})).await;
    loop {
        if watcher.frame().await["op"] == "pong" {
            break;
        }
    }
    assert_eq!(shell.typed_text(), "");
    assert_eq!(*shell.size.lock().unwrap(), size);
    watcher
        .send(json!({"op": "resize", "cols": "wide", "rows": 43}))
        .await;
    assert_eq!(watcher.until_the_end().await, Some(4400));
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_control_frame_with_an_unknown_operation_or_member_is_malformed() {
    let floor = Floor::new().await;
    for frame in [
        json!({"op": "signal", "signal": "INT"}),
        json!({"op": "ping", "n": 1, "extra": true}),
    ] {
        let (id, mut screen) = attached(&floor).await;
        screen.send(frame).await;
        assert_eq!(screen.until_the_end().await, Some(4400));
        assert!(
            floor.row(id).await.is_live(),
            "a malformed attachment does not kill its shell"
        );
    }
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agents_own_terminal_is_watched_without_typing_and_typed_into_only_on_a_ticket_with_control()
 {
    let floor = Floor::new().await;
    agent_declares(&floor, "interactive").await;
    // The harness's screen, as the machine keeps it.
    let watching = agent_ticket(&floor, &floor.scene.cai, false).await;
    let mut watcher = floor.agent_screen(&watching).await;
    // The machine opens the agent's terminal when the watcher comes.
    watcher.frame().await;
    floor
        .fake
        .shell(floor.thread)
        .unwrap()
        .print_alone("harness screen\r\n");
    watcher.type_("rm -rf everything\n").await;
    watcher.send(json!({ "op": "ping", "n": 5 })).await;
    loop {
        let frame = watcher.frame().await;
        if frame["op"] == "pong" {
            break;
        }
    }
    assert_eq!(
        floor.fake.shell(floor.thread).unwrap().typed_text(),
        "",
        "a watcher's keys are not passed on"
    );
    assert_eq!(
        floor.fake.agent_opened.lock().unwrap().as_slice(),
        [(floor.thread, false)]
    );
    watcher.drop_it().await;
    // With control, what is typed reaches the harness, and the audit trail has the count.
    let controlling = agent_ticket(&floor, &floor.scene.dan, true).await;
    let mut driver = floor.agent_screen(&controlling).await;
    let ready = driver.ready().await;
    assert!(
        ready["replay_bytes"].as_i64().unwrap() > 0,
        "the screen as it is"
    );
    assert!(String::from_utf8_lossy(&driver.shown).contains("harness screen"));
    driver.type_("echo steered\n").await;
    driver.until("steered\r\n").await;
    assert_eq!(
        floor.fake.shell(floor.thread).unwrap().typed_text(),
        "echo steered\n"
    );
    driver.drop_it().await;
    let mut noted = Vec::new();
    for _ in 0..200 {
        noted = floor.details("terminal.closed").await;
        if !noted.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    sqlx::query("SELECT 1").execute(floor.pool()).await.unwrap();
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE action = 'terminal.closed' AND details ->> 'agent_terminal' = 'true'",
    )
    .fetch_all(floor.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "{noted:?}");
    assert_eq!(rows[0]["bytes_typed"], 13);
    let address: String = sqlx::query_scalar(
        "SELECT host(ip) FROM audit_log WHERE action = 'terminal.closed' AND details ->> 'agent_terminal' = 'true'")
        .fetch_one(floor.pool()).await.unwrap();
    assert_eq!(
        address, "127.0.0.1",
        "typing retains the real socket's resolved origin"
    );
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_for_the_agents_terminal_opens_no_shell_and_a_shells_ticket_opens_no_agent_terminal()
 {
    let floor = Floor::new().await;
    agent_declares(&floor, "interactive").await;
    let shell = floor.shell_of(&floor.scene.dan).await;
    let agent = agent_ticket(&floor, &floor.scene.dan, true).await;
    let mut wrong = floor.screen(shell.session.id, &agent).await;
    assert_eq!(wrong.until_the_end().await, Some(4401));
    let mut wrong = floor.agent_screen(&shell.ticket).await;
    assert_eq!(wrong.until_the_end().await, Some(4401));
    assert!(floor.row(shell.session.id).await.is_live());
    floor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_screen_on_a_shell_that_its_person_gives_up_for_a_new_one_is_told_that_it_was_closed() {
    let floor = Floor::new().await;
    let dan = &floor.scene.dan;
    let (given_up, mut screen) = attached(&floor).await;
    for _ in 0..3 {
        floor.shell_of(dan).await;
    }
    floor
        .open_with(
            dan,
            Some(floor.thread),
            json!({ "cols": 80, "rows": 24, "close": [given_up] }),
        )
        .await
        .ok();
    assert_eq!(screen.until_the_end().await, Some(1000));
    assert_eq!(screen.closed_reason().as_deref(), Some("closed_by_user"));
    assert!(
        !screen.ops().contains(&"exit".to_owned()),
        "{:?}",
        screen.ops()
    );
    floor.stop().await;
}
