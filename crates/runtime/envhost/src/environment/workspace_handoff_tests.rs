//! Swaps a shell's directory after it was opened but before the PTY starts.

use super::*;
use crate::blobs::MemoryBlobs;
use crate::terminal::Event;
use orch_link_contract::commands::IndexMode;
use orch_types::{SystemClock, new_id};
use tokio::sync::oneshot;

struct Boundary {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

tokio::task_local! { static BEFORE_SHELL_SPAWN: Mutex<Option<Boundary>>; }

pub(super) async fn before_shell_spawn() {
    let boundary = BEFORE_SHELL_SPAWN
        .try_with(|slot| lock(slot).take())
        .ok()
        .flatten();
    if let Some(boundary) = boundary {
        let _ = boundary.entered.send(());
        let _ = boundary.resume.await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_enters_the_open_directory_after_its_name_is_swapped() {
    let owner = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let workspace = owner.path().join("workspace");
    let directory = workspace.join("sub");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("marker"), "offered").unwrap();
    std::fs::write(outside.path().join("marker"), "outside").unwrap();
    let mut settings =
        HostSettings::connected(owner.path().join("data"), vec![owner.path().to_owned()]);
    settings.gates = Gates::OPEN;
    settings.shell = Some("/bin/sh".into());
    let (reports, _receiver) = Reports::channel(32);
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let environment_id = new_id();
    let mut variables = BTreeMap::new();
    variables.insert("HOME".into(), owner.path().to_string_lossy().into_owned());
    host.env_attach(
        &EnvAttach {
            environment_id,
            workspace_root: Some(workspace.to_string_lossy().into_owned()),
            data_sources: None,
            resources: None,
            data_generation: None,
            storage: None,
            ignore: Vec::new(),
            index: IndexMode::Live,
            mounts: None,
            variables,
            secrets: BTreeMap::new(),
        },
        &BTreeMap::new(),
    )
    .await
    .unwrap();

    let terminal_id = new_id();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (resume_tx, resume_rx) = oneshot::channel();
    let opener = host.clone();
    let opening = tokio::spawn(async move {
        BEFORE_SHELL_SPAWN
            .scope(
                Mutex::new(Some(Boundary {
                    entered: entered_tx,
                    resume: resume_rx,
                })),
                async move {
                    opener
                        .pty_open(&PtyOpen {
                            environment_id,
                            terminal_id,
                            cols: 80,
                            rows: 24,
                            cwd: "sub".into(),
                            record: false,
                        })
                        .await
                },
            )
            .await
    });
    // This boundary is after the requested subdirectory is opened and before spawn_command.
    tokio::time::timeout(Duration::from_secs(30), entered_rx)
        .await
        .unwrap()
        .unwrap();
    std::fs::rename(&directory, workspace.join("moved")).unwrap();
    std::os::unix::fs::symlink(outside.path(), &directory).unwrap();
    resume_tx.send(()).unwrap();
    opening.await.unwrap().unwrap();

    let mut screen = host.pty_attach(terminal_id, true).unwrap();
    screen
        .send_input(b"printf 'location=%s:done\\n' \"$(cat marker)\"\n".to_vec())
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(300), async {
        let mut text = String::new();
        loop {
            match screen.next().await {
                Some(Event::Data(bytes)) => text.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("the shell ended before answering: {other:?}; {text}"),
            }
            if text.contains("location=offered:done") || text.contains("location=outside:done") {
                break text;
            }
        }
    })
    .await
    .unwrap();
    drop(screen);
    host.pty_close(&PtyClose { terminal_id }).await.unwrap();
    assert!(answer.contains("location=offered:done"), "{answer}");
    assert!(!answer.contains("location=outside:done"), "{answer}");
}
