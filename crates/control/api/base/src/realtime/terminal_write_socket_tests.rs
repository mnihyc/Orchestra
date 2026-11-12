//! Actual HTTP→WebSocket→Pump→full native channel, on unchanged old or corrected Source.
//! No product IO algorithm, future origin constructor, or fabricated acting caller is copied.

use super::fixture as tickets;

use crate::rig::eventually;

use floor::Floor;

use floor::Heard;

use floor::PATIENCE;

use floor::Screen;

use orch_api_contract::terminals::TerminalTicketCreate;

use orch_link_contract::tunnels::REPLAY_BUFFER_BYTES;

use orch_link_contract::version::Limits;

use std::sync::atomic::AtomicU64;

use std::sync::atomic::Ordering;

use std::time::Instant;
pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn full_buffer(withdraw: bool) -> Proof {
    let mut trace =
        crate::support::failure_trace::FailureTrace::new("terminal full native Pump", PATIENCE);
    trace.mark(format!("prepare scene; withdraw={withdraw}"));
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let id = made.session.id;
    let key = tickets::legacy(&floor.scene.rig.world, floor.scene.dan.id()).await?;
    let reply = floor
        .scene
        .rig
        .world
        .app
        .call(TerminalTicketCreate, &[&id])
        .token(&key.secret)
        .send_bytes()
        .await;
    let token = tickets::ticket(&reply, "actual full-buffer legacy HTTP ticket positive")?;
    let address = floor.serve().await;
    let mut screen = Screen::connect(
        address,
        &format!("/openapi/v1/ws/terminals/{id}?ticket={token}"),
    )
    .await;
    screen.ready().await;
    const POSITIVE: &str = "echo actual-full-buffer-positive\n";
    screen.type_(POSITIVE).await;
    screen.until("actual-full-buffer-positive\r\n").await;
    eventually(
        "the actual Pump accounts for its successful same-key positive",
        || async { floor.row(id).await.bytes_in >= i64::try_from(POSITIVE.len()).unwrap() },
    )
    .await;
    let before = floor.row(id).await.bytes_in;
    let native = floor
        .fake
        .shell(id)
        .ok_or("actual opened native shell absent")?;
    native.pause_input().await;
    let prior_native = native.typed.lock().unwrap().len();
    // Scene's hub and connector negotiate the shipped default window; PTY is nonresumable,
    // so window bytes leave the sender while the rest fills its public replay buffer.
    let filling = Instant::now();
    let capacity = usize::try_from(REPLAY_BUFFER_BYTES)?
        + usize::try_from(Limits::default().tunnel_window_bytes)?;
    // Use the largest accepted paste frame. Tiny tunnel frames make thousands of
    // independent HTTP-origin checks before the marker can enter the real Pump.
    let chunk = crate::realtime::terminals::MAX_TYPED_FRAME_BYTES;
    assert_eq!(capacity % chunk, 0);
    // This is the existing unchanged Pump's TYPED_QUEUE=8 capacity, not a new timeout/budget.
    // Seven equal prefix frames and the marker fill that queue beyond a FULL sender.
    // When bytes_in reports all of them, the marker is in the real Pump, not in TCP/inbox.
    const TYPED_QUEUE_FRAMES: usize = crate::realtime::terminals::TYPED_QUEUE;
    let prefix = capacity + (TYPED_QUEUE_FRAMES - 1) * chunk;
    let piece = "p".repeat(chunk);
    for _ in 0..prefix / chunk {
        screen.type_(&piece).await;
    }
    const MARKER: &str = "QUEUED-MARKER-MUST-NOT-BORROW-OLD-KEY-AUTHORITY";
    screen.type_(MARKER).await;
    let accounted = before + i64::try_from(prefix + MARKER.len())?;
    trace.mark(format!(
        "full-pump phase=enqueued withdraw={withdraw} elapsed_ms={} before={before} expected={accounted} prefix={prefix} native_before={prior_native}",
        filling.elapsed().as_millis(),
    ));
    let reported = AtomicU64::new(u64::MAX);
    eventually(
        "the actual full native sender and Pump queue hold the marker before withdrawal",
        || async {
            let row = floor.row(id).await;
            let second = filling.elapsed().as_secs();
            if reported.swap(second, Ordering::SeqCst) != second {
                trace.mark(format!(
                    "full-pump phase=waiting withdraw={withdraw} elapsed_ms={} counted={} expected={accounted} native={} native_before={prior_native} state={}",
                    filling.elapsed().as_millis(),
                    row.bytes_in,
                    native.typed.lock().unwrap().len(),
                    row.state,
                ));
            }
            row.bytes_in >= accounted
        },
    )
    .await;
    assert_eq!(
        native.typed.lock().unwrap().len(),
        prior_native,
        "actual paused native reader consumed none of the newly queued prefix/marker"
    );
    if withdraw {
        trace.mark("withdraw the same key's current scope");
        tickets::current_scope(&floor.scene.rig.world, key.id, vec![]).await?;
        let (legacy, modern): (Vec<String>, orch_types::Json) =
            sqlx::query_as("SELECT scopes,operation_scope FROM api_tokens WHERE id=$1")
                .bind(key.id)
                .fetch_one(floor.pool())
                .await?;
        assert!(legacy.is_empty());
        assert_eq!(
            modern["rules"],
            serde_json::json!([]),
            "same actual key now admits no future read or input"
        );
    }
    native.resume_input();
    trace.mark("resume native input and wait for marker progress or authority close");
    let required = prior_native + prefix + MARKER.len();
    // A correct refusal closes this socket. Old Source instead advances the actual marker
    // into the native reader. While waiting, draining output preserves full-duplex credit.
    let mut closed = false;
    loop {
        if native.typed.lock().unwrap().len() >= required {
            break;
        }
        match screen.hear_within(PATIENCE).await {
            Some(Heard::Closed(_)) => {
                closed = true;
                break;
            }
            None => {
                return Err(
                    "native progress or the actual authority close was not observed".into(),
                );
            }
            Some(_) => {}
        }
    }
    screen.drop_it().await;
    trace.mark("socket dropped; wait for native input EOF");
    // EOF is an actual native stream fence: every earlier accepted byte has been consumed,
    // so an absent marker cannot merely be delayed behind the accepted prefix.
    eventually(
        "the actual native attachment consumes its accepted input and reaches EOF",
        || async { native.input_ended.load(Ordering::SeqCst) },
    )
    .await;
    let observed = native.typed.lock().unwrap().clone();
    let row = floor.row(id).await;
    let native_closed = floor.fake.closed.lock().unwrap().clone();
    floor.stop().await;
    let marker_seen = observed
        .windows(MARKER.len())
        .any(|window| window == MARKER.as_bytes());
    if withdraw {
        assert!(
            !marker_seen,
            "actual native reader received queued marker after the same key lost every current rule"
        );
        assert!(
            closed,
            "withdrawn active ticket must close its affected socket"
        );
    } else {
        assert!(
            marker_seen,
            "unchanged-authority full-buffer positive must make real native progress after credit release"
        );
    }
    assert_eq!(
        row.state, "open",
        "key-only loss ends socket authority, not the already opened native shell"
    );
    assert!(
        !native_closed.contains(&id),
        "key-only change is not ordinary opener permission loss"
    );
    trace.finish();
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority() -> Proof {
    full_buffer(false).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::realtime::ticket_scope_tests::terminal_write_socket_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority(),
    )
}

#[cfg(any(test, feature = "test-support"))]
async fn actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal() -> Proof {
    full_buffer(true).await
}
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn __api_run_actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal()
-> ::futures::future::LocalBoxFuture<
    'static,
    crate::realtime::ticket_scope_tests::terminal_write_socket_tests::Proof,
> {
    ::std::boxed::Box::pin(
        self::actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal(),
    )
}

#[path = "../../tests/terminals_support/mod.rs"]
pub mod floor;
