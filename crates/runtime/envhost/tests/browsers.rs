//! Real shipped-browser profiles, shared protocol input, pictures, persistence and confinement.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use orch_envhost::blobs::MemoryBlobs;
use orch_envhost::browser_screen::{BrowserInput, BrowserStream};
use orch_envhost::environment::{EnvHost, HostSettings, ManagedPlaces, Report, Reports};
use orch_envhost::processes::ScopeKind;
use orch_link_contract::browser::*;
use orch_link_contract::commands::{EnvAttach, EnvDetach, FsList, IndexMode};
use orch_link_contract::tunnels::ExecOpcode;
use orch_realtime_contract::browser::{
    BrowserClientFrame, BrowserServerFrame, KeyKind, PictureHeader, PointerButton, PointerKind,
};
use orch_types::{Id, Json, SystemClock};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod browser_netem;

/// A real browser has twenty seconds to start; the shared machine gets ample test time around it.
const WAIT: Duration = Duration::from_secs(60);
/// Local fixture servers live in the reserved test range, inside an isolated unit.
const PAGE_PORT: u16 = 41820;
/// The fixture has no external resources or credentials.
const PAGE: &str = r#"<!doctype html><meta charset=utf-8><title>browser-test</title><input id=i><button id=b onclick="document.title='clicked';document.getElementById('count').textContent='clicked'">Go</button><div id=count></div><input id=f type=file><script>document.body.style.background='white'</script>"#;

async fn inside(name: &str) -> bool {
    if std::env::var_os("ORCH_BROWSER_IN_UNIT").is_some() {
        return true;
    }
    let docker = tokio::process::Command::new("docker")
        .args(["image", "inspect", "orch-env-base:image-final"])
        .output()
        .await;
    if !docker.is_ok_and(|o| o.status.success()) {
        eprintln!("skipped: the shipped image is not available");
        return false;
    }
    let executable = if let Some(binary) = std::env::var_os("ORCH_BROWSER_TEST_PROGRAM") {
        std::path::PathBuf::from(binary)
    } else {
        let current = std::env::current_exe().unwrap();
        if current
            .to_string_lossy()
            .contains("x86_64-unknown-linux-musl")
        {
            current
        } else {
            eprintln!("skipped: build the static browser tests and set ORCH_BROWSER_TEST_PROGRAM");
            return false;
        }
    };
    let container = format!("orch-browser-test-{}", orch_types::new_id().simple());
    let image = if name
        == "an_environment_without_browser_programs_still_lists_its_files_and_reports_unsupported"
    {
        "orch-builder:bookworm-slim-rust1.97.0"
    } else {
        "orch-env-base:image-final"
    };
    let output = tokio::time::timeout(
        Duration::from_secs(600),
        tokio::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "--init",
                "--ulimit",
                "core=0:-1",
                "--network",
                "none",
                "--user",
                "1000:1000",
                "--name",
                &container,
                "--env",
                "ORCH_BROWSER_IN_UNIT=1",
                "--mount",
            ])
            .arg(format!(
                "type=bind,src={},dst=/tmp/browser-tests,readonly",
                executable.display()
            ))
            .args([image, "/tmp/browser-tests", "--exact", name, "--nocapture"])
            .output(),
    )
    .await;
    match output {
        Ok(Ok(output)) => {
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("running 1 test\n"),
                "the static fixture lacks {name}; rebuild it before running this scenario"
            );
            assert!(output.status.success(), "the test in the real unit failed");
        }
        _ => {
            let _ = tokio::process::Command::new("docker")
                .args(["rm", "-f", &container])
                .output()
                .await;
            panic!("the real unit did not finish");
        }
    }
    false
}

struct World {
    dir: tempfile::TempDir,
    host: EnvHost,
    id: Id,
    browser_states: Arc<tokio::sync::Mutex<Vec<BrowserState>>>,
}

impl World {
    async fn new() -> Self {
        Self::with_browser(true).await
    }

    async fn with_browser(supported: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(root.join("file.txt"), "workspace file").unwrap();
        let settings = HostSettings::managed(
            dir.path().join("state"),
            ManagedPlaces {
                workspace_root: root,
                home: Some(home),
                mounts_file: None,
                tools_dir: None,
                mounts_enforced: false,
            },
            ScopeKind::All,
        );
        let (reports, mut receiver) = Reports::channel(128);
        let browser_states = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let observed = Arc::clone(&browser_states);
        tokio::spawn(async move {
            while let Some(item) = receiver.recv().await {
                if let Report::BrowserState(state) = item.report {
                    observed.lock().await.push(state);
                }
            }
        });
        let host = EnvHost::new(
            settings,
            Arc::new(SystemClock),
            reports,
            Arc::new(MemoryBlobs::new()),
            Vec::new(),
        );
        let id = orch_types::new_id();
        let facts = host
            .env_attach(
                &EnvAttach {
                    resources: None,
                    data_generation: None,
                    storage: None,
                    data_sources: None,
                    environment_id: id,
                    workspace_root: None,
                    ignore: Vec::new(),
                    index: IndexMode::Full,
                    mounts: None,
                    variables: BTreeMap::new(),
                    secrets: BTreeMap::new(),
                },
                &BTreeMap::new(),
            )
            .await
            .unwrap();
        if supported {
            assert!(!facts.browser.programs.is_empty());
            assert_eq!(facts.browser.playwright_cli.as_deref(), Some("0.1.22"));
            assert_eq!(facts.browser.sandbox, Some(false));
        } else {
            assert!(facts.browser.programs.is_empty());
            assert!(facts.vnc.servers.is_empty());
            assert_eq!(facts.browser.sandbox, None);
        }
        assert!(facts.looked_at.is_some());
        Self {
            dir,
            host,
            id,
            browser_states,
        }
    }
    fn request(&self, profile: &str) -> BrowserOpen {
        BrowserOpen {
            environment_id: self.id,
            browser_id: orch_types::new_id(),
            profile: profile.to_owned(),
            create: true,
            preset: BrowserPreset::Headless,
            args: Vec::new(),
            viewport: "1280x800".to_owned(),
            url: None,
            display: None,
        }
    }
    async fn stop(&self) {
        self.host
            .env_detach(&EnvDetach {
                environment_id: self.id,
            })
            .await
            .unwrap();
    }
}

async fn server() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", PAGE_PORT))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut bytes = [0; 8192];
                let _ = socket.read(&mut bytes).await;
                let slow = bytes.starts_with(b"GET /slow ");
                let hold = bytes.starts_with(b"GET /hold ");
                if hold {
                    // The document commits immediately; a blocking script leaves loading visible during later title events.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                let body = if hold {
                    "/* the delayed script has arrived */".to_owned()
                } else if slow {
                    format!(
                        "{}<script src='/hold'></script>",
                        PAGE.replace("browser-test", "loaded-slow")
                    )
                } else {
                    PAGE.to_owned()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nSet-Cookie: signed_in=kept; Max-Age=3600; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
}

async fn page(browser: &Arc<orch_envhost::browsers::RunningBrowser>) -> (String, String) {
    let targets = browser
        .cdp
        .call("Target.getTargets", json!({}), None)
        .await
        .unwrap();
    let tab = targets["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["type"] == "page")
        .unwrap()["targetId"]
        .as_str()
        .unwrap()
        .to_owned();
    let session = browser
        .cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId":tab,"flatten":true}),
            None,
        )
        .await
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();
    (tab, session)
}

async fn value(
    browser: &Arc<orch_envhost::browsers::RunningBrowser>,
    session: &str,
    expression: &str,
) -> Json {
    browser
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true}),
            Some(session),
        )
        .await
        .unwrap()["result"]["value"]
        .clone()
}

async fn wait_value(
    browser: &Arc<orch_envhost::browsers::RunningBrowser>,
    session: &str,
    expression: &str,
    expected: Json,
) {
    eprintln!("observing fixture state: {expression}");
    tokio::time::timeout(WAIT, async {
        loop {
            if value(browser, session, expression).await == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn complete_thread_profile_names_keep_real_browser_cookies_apart() {
    if !inside("complete_thread_profile_names_keep_real_browser_cookies_apart").await {
        return;
    }
    let world = World::new().await;
    let a: Id = "019908ab-1111-7000-8000-000000000001".parse().unwrap();
    let b: Id = "019908ab-1111-7000-8000-000000000002".parse().unwrap();
    let first = world.request(&format!("thread-{}", a.simple()));
    let second = world.request(&format!("thread-{}", b.simple()));
    let opened = world.host.browser_open(&first).await.unwrap();
    let separate = world.host.browser_open(&second).await.unwrap();
    assert!(opened.started && separate.started);
    assert_ne!(opened.endpoint, separate.endpoint);
    let environment = world.host.environment(world.id).unwrap();
    let first_browser = environment.browsers().get(first.browser_id).await.unwrap();
    let second_browser = environment.browsers().get(second.browser_id).await.unwrap();
    first_browser
        .cdp
        .call(
            "Storage.setCookies",
            json!({"cookies":[{"name":"own_sign_in","value":"first","url":"http://localhost/"}]}),
            None,
        )
        .await
        .unwrap();
    let first_cookies = first_browser
        .cdp
        .call("Storage.getCookies", json!({}), None)
        .await
        .unwrap();
    let second_cookies = second_browser
        .cdp
        .call("Storage.getCookies", json!({}), None)
        .await
        .unwrap();
    assert!(
        first_cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cookie| cookie["name"] == "own_sign_in")
    );
    assert!(
        !second_cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cookie| cookie["name"] == "own_sign_in")
    );
    let joined = world
        .host
        .browser_open(&world.request(&first.profile))
        .await
        .unwrap();
    assert!(!joined.started);
    assert_eq!(joined.running, Some(first.browser_id));
    world.stop().await;
}

#[tokio::test]
async fn two_callers_join_one_kept_profile_and_a_cookie_survives_quit_and_environment_reattach() {
    if !inside(
        "two_callers_join_one_kept_profile_and_a_cookie_survives_quit_and_environment_reattach",
    )
    .await
    {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("team");
    let other = world.request("team");
    let started = tokio::time::Instant::now();
    let (a, b) = tokio::join!(
        world.host.browser_open(&request),
        world.host.browser_open(&other)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.endpoint, b.endpoint);
    assert_ne!(a.started, b.started);
    let id = if a.started {
        request.browser_id
    } else {
        other.browser_id
    };
    eprintln!(
        "browser startup: {:.3} s; sandbox: false; one process for two concurrent calls",
        started.elapsed().as_secs_f64()
    );
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment.browsers().get(id).await.unwrap();
    let (_, session) = page(&browser).await;
    browser
        .cdp
        .call(
            "Page.navigate",
            json!({"url":format!("http://127.0.0.1:{PAGE_PORT}/")}),
            Some(&session),
        )
        .await
        .unwrap();
    wait_value(
        &browser,
        &session,
        "document.cookie",
        json!("signed_in=kept"),
    )
    .await;
    let listed = world
        .host
        .fs_list(&FsList {
            environment_id: world.id,
            mount: None,
            path: String::new(),
            watch_id: None,
            watch_instance: None,
            page: None,
        })
        .await
        .unwrap();
    assert!(!serde_json::to_string(&listed).unwrap().contains("browsers"));
    let store = MemoryBlobs::new();
    let snapshot = world
        .host
        .fs_snapshot(
            &orch_link_contract::commands::FsSnapshot {
                work: None,
                environment_id: world.id,
                job_id: orch_types::new_id(),
                selection: Default::default(),
            },
            &store,
            &orch_envhost::snapshot::NoProgress,
        )
        .await
        .unwrap();
    assert_eq!(snapshot.file_count, 1);
    let manifest = store.get_bytes(&snapshot.manifest_blob).unwrap();
    assert!(!String::from_utf8_lossy(&manifest).contains("browsers"));
    assert!(!String::from_utf8_lossy(&manifest).contains("signed_in"));
    world
        .host
        .fs_materialize(
            &orch_link_contract::commands::FsMaterialize {
                work: None,
                environment_id: world.id,
                job_id: None,
                manifest_blob: snapshot.manifest_blob,
                target_root: "fork".to_owned(),
                purpose: orch_link_contract::commands::MaterializePurpose::Move,
            },
            &store,
            &orch_envhost::snapshot::NoProgress,
        )
        .await
        .unwrap();
    let fork = world.dir.path().join("workspace/fork");
    assert_eq!(
        std::fs::read_to_string(fork.join("file.txt")).unwrap(),
        "workspace file"
    );
    assert_eq!(std::fs::read_dir(fork).unwrap().count(), 1);
    let refused = world
        .host
        .browser_profile(&BrowserProfileCommand {
            environment_id: world.id,
            action: BrowserProfileAction::Delete,
            name: "team".to_owned(),
            to: None,
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "in_use");
    world
        .host
        .browser_close(&BrowserClose { browser_id: id })
        .await
        .unwrap();
    let reopened = world.request("team");
    world.host.browser_open(&reopened).await.unwrap();
    let browser = environment
        .browsers()
        .get(reopened.browser_id)
        .await
        .unwrap();
    let (_, session) = page(&browser).await;
    let cookies = browser
        .cdp
        .call("Storage.getCookies", json!({}), None)
        .await
        .unwrap();
    assert!(
        cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "signed_in" && c["value"] == "kept")
    );
    let _ = session;
    world.stop().await;
    let attach = EnvAttach {
        resources: None,
        data_generation: None,
        storage: None,
        data_sources: None,
        environment_id: world.id,
        workspace_root: None,
        ignore: Vec::new(),
        index: IndexMode::Full,
        mounts: None,
        variables: BTreeMap::new(),
        secrets: BTreeMap::new(),
    };
    world
        .host
        .env_attach(&attach, &BTreeMap::new())
        .await
        .unwrap();
    let request = world.request("team");
    world.host.browser_open(&request).await.unwrap();
    let browser = world
        .host
        .environment(world.id)
        .unwrap()
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let cookies = browser
        .cdp
        .call("Storage.getCookies", json!({}), None)
        .await
        .unwrap();
    assert!(
        cookies["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "signed_in")
    );
    world
        .host
        .browser_close(&BrowserClose {
            browser_id: request.browser_id,
        })
        .await
        .unwrap();
    world
        .host
        .browser_profile(&BrowserProfileCommand {
            environment_id: world.id,
            action: BrowserProfileAction::Rename,
            name: "team".to_owned(),
            to: Some("renamed".to_owned()),
        })
        .await
        .unwrap();
    world
        .host
        .browser_profile(&BrowserProfileCommand {
            environment_id: world.id,
            action: BrowserProfileAction::Delete,
            name: "renamed".to_owned(),
            to: None,
        })
        .await
        .unwrap();
    world.stop().await;
    assert!(world.dir.path().join("home/.orch/browsers").is_dir());
}

#[tokio::test]
async fn a_person_and_playwright_drive_the_same_tab_while_its_picture_is_watched() {
    if !inside("a_person_and_playwright_drive_the_same_tab_while_its_picture_is_watched").await {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("shared");
    let opened = world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    browser
        .cdp
        .call(
            "Page.navigate",
            json!({"url":format!("http://127.0.0.1:{PAGE_PORT}/")}),
            Some(&session),
        )
        .await
        .unwrap();
    wait_value(&browser, &session, "document.title", json!("browser-test")).await;
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    stream
        .input
        .send(BrowserInput {
            frame: BrowserClientFrame::Watch {
                tab: tab.clone(),
                width: 640,
                height: 400,
                dpr: 1.0,
            },
            control_watch: true,
        })
        .await
        .unwrap();
    let first = tokio::time::timeout(WAIT, async {
        loop {
            let output = stream.output.recv().await.unwrap();
            let bytes = output.wire().unwrap().len();
            stream.handed(&output, bytes as u64).await;
            if output.message.opcode == ExecOpcode::Binary {
                break output.message.payload;
            }
        }
    })
    .await
    .unwrap();
    let header = PictureHeader::decode(&first).unwrap();
    assert!(header.image.width <= 640 && header.image.height <= 400);
    assert!(header.viewport.width >= header.image.width);
    let input = stream.input.clone();
    let (sharp, mut sharp_rx) = tokio::sync::watch::channel(false);
    let drain = tokio::spawn(async move {
        while let Some(output) = stream.output.recv().await {
            let bytes = output.wire().unwrap().len();
            stream.handed(&output, bytes as u64).await;
            if output.message.opcode == ExecOpcode::Text {
                let frame: BrowserServerFrame =
                    serde_json::from_slice(&output.message.payload).unwrap();
                if matches!(frame, BrowserServerFrame::Quality { level: 0, .. }) {
                    sharp.send_replace(true);
                }
                if let BrowserServerFrame::Error { code, detail } = frame {
                    panic!("browser input failed: {code:?}: {detail}");
                }
            }
        }
    });
    let attach = tokio::process::Command::new("playwright-cli")
        .arg("-s=shared")
        .arg("attach")
        .arg(format!("--cdp={}", opened.endpoint))
        .output()
        .await
        .unwrap();
    assert!(
        attach.status.success(),
        "{}",
        String::from_utf8_lossy(&attach.stderr)
    );
    browser
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression":"document.getElementById('i').focus()"}),
            Some(&session),
        )
        .await
        .unwrap();
    input
        .send(BrowserInput {
            frame: BrowserClientFrame::Text {
                tab: tab.clone(),
                text: "person 中文".to_owned(),
            },
            control_watch: false,
        })
        .await
        .unwrap();
    wait_value(
        &browser,
        &session,
        "document.getElementById('i').value",
        json!("person 中文"),
    )
    .await;
    let script = tokio::process::Command::new("playwright-cli")
        .args([
            "-s=shared",
            "eval",
            "document.getElementById('i').value='agent'",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        script.status.success(),
        "{}",
        String::from_utf8_lossy(&script.stderr)
    );
    wait_value(
        &browser,
        &session,
        "document.getElementById('i').value",
        json!("agent"),
    )
    .await;
    browser
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression":"document.getElementById('i').blur()"}),
            Some(&session),
        )
        .await
        .unwrap();
    let position=value(&browser,&session,"JSON.stringify((()=>{let r=document.getElementById('b').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})())").await;
    let position: Json = serde_json::from_str(position.as_str().unwrap()).unwrap();
    let clicked = tokio::time::Instant::now();
    for kind in [PointerKind::Down, PointerKind::Up] {
        input
            .send(BrowserInput {
                frame: BrowserClientFrame::Pointer {
                    tab: tab.clone(),
                    kind,
                    x: position["x"].as_f64().unwrap(),
                    y: position["y"].as_f64().unwrap(),
                    button: Some(PointerButton::Left),
                    buttons: None,
                    clicks: Some(1),
                    dx: None,
                    dy: None,
                    modifiers: None,
                },
                control_watch: false,
            })
            .await
            .unwrap();
    }
    wait_value(&browser, &session, "document.title", json!("clicked")).await;
    eprintln!(
        "click to visible tab title: {:.3} s",
        clicked.elapsed().as_secs_f64()
    );
    input
        .send(BrowserInput {
            frame: BrowserClientFrame::Key {
                tab: tab.clone(),
                kind: KeyKind::Down,
                key: "Tab".to_owned(),
                code: "Tab".to_owned(),
                text: None,
                modifiers: None,
                repeat: None,
                location: None,
            },
            control_watch: false,
        })
        .await
        .unwrap();
    let clear = tokio::time::timeout(WAIT, async {
        while !*sharp_rx.borrow_and_update() {
            sharp_rx.changed().await.unwrap();
        }
    })
    .await;
    assert!(clear.is_ok(), "a still page is redrawn sharp");
    let _ = tokio::process::Command::new("playwright-cli")
        .args(["-s=shared", "detach"])
        .output()
        .await;
    drain.abort();
    drop(input);
    world.stop().await;
}

#[tokio::test]
async fn moving_pictures_follow_a_shaped_link_and_a_still_page_is_sent_once_more_sharp() {
    if !inside("moving_pictures_follow_a_shaped_link_and_a_still_page_is_sent_once_more_sharp")
        .await
    {
        return;
    }
    let world = World::new().await;
    let request = world.request("levels");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    browser.cdp.call("Runtime.evaluate",json!({"expression":"document.body.innerHTML='<canvas id=c width=1280 height=800></canvas>';let cx=c.getContext('2d');window.tick=setInterval(()=>{let d=cx.createImageData(c.width,c.height);for(let i=0;i<d.data.length;i+=4){let r=Math.random()*255;d.data[i]=r;d.data[i+1]=Math.random()*255;d.data[i+2]=Math.random()*255;d.data[i+3]=255}cx.putImageData(d,0,0)},33)"}),Some(&session)).await.unwrap();
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    stream
        .input
        .send(BrowserInput {
            frame: BrowserClientFrame::Watch {
                tab: tab.clone(),
                width: 1280,
                height: 800,
                dpr: 1.0,
            },
            control_watch: true,
        })
        .await
        .unwrap();
    // Five seconds at 1.6 Mbit/s, its tenth-rate profile for twenty seconds, then more than four clear ten-second steps.
    const BASELINE: Duration = Duration::from_secs(5);
    const BAD_LINK: Duration = Duration::from_secs(20);
    // A busy test machine needs room for samples that caught a frame in transit; record five seconds at the best level once it returns.
    const RECOVERY: Duration = Duration::from_secs(120);
    const BEST_SAMPLE: Duration = Duration::from_secs(5);
    const WRITE_CHUNK: usize = 2_000;
    let began = tokio::time::Instant::now();
    let mut level = 1_u16;
    let mut seen = std::collections::BTreeSet::new();
    let mut stats = std::collections::BTreeMap::<u16, (u64, u64, f64)>::new();
    let mut last = began;
    let mut best_at = None;
    while began.elapsed() < BASELINE + BAD_LINK + RECOVERY
        && best_at.is_none_or(|at: tokio::time::Instant| at.elapsed() < BEST_SAMPLE)
    {
        let output = tokio::time::timeout(WAIT, stream.output.recv())
            .await
            .unwrap()
            .unwrap();
        let bytes = output.wire().unwrap().len();
        let now = tokio::time::Instant::now();
        let item = stats.entry(level).or_default();
        item.2 += now.duration_since(last).as_secs_f64();
        last = now;
        if output.message.opcode == ExecOpcode::Text {
            if let Ok(BrowserServerFrame::Quality {
                level: next,
                why: orch_realtime_contract::browser::BrowserQualityWhy::Link,
            }) = serde_json::from_slice(&output.message.payload)
            {
                level = next;
                best_at = (next == 0).then(tokio::time::Instant::now);
                seen.insert(next);
                eprintln!(
                    "quality at {:.3} s: {}",
                    began.elapsed().as_secs_f64(),
                    next
                );
            }
        } else {
            item.0 += 1;
            item.1 += bytes as u64;
        }
        let mut remaining = bytes;
        while remaining > 0 {
            let n = remaining.min(WRITE_CHUNK);
            let elapsed = began.elapsed();
            let rate = if elapsed < BASELINE {
                200_000.0
            } else if elapsed < BASELINE + BAD_LINK {
                20_000.0
            } else {
                0.0
            };
            if rate > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(n as f64 / rate)).await;
            }
            stream.load.handed(n as u64);
            remaining -= n;
        }
        stream.handed(&output, 0).await;
    }
    assert!(
        seen.contains(&4),
        "the link must fall to its cheapest level"
    );
    assert_eq!(
        level, 0,
        "ten clear seconds per step restore the best level"
    );
    for (level, (frames, bytes, seconds)) in stats {
        eprintln!(
            "level {level}: {frames} frames, {bytes} bytes, {seconds:.3} s, {:.3} fps, {:.0} B/s",
            frames as f64 / seconds,
            bytes as f64 / seconds
        );
    }
    browser.cdp.call("Runtime.evaluate",json!({"expression":"clearInterval(window.tick);document.body.innerHTML='<p>still and sharp</p>'"}),Some(&session)).await.unwrap();
    let quiet_by = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < quiet_by {
        if let Ok(Some(output)) =
            tokio::time::timeout(Duration::from_secs(1), stream.output.recv()).await
        {
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), stream.output.recv())
            .await
            .is_err(),
        "an unchanged page produces no more frames or metadata"
    );
    drop(stream);
    world.stop().await;
}

#[tokio::test]
async fn an_agent_browser_is_adopted_while_its_profile_lock_and_endpoint_are_respected() {
    if !inside("an_agent_browser_is_adopted_while_its_profile_lock_and_endpoint_are_respected")
        .await
    {
        return;
    }
    let world = World::new().await;
    let environment = world.host.environment(world.id).unwrap();
    let look = world
        .host
        .screen_look(&ScreenLook {
            environment_id: world.id,
        })
        .await
        .unwrap();
    assert!(!look.vnc.servers.is_empty());
    assert_eq!(look.vnc.window_manager.as_deref(), Some("openbox"));
    world
        .host
        .browser_profile(&BrowserProfileCommand {
            environment_id: world.id,
            action: BrowserProfileAction::Create,
            name: "announced".to_owned(),
            to: None,
        })
        .await
        .unwrap();
    let profile = environment.browsers().directory().join("announced");
    let mut command = orch_envhost::browser_discovery::browser_command(
        std::path::Path::new(&look.browser.programs[0].path),
        &BTreeMap::new(),
    );
    command
        .args([
            "--headless",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0",
            "--no-first-run",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut process = command.spawn().unwrap();
    let endpoint = tokio::time::timeout(WAIT, async {
        loop {
            if let Some((port, _)) = orch_envhost::browser_discovery::active_port(&profile) {
                let endpoint = format!("http://127.0.0.1:{port}");
                if orch_envhost::browser_cdp::identify(&endpoint).await.is_ok() {
                    break endpoint;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        world
            .host
            .browser_open(&world.request("announced"))
            .await
            .unwrap_err()
            .code(),
        "in_use"
    );
    let id = orch_types::new_id();
    let adopted = world
        .host
        .browser_adopt(&BrowserAdopt {
            environment_id: world.id,
            browser_id: id,
            endpoint,
        })
        .await
        .unwrap();
    assert_eq!(adopted.profile.as_deref(), Some("announced"));
    let joined = world
        .host
        .browser_open(&world.request("announced"))
        .await
        .unwrap();
    assert!(!joined.started);
    assert_eq!(joined.running, Some(id));
    let nonlocal = world
        .host
        .browser_adopt(&BrowserAdopt {
            environment_id: world.id,
            browser_id: orch_types::new_id(),
            endpoint: "http://192.0.2.1:41821".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(nonlocal.code(), "not_local");
    let no_port = world
        .host
        .browser_adopt(&BrowserAdopt {
            environment_id: world.id,
            browser_id: orch_types::new_id(),
            endpoint: "http://127.0.0.1:41821".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(no_port.code(), "not_reachable");
    assert!(no_port.to_string().contains("--remote-debugging-port=0"));
    world
        .host
        .browser_close(&BrowserClose { browser_id: id })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, process.wait())
        .await
        .unwrap()
        .unwrap();
    world.stop().await;
}

#[tokio::test]
async fn a_browser_with_a_window_uses_an_opened_display_and_both_end_with_the_environment() {
    if !inside("a_browser_with_a_window_uses_an_opened_display_and_both_end_with_the_environment")
        .await
    {
        return;
    }
    let world = World::new().await;
    let display_id = orch_types::new_id();
    let display = world
        .host
        .display_open(&orch_link_contract::browser_window::DisplayOpen {
            environment_id: world.id,
            display_id,
            geometry: "1280x800".to_owned(),
        })
        .await
        .unwrap();
    assert!(display.x_display.starts_with(':'));
    let mut request = world.request("window");
    request.preset = BrowserPreset::Window;
    request.display = Some(BrowserOpenDisplay {
        display_id,
        x_display: display.x_display,
    });
    let browser = world.host.browser_open(&request).await.unwrap();
    let look = world
        .host
        .screen_look(&ScreenLook {
            environment_id: world.id,
        })
        .await
        .unwrap();
    assert_eq!(look.displays.len(), 1);
    assert_eq!(look.browsers.len(), 1);
    world.stop().await;
    assert!(
        orch_envhost::browser_cdp::identify(&browser.endpoint)
            .await
            .is_err()
    );
    assert!(
        tokio::net::TcpStream::connect(&display.endpoint)
            .await
            .is_err()
    );
    assert!(world.dir.path().join("home/.orch/browsers/window").is_dir());
}

#[tokio::test]
async fn an_environment_without_browser_programs_still_lists_its_files_and_reports_unsupported() {
    if !inside(
        "an_environment_without_browser_programs_still_lists_its_files_and_reports_unsupported",
    )
    .await
    {
        return;
    }
    let world = World::with_browser(false).await;
    let look = world
        .host
        .screen_look(&ScreenLook {
            environment_id: world.id,
        })
        .await
        .unwrap();
    assert!(look.browser.programs.is_empty());
    assert!(look.vnc.servers.is_empty());
    assert_eq!(
        world
            .host
            .browser_open(&world.request("team"))
            .await
            .unwrap_err()
            .code(),
        "unsupported"
    );
    let files = world
        .host
        .fs_list(&FsList {
            environment_id: world.id,
            mount: None,
            path: String::new(),
            watch_id: None,
            watch_instance: None,
            page: None,
        })
        .await
        .unwrap();
    assert!(serde_json::to_string(&files).unwrap().contains("file.txt"));
    world.stop().await;
}

fn quantization(jpeg: &[u8]) -> u64 {
    let mut at = 2;
    while at + 4 < jpeg.len() {
        assert_eq!(jpeg[at], 0xff);
        let marker = jpeg[at + 1];
        let length = usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
        if marker == 0xdb {
            return jpeg[at + 5..at + 2 + length]
                .iter()
                .map(|n| u64::from(*n))
                .sum();
        }
        at += 2 + length;
    }
    panic!("the JPEG has no quantization table");
}

#[tokio::test]
async fn a_quiet_tab_is_redrawn_once_with_finer_jpeg_quantization_and_then_costs_nothing() {
    if !inside("a_quiet_tab_is_redrawn_once_with_finer_jpeg_quantization_and_then_costs_nothing")
        .await
    {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("sharp");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    browser
        .cdp
        .call(
            "Page.navigate",
            json!({"url":format!("http://127.0.0.1:{PAGE_PORT}/")}),
            Some(&session),
        )
        .await
        .unwrap();
    wait_value(&browser, &session, "document.title", json!("browser-test")).await;
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    stream
        .input
        .send(BrowserInput {
            frame: BrowserClientFrame::Watch {
                tab,
                width: 640,
                height: 400,
                dpr: 1.0,
            },
            control_watch: false,
        })
        .await
        .unwrap();
    let (mut first, mut sharp) = (None, None);
    let mut clear = false;
    tokio::time::timeout(WAIT, async {
        while sharp.is_none() {
            let output = stream.output.recv().await.unwrap();
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
            match output.message.opcode {
                ExecOpcode::Binary => {
                    if first.is_none() {
                        first = Some(quantization(&output.message.payload[12..]));
                    } else if clear {
                        sharp = Some(quantization(&output.message.payload[12..]));
                    }
                }
                ExecOpcode::Text => {
                    clear |= matches!(
                        serde_json::from_slice::<BrowserServerFrame>(&output.message.payload)
                            .unwrap(),
                        BrowserServerFrame::Quality {
                            level: 0,
                            why: orch_realtime_contract::browser::BrowserQualityWhy::Clear
                        }
                    );
                }
            }
        }
    })
    .await
    .unwrap();
    assert!(
        sharp.unwrap() < first.unwrap(),
        "the still picture uses finer JPEG quantization"
    );
    let settle = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < settle {
        if let Ok(Some(output)) =
            tokio::time::timeout(Duration::from_millis(500), stream.output.recv()).await
        {
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
        }
    }
    let quiet = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < quiet {
        if let Ok(Some(output)) = tokio::time::timeout_at(quiet, stream.output.recv()).await {
            assert_eq!(
                output.message.opcode,
                ExecOpcode::Text,
                "an unchanged page produced another JPEG"
            );
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
        }
    }
    drop(stream);
    world.stop().await;
}

async fn drive(sender: &tokio::sync::mpsc::Sender<BrowserInput>, frame: Json) {
    let frame = serde_json::from_value(frame).unwrap();
    sender
        .send(BrowserInput {
            frame,
            control_watch: true,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_loading_tab_keeps_its_loading_flag_until_its_page_has_finished() {
    if !inside("a_loading_tab_keeps_its_loading_flag_until_its_page_has_finished").await {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("loading");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    drive(
        &stream.input,
        json!({"op":"watch","tab":tab,"width":1280,"height":800,"dpr":1}),
    )
    .await;
    drive(
        &stream.input,
        json!({"op":"navigate","tab":tab,"url":"http://127.0.0.1:41820/slow"}),
    )
    .await;
    tokio::time::timeout(WAIT, async {
        let mut loading = false;
        let mut changed = false;
        while let Some(output) = stream.output.recv().await {
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
            if output.message.opcode != ExecOpcode::Text {
                continue;
            }
            let frame: BrowserServerFrame =
                serde_json::from_slice(&output.message.payload).unwrap();
            if let BrowserServerFrame::Tabs { tabs } = frame {
                let Some(found) = tabs.iter().find(|t| t.id == tab) else {
                    continue;
                };
                loading |= found.loading;
                if found.loading && found.title == "loaded-slow" && !changed {
                    value(&browser, &session, "document.title='during-load'").await;
                    changed = true;
                }
                if changed && found.title == "during-load" && !found.loading {
                    assert!(loading, "the navigation announced that it was loading");
                    assert_eq!(
                        value(&browser, &session, "document.readyState").await,
                        "complete",
                        "a title event must not clear loading before the page finishes"
                    );
                    return;
                }
            }
        }
        panic!("the tab stream ended before the navigation finished");
    })
    .await
    .unwrap();
    world.stop().await;
}

#[tokio::test]
async fn a_page_changing_its_title_does_not_keep_an_unwatched_browser_from_being_idle() {
    if !inside("a_page_changing_its_title_does_not_keep_an_unwatched_browser_from_being_idle").await
    {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("activity");
    world.host.browser_open(&request).await.unwrap();
    let browser = world
        .host
        .environment(world.id)
        .unwrap()
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (_, session) = page(&browser).await;
    browser
        .cdp
        .call(
            "Page.navigate",
            json!({"url":"http://127.0.0.1:41820/"}),
            Some(&session),
        )
        .await
        .unwrap();
    wait_value(&browser, &session, "document.title", json!("browser-test")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    world.browser_states.lock().await.clear();
    value(
        &browser,
        &session,
        "document.title='a title is not activity'",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        world.browser_states.lock().await.is_empty(),
        "changing a title must not refresh the idle clock"
    );
    browser
        .cdp
        .call(
            "Page.navigate",
            json!({"url":"http://127.0.0.1:41820/?a-tab-went-elsewhere"}),
            Some(&session),
        )
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if !world.browser_states.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        world.browser_states.lock().await.last().unwrap().tabs,
        Some(1)
    );
    world.stop().await;
}

#[tokio::test]
async fn watching_does_not_resize_the_page_and_a_new_view_keeps_the_drivers_size() {
    if !inside("watching_does_not_resize_the_page_and_a_new_view_keeps_the_drivers_size").await {
        return;
    }
    let world = World::new().await;
    let request = world.request("layout");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    // An agent may already be working in a layout of its own when somebody watches it.
    browser
        .cdp
        .call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":360,"height":240,"deviceScaleFactor":1,"mobile":false}),
            Some(&session),
        )
        .await
        .unwrap();
    let mut screen = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    let ready = tokio::time::timeout(WAIT, async {
        loop {
            let output = screen.output.recv().await.unwrap();
            screen
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
            if output.message.opcode == ExecOpcode::Text {
                let frame: BrowserServerFrame =
                    serde_json::from_slice(&output.message.payload).unwrap();
                if let BrowserServerFrame::Ready { viewport, .. } = frame {
                    break viewport;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((ready.width, ready.height), (360, 240));
    screen
        .input
        .send(BrowserInput {
            frame: serde_json::from_value(
                json!({"op":"watch","tab":tab,"width":1280,"height":800,"dpr":1}),
            )
            .unwrap(),
            control_watch: false,
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let output = screen.output.recv().await.unwrap();
            screen
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
            if output.message.opcode == ExecOpcode::Binary {
                let picture = PictureHeader::decode(&output.message.payload).unwrap();
                assert!(picture.image.width <= 360 && picture.image.height <= 240);
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(value(&browser, &session, "window.innerWidth").await, 360);
    assert_eq!(value(&browser, &session, "window.innerHeight").await, 240);
    drive(
        &screen.input,
        json!({"op":"viewport","width":640,"height":480}),
    )
    .await;
    wait_value(&browser, &session, "window.innerWidth", json!(640)).await;
    wait_value(&browser, &session, "window.innerHeight", json!(480)).await;
    drop(screen);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut next = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    let ready = tokio::time::timeout(WAIT, async {
        loop {
            let output = next.output.recv().await.unwrap();
            next.handed(&output, output.wire().unwrap().len() as u64)
                .await;
            if output.message.opcode == ExecOpcode::Text {
                let frame: BrowserServerFrame =
                    serde_json::from_slice(&output.message.payload).unwrap();
                if let BrowserServerFrame::Ready { viewport, .. } = frame {
                    break viewport;
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!((ready.width, ready.height), (640, 480));
    assert_eq!(value(&browser, &session, "window.innerWidth").await, 640);
    assert_eq!(value(&browser, &session, "window.innerHeight").await, 480);
    drop(next);
    world.stop().await;
}

async fn metadata(
    receiver: &mut tokio::sync::broadcast::Receiver<BrowserServerFrame>,
    predicate: impl Fn(&BrowserServerFrame) -> bool,
) -> BrowserServerFrame {
    // A dialog may consume its whole minute before it is dismissed.
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match receiver.recv().await {
                Ok(frame) if predicate(&frame) => break frame,
                Ok(BrowserServerFrame::Error { code, detail }) => {
                    panic!("control failed: {code:?}: {detail}")
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("control ended: {error}"),
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn typing_scrolling_tabs_history_dialogs_workspace_files_copy_and_resize_reach_the_page() {
    if !inside(
        "typing_scrolling_tabs_history_dialogs_workspace_files_copy_and_resize_reach_the_page",
    )
    .await
    {
        return;
    }
    server().await;
    let world = World::new().await;
    let request = world.request("controls");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    let address = format!("http://127.0.0.1:{PAGE_PORT}/?first");
    browser
        .cdp
        .call("Page.navigate", json!({"url":address}), Some(&session))
        .await
        .unwrap();
    wait_value(&browser, &session, "document.title", json!("browser-test")).await;
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    let input = stream.input.clone();
    let (frames, mut receiver) = tokio::sync::broadcast::channel(128);
    let drain = tokio::spawn(async move {
        while let Some(output) = stream.output.recv().await {
            stream
                .handed(&output, output.wire().unwrap().len() as u64)
                .await;
            if output.message.opcode == ExecOpcode::Text {
                let frame =
                    serde_json::from_slice::<BrowserServerFrame>(&output.message.payload).unwrap();
                let _ = frames.send(frame);
            }
        }
    });
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::Ready { .. })
    })
    .await;
    drive(
        &input,
        json!({"op":"watch","tab":tab,"width":1280,"height":800,"dpr":1}),
    )
    .await;
    // Wait for the watch's metric change, which occurs before input is sent.
    wait_value(&browser, &session, "window.innerHeight", json!(800)).await;
    value(&browser, &session, "document.getElementById('i').focus()").await;
    for kind in ["down", "up"] {
        drive(
            &input,
            json!({"op":"key","tab":tab,"kind":kind,"key":"a","code":"KeyA","text":"a"}),
        )
        .await;
    }
    wait_value(
        &browser,
        &session,
        "document.getElementById('i').value",
        json!("a"),
    )
    .await;
    drive(&input, json!({"op":"text","tab":tab,"text":" pasted 中文"})).await;
    wait_value(
        &browser,
        &session,
        "document.getElementById('i').value",
        json!("a pasted 中文"),
    )
    .await;
    value(&browser, &session, "document.body.style.height='4000px'").await;
    drive(
        &input,
        json!({"op":"pointer","tab":tab,"kind":"move","x":500,"y":500}),
    )
    .await;
    drive(
        &input,
        json!({"op":"pointer","tab":tab,"kind":"wheel","x":500,"y":500,"dy":600,"dx":0}),
    )
    .await;
    wait_value(&browser, &session, "window.scrollY>0", json!(true)).await;
    value(&browser, &session, "window.scrollTo(0,0)").await;
    value(&browser,&session,"document.getElementById('count').textContent='selected';let r=document.createRange();r.selectNodeContents(document.getElementById('count'));let s=window.getSelection();s.removeAllRanges();s.addRange(r)").await;
    drive(&input, json!({"op":"copy","tab":tab})).await;
    assert!(
        matches!(metadata(&mut receiver,|f|matches!(f,BrowserServerFrame::Copied{..})).await,BrowserServerFrame::Copied{text} if text=="selected")
    );
    let position=value(&browser,&session,"JSON.stringify((()=>{let r=document.getElementById('f').getBoundingClientRect();return {x:r.x+10,y:r.y+r.height/2}})())").await;
    let position: Json = serde_json::from_str(position.as_str().unwrap()).unwrap();
    for kind in ["down", "up"] {
        drive(&input,json!({"op":"pointer","tab":tab,"kind":kind,"x":position["x"],"y":position["y"],"button":"left","clicks":1})).await;
    }
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::Chooser { .. })
    })
    .await;
    drive(&input, json!({"op":"files","tab":tab,"paths":["file.txt"]})).await;
    wait_value(
        &browser,
        &session,
        "document.getElementById('f').files[0]?.name",
        json!("file.txt"),
    )
    .await;
    value(
        &browser,
        &session,
        "setTimeout(()=>{window.answer=prompt('A test question','initial')},0)",
    )
    .await;
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::Dialog { .. })
    })
    .await;
    drive(
        &input,
        json!({"op":"dialog","tab":tab,"accept":true,"text":"answered"}),
    )
    .await;
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::DialogClosed { .. })
    })
    .await;
    wait_value(&browser, &session, "window.answer", json!("answered")).await;
    value(
        &browser,
        &session,
        "setTimeout(()=>{window.answer=confirm('Left unanswered')},0)",
    )
    .await;
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::Dialog { .. })
    })
    .await;
    let asked = tokio::time::Instant::now();
    metadata(&mut receiver, |f| {
        matches!(f, BrowserServerFrame::DialogClosed { .. })
    })
    .await;
    assert!(asked.elapsed() >= Duration::from_secs(59));
    wait_value(&browser, &session, "window.answer", json!(false)).await;
    drive(
        &input,
        json!({"op":"navigate","tab":tab,"url":format!("http://127.0.0.1:{PAGE_PORT}/?second")}),
    )
    .await;
    wait_value(&browser, &session, "location.search", json!("?second")).await;
    for (direction, query) in [("back", "?first"), ("forward", "?second")] {
        drive(&input, json!({"op":"history","tab":tab,"to":direction})).await;
        wait_value(&browser, &session, "location.search", json!(query)).await;
    }
    value(&browser, &session, "window.changed=true").await;
    drive(&input, json!({"op":"history","tab":tab,"to":"reload"})).await;
    wait_value(
        &browser,
        &session,
        "typeof window.changed",
        json!("undefined"),
    )
    .await;
    drive(&input, json!({"op":"history","tab":tab,"to":"stop"})).await;
    browser
        .cdp
        .call(
            "Runtime.evaluate",
            json!({"expression":"window.open('about:blank')","userGesture":true}),
            Some(&session),
        )
        .await
        .unwrap();
    let tabs = metadata(
        &mut receiver,
        |f| matches!(f,BrowserServerFrame::Tabs{tabs} if tabs.len()==2),
    )
    .await;
    let BrowserServerFrame::Tabs { tabs } = tabs else {
        unreachable!()
    };
    assert!(tabs.iter().any(|t| t.opener.as_deref() == Some(&tab)));
    let other = tabs.iter().find(|t| t.id != tab).unwrap().id.clone();
    drive(&input, json!({"op":"tab","do":"activate","tab":other})).await;
    drive(&input, json!({"op":"viewport","width":900,"height":600})).await;
    metadata(&mut receiver,|f|matches!(f,BrowserServerFrame::Ready{viewport,..} if viewport.width==900 && viewport.height==600)).await;
    wait_value(&browser, &session, "window.innerWidth", json!(900)).await;
    wait_value(&browser, &session, "window.innerHeight", json!(600)).await;
    drive(&input, json!({"op":"tab","do":"close","tab":other})).await;
    metadata(
        &mut receiver,
        |f| matches!(f,BrowserServerFrame::Tabs{tabs} if tabs.len()==1),
    )
    .await;
    drain.abort();
    drop(input);
    world.stop().await;
}

#[tokio::test]
async fn a_connected_host_uses_its_private_profiles_only_while_the_machine_owner_allows_browsers() {
    if !inside(
        "a_connected_host_uses_its_private_profiles_only_while_the_machine_owner_allows_browsers",
    )
    .await
    {
        return;
    }
    use orch_envhost::environment::Gates;
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let personal = directory.path().join("personal-profile");
    std::fs::create_dir(&personal).unwrap();
    std::fs::write(personal.join("marker"), "personal cookies").unwrap();
    let settings = HostSettings::connected(
        directory.path().join("connector"),
        vec![directory.path().to_path_buf()],
    );
    let (reports, mut receiver) = Reports::channel(128);
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    let id = orch_types::new_id();
    host.env_attach(
        &EnvAttach {
            resources: None,
            data_generation: None,
            storage: None,
            environment_id: id,
            data_sources: None,
            workspace_root: Some(workspace.to_string_lossy().into_owned()),
            ignore: Vec::new(),
            index: IndexMode::Full,
            mounts: None,
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
        },
        &BTreeMap::new(),
    )
    .await
    .unwrap();
    let request = BrowserOpen {
        environment_id: id,
        browser_id: orch_types::new_id(),
        profile: "team".to_owned(),
        create: true,
        preset: BrowserPreset::Headless,
        args: Vec::new(),
        viewport: "1280x800".to_owned(),
        url: None,
        display: None,
    };
    assert!(host.browser_open(&request).await.is_err());
    let environment = host.environment(id).unwrap();
    let profiles = directory
        .path()
        .join("connector/browsers")
        .join(id.to_string());
    assert_eq!(environment.browsers().directory(), profiles);
    assert!(!profiles.join("team").exists());
    host.set_policy(
        Gates {
            browser: true,
            ..Gates::CLOSED
        },
        Some(vec![directory.path().to_path_buf()]),
        None,
        orch_envhost::mcp::McpGates {
            add: false,
            import: false,
        },
    );
    tokio::time::timeout(WAIT, async {
        while environment.browsers().require_allowed().await.is_err() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let opened = host.browser_open(&request).await.unwrap();
    let listed = environment.processes().list();
    assert!(
        listed
            .iter()
            .any(|p| p.info.command.contains("user-data-dir"))
    );
    assert!(profiles.join("team").is_dir());
    assert_eq!(
        std::fs::read_to_string(personal.join("marker")).unwrap(),
        "personal cookies"
    );
    host.set_policy(
        Gates::CLOSED,
        Some(vec![directory.path().to_path_buf()]),
        None,
        orch_envhost::mcp::McpGates {
            add: false,
            import: false,
        },
    );
    tokio::time::timeout(WAIT, async {
        while environment
            .browsers()
            .get(request.browser_id)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        orch_envhost::browser_cdp::identify(&opened.endpoint)
            .await
            .is_err()
    );
    assert!(profiles.join("team").is_dir());
    host.env_detach(&EnvDetach { environment_id: id })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_picture_recovers_through_real_packet_loss_delay_and_the_floor_rate() {
    if !browser_netem::inside(
        "the_picture_recovers_through_real_packet_loss_delay_and_the_floor_rate",
    )
    .await
    {
        return;
    }
    browser_netem::begin().await;
    let world = World::new().await;
    let request = world.request("netem");
    world.host.browser_open(&request).await.unwrap();
    let environment = world.host.environment(world.id).unwrap();
    let browser = environment
        .browsers()
        .get(request.browser_id)
        .await
        .unwrap();
    let (tab, session) = page(&browser).await;
    // Two changing pictures per second remain expensive on the constrained path and leave a genuinely empty queue on the restored path.
    browser.cdp.call("Runtime.evaluate",json!({"expression":"document.body.innerHTML='<canvas id=c width=1280 height=800></canvas>';let cx=c.getContext('2d');window.tick=setInterval(()=>{let d=cx.createImageData(c.width,c.height);for(let i=0;i<d.data.length;i+=4){d.data[i]=Math.random()*255;d.data[i+1]=Math.random()*255;d.data[i+2]=Math.random()*255;d.data[i+3]=255}cx.putImageData(d,0,0)},500)"}),Some(&session)).await.unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", browser_netem::PORT))
        .await
        .unwrap();
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_send_buffer_size(16 * 1024).unwrap();
    let mut writer = socket
        .connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut reader, _) = listener.accept().await.unwrap();
    let mut stream = BrowserStream::start(Arc::clone(&browser), environment.files().clone());
    let input = stream.input.clone();
    let producer = tokio::spawn(async move {
        while let Some(output) = stream.output.recv().await {
            let wire = output.wire().unwrap();
            let mut remaining = wire.as_ref();
            while !remaining.is_empty() {
                let n = writer.write(remaining).await.unwrap();
                assert_ne!(n, 0);
                stream.load.handed(n as u64);
                remaining = &remaining[n..];
            }
            stream.handed(&output, 0).await;
        }
    });
    browser_netem::start();
    drive(
        &input,
        json!({"op":"watch","tab":tab,"width":1280,"height":800,"dpr":1}),
    )
    .await;
    // Four clear ten-second steps may be interrupted by a sampled packet in transit. Stop after five measured seconds at the best level, with three minutes as the ceiling.
    let began = tokio::time::Instant::now();
    let mut best = None;
    let mut level = 1_u16;
    let mut seen = std::collections::BTreeSet::new();
    let mut stats = BTreeMap::<u16, (u64, u64, f64)>::new();
    let mut last = began;
    let mut inbox = bytes::BytesMut::new();
    let mut read = [0; 16 * 1024];
    while began.elapsed() < Duration::from_secs(180)
        && best.is_none_or(|at: tokio::time::Instant| at.elapsed() < Duration::from_secs(5))
    {
        let message = loop {
            if let Some(message) =
                orch_link_contract::tunnels::ExecMessage::take(&mut inbox, 16 * 1024 * 1024)
                    .unwrap()
            {
                break message;
            }
            let n = tokio::time::timeout(WAIT, reader.read(&mut read))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(n, 0);
            inbox.extend_from_slice(&read[..n]);
        };
        let now = tokio::time::Instant::now();
        let item = stats.entry(level).or_default();
        item.2 += now.duration_since(last).as_secs_f64();
        last = now;
        if message.opcode == ExecOpcode::Text {
            let value: Json = serde_json::from_slice(&message.payload).unwrap();
            if let Ok(BrowserServerFrame::Quality {
                level: next,
                why: orch_realtime_contract::browser::BrowserQualityWhy::Link,
            }) = serde_json::from_value(value["frame"].clone())
            {
                level = next;
                seen.insert(next);
                best = (next == 0).then(tokio::time::Instant::now);
                eprintln!(
                    "netem quality at {:.3}s: {next}",
                    began.elapsed().as_secs_f64()
                );
            }
        } else {
            item.0 += 1;
            item.1 += message.payload.len() as u64;
        }
    }
    assert!(seen.contains(&4));
    assert_eq!(level, 0);
    for (level, (frames, bytes, seconds)) in stats {
        eprintln!(
            "netem level {level}: {:.3} fps, {:.0} B/s ({frames} frames in {seconds:.3}s)",
            frames as f64 / seconds,
            bytes as f64 / seconds
        );
    }
    producer.abort();
    drop(input);
    world.stop().await;
}
