//! A person's shell on the environment role: opened by a command, attached through a tunnel that ends at the API, typed into, left and taken up again with its last output, and ended.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod rig_environment;

use std::time::Duration;

use orch_link::tunnels::channel::{Tunnel, TunnelEvent};
use orch_link_contract::commands::{PtyClose, PtyOpen};
use orch_link_contract::tunnels::{OpenErrorCode, Service, TicketSource};
use orch_types::{Id, new_id};
use rig_environment::{EnvRig, PATIENCE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Reads from the tunnel until `wanted` has been seen in what came, or the patience ends.
async fn read_until(tunnel: &mut Tunnel, wanted: &str) -> String {
    let mut seen = String::new();
    let mut buffer = vec![0u8; 4096];
    let outcome = tokio::time::timeout(PATIENCE, async {
        while !seen.contains(wanted) {
            let count = tunnel.read(&mut buffer).await.unwrap();
            assert!(count > 0, "the tunnel ended; the screen showed {seen:?}");
            seen.push_str(&String::from_utf8_lossy(&buffer[..count]));
        }
    })
    .await;
    assert!(
        outcome.is_ok(),
        "{wanted:?} never came; the screen showed {seen:?}"
    );
    seen
}

async fn open_shell(rig: &EnvRig) -> (Id, String) {
    let terminal_id = new_id();
    let opened = rig
        .succeed(&PtyOpen {
            environment_id: rig.environment,
            terminal_id,
            cols: 100,
            rows: 30,
            cwd: String::new(),
            record: false,
        })
        .await;
    assert_eq!(opened["service"], format!("pty/{terminal_id}"));
    (terminal_id, opened["shell"].as_str().unwrap().to_owned())
}

async fn attach(rig: &EnvRig, terminal_id: Id) -> Tunnel {
    let ticket = rig.ticket(TicketSource::Api, Service::Pty { terminal_id }, None, None);
    tokio::time::timeout(PATIENCE, rig.hub.terminals().open(rig.machine_id, &ticket))
        .await
        .expect("the shell never answered the tunnel")
        .expect("the tunnel to the shell was refused")
}

#[tokio::test]
async fn a_shell_is_typed_into_through_a_tunnel_left_and_taken_up_again_with_its_last_output() {
    let mut rig = EnvRig::new();
    rig.config.environment.terminal = true;
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let (terminal_id, shell) = open_shell(&rig).await;
    assert!(!shell.is_empty());

    // Typed into through a tunnel, it answers.
    let mut first = attach(&rig, terminal_id).await;
    let calculate = if cfg!(windows) {
        b"set /A 6*7\r\n".as_slice()
    } else {
        b"echo $((6*7))\n".as_slice()
    };
    first.write_all(calculate).await.unwrap();
    let screen = read_until(&mut first, "42").await;
    assert!(screen.contains("42"));

    // Left: the tunnel goes, the shell does not. Something it prints meanwhile is kept.
    drop(first);
    let away = rig.home.path().join("while-away");
    let mut second = {
        // The command runs while nobody is attached: a file appears, and the shell says so later.
        let mut again = attach(&rig, terminal_id).await;
        let command = if cfg!(windows) {
            format!(
                "type nul > \"{}\" & cmd /V:ON /C \"set /A n=5+4 & echo kept-!n!\"\r\n",
                away.display()
            )
        } else {
            format!(
                "touch '{}' && echo kept-$((5+4))\n",
                away.display().to_string().replace('\'', "'\\''")
            )
        };
        again.write_all(command.as_bytes()).await.unwrap();
        read_until(&mut again, "kept-9").await;
        drop(again);
        tokio::time::sleep(Duration::from_millis(200)).await;
        attach(&rig, terminal_id).await
    };
    assert!(away.exists(), "the shell went on while no tunnel was open");
    // Taken up again, the screen is found as it was: the last output comes first.
    let found = read_until(&mut second, "kept-9").await;
    assert!(
        found.contains("42"),
        "the earlier output is part of what was kept: {found:?}"
    );

    // The shell ends: the tunnel is told with the code, and closes.
    let mut events = second.take_events().unwrap();
    second
        .write_all(if cfg!(windows) {
            b"exit 3\r\n"
        } else {
            b"exit 3\n"
        })
        .await
        .unwrap();
    let told = tokio::time::timeout(PATIENCE, async {
        loop {
            match events.recv().await {
                Some(TunnelEvent::Exit { code }) => return code,
                Some(TunnelEvent::Resize { .. }) => {}
                None => panic!("the tunnel ended without telling how the shell ended"),
            }
        }
    })
    .await
    .expect("the shell's end was never told");
    assert_eq!(told, 3);
    rig.stop().await;
}

#[tokio::test]
async fn the_size_of_the_screen_follows_the_tunnel_and_a_second_screen_takes_the_shell_from_the_first()
 {
    let mut rig = EnvRig::new();
    rig.config.environment.terminal = true;
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let (terminal_id, _) = open_shell(&rig).await;

    let mut first = attach(&rig, terminal_id).await;
    first.resize(132, 43);
    let query_size = if cfg!(windows) {
        b"powershell.exe -NoProfile -Command \"$s=$Host.UI.RawUI.WindowSize; Write-Output ($s.Height.ToString()+' '+$s.Width.ToString())\"\r\n".as_slice()
    } else {
        b"stty size\n".as_slice()
    };
    first.write_all(query_size).await.unwrap();
    read_until(&mut first, "43 132").await;

    // A second tunnel to the shell ends the first, which is told why.
    let mut second = attach(&rig, terminal_id).await;
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(PATIENCE, first.read_to_end(&mut rest)).await;
    assert!(ended.is_ok(), "the first screen kept the shell");
    let calculate = if cfg!(windows) {
        b"cmd /V:ON /C \"set /A n=1+1 & echo second-!n!\"\r\n".as_slice()
    } else {
        b"echo second-$((1+1))\n".as_slice()
    };
    second.write_all(calculate).await.unwrap();
    read_until(&mut second, "second-2").await;

    // Closing the shell says how it ended.
    let closed = rig.succeed(&PtyClose { terminal_id }).await;
    assert!(closed["bytes_out"].as_i64().is_some_and(|bytes| bytes > 0));
    rig.stop().await;
}

#[tokio::test]
async fn a_shell_that_policy_forbids_is_refused_whoever_asks_and_so_is_a_tunnel_to_any() {
    let mut rig = EnvRig::new();
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    // Switched off, as a machine is installed: the command is refused with the policy's word.
    let (code, detail) = rig
        .failure(&PtyOpen {
            environment_id: rig.environment,
            terminal_id: new_id(),
            cols: 80,
            rows: 24,
            cwd: String::new(),
            record: false,
        })
        .await;
    assert_eq!(code, "unsupported");
    assert!(detail.contains("environment.terminal"), "{detail}");

    // And a tunnel to a shell, whichever it names, is refused at its opening while shells are off.
    let ticket = rig.ticket(
        TicketSource::Api,
        Service::Pty {
            terminal_id: new_id(),
        },
        None,
        None,
    );
    let refused = rig.hub.terminals().open(rig.machine_id, &ticket).await;
    match refused {
        Err(orch_link::tunnels::channel::OpenError::Refused { code, detail }) => {
            assert_eq!(code, OpenErrorCode::Refused);
            assert!(detail.contains("switched off"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    rig.stop().await;
}

#[tokio::test]
async fn a_tunnel_to_a_shell_that_is_not_there_is_refused_at_its_opening() {
    let mut rig = EnvRig::new();
    rig.config.environment.terminal = true;
    let workspace = rig.directory("work");
    rig.start().await;
    rig.attach(&workspace).await;
    let ticket = rig.ticket(
        TicketSource::Api,
        Service::Pty {
            terminal_id: new_id(),
        },
        None,
        None,
    );
    match rig.hub.terminals().open(rig.machine_id, &ticket).await {
        Err(orch_link::tunnels::channel::OpenError::Refused { code, .. }) => {
            assert_eq!(code, OpenErrorCode::NoSuchService);
        }
        other => panic!("{other:?}"),
    }
    rig.stop().await;
}
