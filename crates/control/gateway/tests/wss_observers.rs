//! Actual TLS/WSS observation controls. Loopback execution alone is not a kernel-netem proof.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

#[cfg(feature = "test-observers")]
mod common;

#[cfg(not(feature = "test-observers"))]
#[test]
fn the_dedicated_observer_is_absent_from_default_builds() {
    assert!(!cfg!(feature = "test-observers"));
}

#[cfg(feature = "test-observers")]
mod observed {
    use super::common::{Machine, PATIENCE, World, eventually};
    use orch_link::test_observers::Recording;
    use orch_link::tunnels::channel::{IncomingTunnels, Tunnel};
    use orch_link_contract::commands::{CommandKind, CommandResult, MachineProbe, SerialKey};
    use orch_link_contract::enrollment::Trust;
    use orch_link_contract::session::Role;
    use orch_link_contract::tunnels::{REPLAY_BUFFER_BYTES, Service};
    use orch_link_contract::version::Limits;
    use orch_types::{Time, new_id};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;

    // The same byte-by-offset pattern used by the existing real-TLS tunnel controls.
    fn pattern(seed: u64, from: u64, len: usize) -> Vec<u8> {
        (0..len as u64)
            .map(|i| {
                let value = (from + i).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
                (value >> 29) as u8
            })
            .collect()
    }

    async fn channel(
        world: &World,
        machine: &Machine,
        incoming: &mut IncomingTunnels,
    ) -> (Tunnel, Tunnel) {
        let terminal = new_id();
        let ticket = world
            .api
            .link
            .tickets()
            .terminal(
                machine.id,
                Service::Pty {
                    terminal_id: terminal,
                },
            )
            .unwrap();
        let terminals = world.api.link.terminals().clone();
        let (opened, accepted) = tokio::join!(terminals.open(machine.id, &ticket), async {
            incoming.next().await.unwrap().accept()
        });
        (opened.unwrap(), accepted.unwrap())
    }

    async fn write(mut output: tokio::io::WriteHalf<Tunnel>, seed: u64, total: u64) {
        let mut offset = 0;
        while offset < total {
            let len = usize::try_from((total - offset).min(16 * 1024)).unwrap();
            output.write_all(&pattern(seed, offset, len)).await.unwrap();
            offset += len as u64;
        }
        output.shutdown().await.unwrap();
    }

    async fn read(
        mut input: tokio::io::ReadHalf<Tunnel>,
        seed: u64,
        total: u64,
        progress: Arc<AtomicU64>,
        hold: Option<(Arc<Notify>, Arc<Notify>)>,
    ) {
        let mut offset = 0;
        let mut buffer = vec![0u8; 32 * 1024];
        let mut hold = hold;
        loop {
            let len = input.read(&mut buffer).await.unwrap();
            if len == 0 {
                break;
            }
            assert_eq!(&buffer[..len], &pattern(seed, offset, len));
            offset += len as u64;
            progress.store(offset, Ordering::SeqCst);
            if let Some((started, release)) = hold.take() {
                started.notify_one();
                release.notified().await;
            }
        }
        assert_eq!(offset, total, "no ordered bytes were lost or duplicated");
    }

    #[tokio::test]
    async fn exact_ack_and_queue_observation_on_real_tls_wss() {
        const TOTAL: u64 = 100 * 1024 * 1024; // Existing link release transfer criterion.
        let command = new_id();
        // Bound this test recorder by the existing channel-count ceiling. Overflow is a
        // failed observation, never evidence that the product queue stayed bounded.
        let recording =
            Recording::install(&[command], Limits::default().max_channels as usize).unwrap();
        let mut world = World::start().await;
        let enrolled = world.enroll("observed-environment").await;
        let machine = world.start_machine_as(
            &enrolled,
            enrolled.endpoints(Trust::Pinned),
            vec![Role::Environment],
        );
        tokio::time::timeout(PATIENCE, machine.link.connected())
            .await
            .unwrap();
        machine.handler.hold(CommandKind::MachineProbe);
        machine.handler.on(CommandKind::MachineProbe, |_| Ok(CommandResult::MachineProbe(
            serde_json::from_value(serde_json::json!({"hostname":"owned-fixture","os_version":"fixture",
                "cpus":1,"memory_bytes":1048576,"harnesses":[],"adapters":[],"plugins":[],
                "policy":null,"rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0},"clock_offset_ms":0})).unwrap()
        )));
        let mut incoming = machine.link.incoming_tunnels();
        let (bulk_api, bulk_machine) = channel(&world, &machine, &mut incoming).await;
        let (mut interactive_api, mut interactive_machine) =
            channel(&world, &machine, &mut incoming).await;
        let (api_read, api_write) = tokio::io::split(bulk_api);
        let (machine_read, machine_write) = tokio::io::split(bulk_machine);
        let progress = Arc::new(AtomicU64::new(0));
        let stalled = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let api_writer = tokio::spawn(write(api_write, 11, TOTAL));
        let transfers = [
            tokio::spawn(write(machine_write, 23, TOTAL)),
            tokio::spawn(read(api_read, 23, TOTAL, Arc::new(AtomicU64::new(0)), None)),
            tokio::spawn(read(
                machine_read,
                11,
                TOTAL,
                Arc::clone(&progress),
                Some((Arc::clone(&stalled), Arc::clone(&release))),
            )),
        ];
        tokio::time::timeout(PATIENCE, stalled.notified())
            .await
            .unwrap();
        world
            .api
            .link
            .dispatcher()
            .issue_as(
                command,
                machine.id,
                &SerialKey::Machine,
                Time::new(chrono::Utc::now() + chrono::Duration::seconds(30)),
                &MachineProbe {},
            )
            .await
            .unwrap();
        eventually(
            "the actual TLS ACK to arrive at API and gateway",
            || async {
                recording
                    .recorder
                    .snapshot()
                    .commands
                    .iter()
                    .any(|row| row.id == command && row.seen[2] > 0 && row.seen[3] > 0)
            },
        )
        .await;
        tokio::time::timeout(PATIENCE, machine.handler.started(command))
            .await
            .unwrap();
        assert!(
            !api_writer.is_finished(),
            "the stopped reader applied backpressure"
        );
        assert_eq!(machine.handler.runs_of(command), 1);
        let ack = recording
            .recorder
            .snapshot()
            .commands
            .into_iter()
            .find(|row| row.id == command)
            .unwrap();
        assert_eq!(
            ack.seen,
            [1, 1, 1, 1],
            "handoff/ACK observations stay independent from the held result"
        );
        assert!(ack.carrier_ack_micros.is_some());
        assert!(ack.api_ack_micros.is_some());

        // This is a byte echo fixture over the actual PTY carrier, not a claim of a
        // native interactive shell. No remote-echo latency threshold is invented.
        interactive_api.write_all(b"owned echo\n").await.unwrap();
        let mut typed = [0u8; 11];
        tokio::time::timeout(PATIENCE, interactive_machine.read_exact(&mut typed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&typed, b"owned echo\n");
        interactive_machine.write_all(&typed).await.unwrap();
        let mut echoed = [0u8; 11];
        tokio::time::timeout(PATIENCE, interactive_api.read_exact(&mut echoed))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(echoed, typed);
        machine.handler.release();
        tokio::time::timeout(PATIENCE, world.api.link.dispatcher().wait(command))
            .await
            .unwrap()
            .unwrap();
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(120), async {
            api_writer.await.unwrap();
            for transfer in transfers {
                transfer.await.unwrap();
            }
        })
        .await
        .expect("loopback100MiB fixture completes; this is not the shaped-run hang budget");
        assert_eq!(progress.load(Ordering::SeqCst), TOTAL);
        let channels = 2_u64;
        let per_channel = u64::from(Limits::default().tunnel_window_bytes) + REPLAY_BUFFER_BYTES;
        assert!(
            machine.observed_buffers.peak() <= channels * 2 * per_channel,
            "observed channel storage respects the existing receive/replay budgets"
        );
        drop(interactive_api);
        drop(interactive_machine);
        eventually("measured carrier/backplane queues to drain", || async {
            recording.recorder.snapshot().queues.iter().all(|queue| {
                queue.queued_messages == 0
                    && queue.in_flight_messages == 0
                    && queue.in_flight_payload_bytes == 0
                    && queue.in_flight_unknown_sizes == 0
                    && !queue.receiver_closed
                    && !queue.inconsistent
                    && !queue.indeterminate_close_race
                    && queue.discarded_on_receiver_close == 0
                    && queue.selected_without_completion == 0
                    && queue.failed_enqueues == 0
                    && queue.unknown_sizes_seen == 0
            })
        })
        .await;
        let before_stop = recording.recorder.snapshot();
        assert!(!before_stop.overflowed);
        for label in [
            "api.terminal_out",
            "controller.down",
            "controller.notices",
            "machine.inputs",
        ] {
            let queue = before_stop
                .queues
                .iter()
                .find(|queue| queue.label == label)
                .expect("the actual mixed path must construct each observed queue family");
            assert!(
                queue.selected > 0,
                "{label} must carry actual fixture traffic"
            );
            assert_eq!(
                queue.selected,
                queue.sent + queue.processed,
                "{label}: every selected item completed or was handled locally"
            );
        }
        assert!(
            before_stop
                .queues
                .iter()
                .any(|queue| queue.label == "api.backplane" && queue.selected > 0)
        );
        assert!(
            before_stop
                .queues
                .iter()
                .any(|queue| queue.label == "gateway.backplane" && queue.selected > 0)
        );
        assert!(
            before_stop
                .queues
                .iter()
                .any(|queue| queue.label == "link.data" && queue.peak_payload_bytes > 0)
        );
        assert!(before_stop.queues.iter().all(|queue| !queue.inconsistent
            && !queue.indeterminate_close_race
            && queue.discarded_on_receiver_close == 0
            && queue.selected_without_completion == 0
            && queue.failed_enqueues == 0
            && !queue.receiver_closed));
        machine.link.shutdown().await;
        world.stop().await;
        if let Some(path) = std::env::var_os("ORCH_TEST_WSS_OBSERVATIONS") {
            std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
                "kernel_profile":"unshaped_loopback", "recommended_ack_requirement_ms":1000,
                "floor_latency_requirement":null, "before_shutdown":before_stop,
                "after_shutdown":recording.recorder.snapshot(), "complete_wss_release_gate":false,
                "scope":"instrumented realTLS fixture only; no kernel loss/native shell/browser qualification"
            })).unwrap()).unwrap();
        }
    }
}
