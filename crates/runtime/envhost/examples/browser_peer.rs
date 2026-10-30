//! A test-only environment peer that runs the shipped browser behind a private Unix socket.
#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test peer fails by panicking"
)]

#[cfg(unix)]
use bytes::BytesMut;
#[cfg(unix)]
use orch_envhost::blobs::MemoryBlobs;
#[cfg(unix)]
use orch_envhost::browser_screen::{BrowserInput, BrowserStream};
#[cfg(unix)]
use orch_envhost::environment::{EnvHost, HostSettings, ManagedPlaces, Reports};
#[cfg(unix)]
use orch_envhost::processes::ScopeKind;
#[cfg(unix)]
use orch_link_contract::commands::{Command, CommandResult, Empty, EnvAttach, IndexMode};
#[cfg(unix)]
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode};
#[cfg(unix)]
use orch_types::{Id, Json, SystemClock};
#[cfg(unix)]
use serde_json::json;
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The peer's fixture page is served only in the unit's loopback namespace.
#[cfg(unix)]
const PAGE_PORT: u16 = 41820;
/// One maximum viewport JPEG is much smaller than sixteen mebibytes.
#[cfg(unix)]
const MESSAGE_MAX: usize = 16 * 1024 * 1024;

#[cfg(unix)]
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let socket = PathBuf::from(args.next().unwrap());
    let id: Id = args.next().unwrap().parse().unwrap();
    let generation = args.next().map(|n| n.parse().unwrap()).unwrap_or(1);
    let root = socket.parent().unwrap().to_path_buf();
    let workspace = root.join("workspace");
    let home = root.join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(workspace.join("file.txt"), "workspace file").unwrap();
    let mut settings = HostSettings::managed(
        root.join("state"),
        ManagedPlaces {
            workspace_root: workspace,
            home: Some(home),
            mounts_file: None,
            tools_dir: None,
            mounts_enforced: false,
        },
        ScopeKind::All,
    );
    settings.generation = generation;
    let (reports, mut receiver) = Reports::channel(128);
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });
    let host = EnvHost::new(
        settings,
        Arc::new(SystemClock),
        reports,
        Arc::new(MemoryBlobs::new()),
        Vec::new(),
    );
    host.env_attach(
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
    page_server().await;
    if socket.exists() {
        std::fs::remove_file(&socket).unwrap();
    }
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let tunnels = Arc::new(std::sync::atomic::AtomicU64::new(0));
    println!("ready");
    loop {
        let (mut stream, _) = listener.accept().await.unwrap();
        let host = host.clone();
        let root = root.clone();
        let tunnels = Arc::clone(&tunnels);
        tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                let mut b = [0];
                if stream.read_exact(&mut b).await.is_err() {
                    return;
                }
                if b[0] == b'\n' {
                    break;
                }
                request.push(b[0]);
                if request.len() > MESSAGE_MAX {
                    return;
                }
            }
            let Ok(value) = serde_json::from_slice::<Json>(&request) else {
                return;
            };
            if value["tunnels"].as_bool() == Some(true) {
                stream
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"ok":tunnels.load(std::sync::atomic::Ordering::Relaxed)})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                return;
            }
            if let Some(port) = value["generic_port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
            {
                let environment = host.environment(id).unwrap();
                let result = environment
                    .ports()
                    .owned(port, orch_api_contract::ports::PortTransport::Tcp);
                let reply = match result {
                    Ok(_) => json!({"ok":true}),
                    Err(error) => json!({"error":error.to_string()}),
                };
                stream
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .unwrap();
                return;
            }
            if let Some(password) = value["launch_protected_display"].as_str() {
                use des::cipher::{BlockEncrypt, KeyInit};
                use std::os::unix::fs::OpenOptionsExt;
                // The test server consumes TigerVNC's fixed-key eight-byte password-file format.
                let key = [23u8, 82, 107, 6, 35, 78, 88, 7].map(u8::reverse_bits);
                let mut block = [0u8; 8];
                for (slot, byte) in block.iter_mut().zip(password.as_bytes()) {
                    *slot = *byte;
                }
                let cipher = des::Des::new((&key).into());
                cipher.encrypt_block((&mut block).into());
                let password_file = root.join("protected-display-password");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&password_file)
                    .unwrap();
                std::io::Write::write_all(&mut file, &block).unwrap();
                let mut command = tokio::process::Command::new("Xvnc");
                command
                    .args([
                        ":90",
                        "-geometry",
                        "640x480",
                        "-depth",
                        "24",
                        "-rfbport",
                        "41990",
                        "-localhost",
                        "-nolisten",
                        "tcp",
                        "-AlwaysShared",
                        "-SecurityTypes",
                        "VncAuth",
                        "-PasswordFile",
                    ])
                    .arg(&password_file)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(false);
                let mut child = command.spawn().unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        assert!(child.try_wait().unwrap().is_none());
                        if tokio::net::TcpStream::connect("127.0.0.1:41990")
                            .await
                            .is_ok()
                        {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                })
                .await
                .unwrap();
                stream
                    .write_all(
                        format!("{}\n", json!({"ok":{"endpoint":"127.0.0.1:41990"}})).as_bytes(),
                    )
                    .await
                    .unwrap();
                return;
            }
            if let Some(display) = value["recover_display"]
                .as_str()
                .and_then(|v| v.parse::<Id>().ok())
            {
                let recovered = orch_envhost::browser_window::Windows::with_store(
                    BTreeMap::new(),
                    root.join("state/displays").join(id.to_string()),
                )
                .unwrap();
                let (endpoint, x_display) = recovered.open(display, "1280x800").await.unwrap();
                stream
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"ok":{"endpoint":endpoint,"x_display":x_display}})
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                return;
            }
            if let Some(with_port) = value["launch_cli"].as_bool() {
                let environment = host.environment(id).unwrap();
                let profile = environment.browsers().directory().join(if with_port {
                    "cli-announced"
                } else {
                    "cli-no-port"
                });
                std::fs::create_dir_all(&profile).unwrap();
                let config = root.join(if with_port {
                    "cli-announced.json"
                } else {
                    "cli-no-port.json"
                });
                std::fs::write(&config,json!({"browser":{"launchOptions":{"args":if with_port {vec!["--remote-debugging-port=0"]}else{vec![]}}}}).to_string()).unwrap();
                let output = tokio::process::Command::new("playwright-cli")
                    .arg(if with_port {
                        "-s=cli-announced"
                    } else {
                        "-s=cli-no-port"
                    })
                    .arg("open")
                    .arg(format!("--profile={}", profile.display()))
                    .env("PLAYWRIGHT_MCP_CONFIG", config)
                    .current_dir(&root)
                    .output()
                    .await
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let endpoint = orch_envhost::browser_discovery::active_port(&profile)
                    .map(|(port, _)| format!("http://127.0.0.1:{port}"));
                stream.write_all(format!("{}\n",json!({"ok":{"endpoint":endpoint,"profile":profile.file_name().unwrap().to_string_lossy()}})).as_bytes()).await.unwrap();
                return;
            }
            if let Some(browser) = value["cookies"].as_str().and_then(|v| v.parse::<Id>().ok()) {
                let environment = host.environment(id).unwrap();
                let browser = environment.browsers().get(browser).await.unwrap();
                let cookies = browser
                    .cdp
                    .call("Storage.getCookies", json!({}), None)
                    .await
                    .unwrap();
                stream
                    .write_all(format!("{}\n", json!({"ok":cookies})).as_bytes())
                    .await
                    .unwrap();
                return;
            }
            if let Some(display) = value["rfb"].as_str().and_then(|v| v.parse::<Id>().ok()) {
                let environment = host.environment(id).unwrap();
                let mut connection = environment.displays().connect(display).await.unwrap();
                stream
                    .write_all(&connection.authenticated.version)
                    .await
                    .unwrap();
                assert!(matches!(stream.read_u8().await.unwrap(), 0 | 1));
                connection.authenticated.stream.write_u8(1).await.unwrap();
                tunnels.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let relay = tokio::io::copy_bidirectional(
                    &mut stream,
                    &mut connection.authenticated.stream,
                );
                tokio::select! {_=relay=>{},_=connection.stopped.cancelled()=>{}}
                return;
            }
            if value.get("tunnel").is_some() {
                let Some(browser) = value["tunnel"].as_str().and_then(|v| v.parse::<Id>().ok())
                else {
                    return;
                };
                let Ok(environment) = host.environment(id) else {
                    return;
                };
                let Some(browser) = environment.browsers().get(browser).await else {
                    return;
                };
                tunnels.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                bridge(
                    stream,
                    BrowserStream::start(browser, environment.files().clone()),
                )
                .await;
                return;
            }
            if let Some(browser) = value["observe"].as_str().and_then(|v| v.parse::<Id>().ok()) {
                let environment = host.environment(id).unwrap();
                let browser = environment.browsers().get(browser).await.unwrap();
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
                    .clone();
                let attached = browser
                    .cdp
                    .call(
                        "Target.attachToTarget",
                        json!({"targetId":tab,"flatten":true}),
                        None,
                    )
                    .await
                    .unwrap();
                let session = attached["sessionId"].as_str().unwrap();
                let reply = browser
                    .cdp
                    .call(
                        "Runtime.evaluate",
                        json!({"expression":value["expression"],"returnByValue":true}),
                        Some(session),
                    )
                    .await
                    .unwrap();
                let _ = stream
                    .write_all(format!("{}\n", json!({"ok":reply["result"]["value"]})).as_bytes())
                    .await;
                return;
            }
            let Ok(command) = serde_json::from_value::<Command>(value["command"].clone()) else {
                return;
            };
            let result = match command {
                Command::BrowserOpen(body) => host
                    .browser_open(&body)
                    .await
                    .map(CommandResult::BrowserOpen),
                Command::BrowserAdopt(body) => host
                    .browser_adopt(&body)
                    .await
                    .map(CommandResult::BrowserAdopt),
                Command::BrowserClose(body) => host
                    .browser_close(&body)
                    .await
                    .map(|()| CommandResult::BrowserClose(Empty {})),
                Command::BrowserProfile(body) => host
                    .browser_profile(&body)
                    .await
                    .map(CommandResult::BrowserProfile),
                Command::EnvDetach(body) => host
                    .env_detach(&body)
                    .await
                    .map_err(|error| {
                        orch_envhost::browsers::BrowserError::Failed(error.to_string())
                    })
                    .map(|()| CommandResult::EnvDetach(Empty {})),
                Command::ScreenLook(body) => {
                    host.screen_look(&body).await.map(CommandResult::ScreenLook)
                }
                Command::DisplayOpen(body) => host
                    .display_open(&body)
                    .await
                    .map(CommandResult::DisplayOpen)
                    .map_err(|error| {
                        orch_envhost::browsers::BrowserError::Failed(error.to_string())
                    }),
                Command::DisplayAdopt(body) => host
                    .environment(body.environment_id)
                    .unwrap()
                    .displays()
                    .adopt(
                        body.display_id,
                        &body.endpoint,
                        value["password"]
                            .as_str()
                            .map(|password| password.as_bytes().to_vec()),
                    )
                    .await
                    .map(CommandResult::DisplayAdopt)
                    .map_err(|error| {
                        orch_envhost::browsers::BrowserError::Failed(error.to_string())
                    }),
                Command::DisplayClose(body) => {
                    host.environment(id)
                        .unwrap()
                        .displays()
                        .close(body.display_id)
                        .await;
                    Ok(CommandResult::DisplayClose(Empty {}))
                }
                _ => return,
            };
            let reply = match result {
                Ok(result) => json!({"ok":result}),
                Err(error) => json!({"error":{"code":error.code(),"detail":error.to_string()}}),
            };
            let _ = stream.write_all(format!("{reply}\n").as_bytes()).await;
        });
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("the browser test peer requires a Unix socket");
}

#[cfg(unix)]
async fn page_server() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", PAGE_PORT))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut bytes = [0; 8192];
                let _ = socket.read(&mut bytes).await;
                let page = "<!doctype html><title>api-page</title><input id=i autofocus><button id=b onclick=\"document.title='clicked'\">Go</button><div id=counter>unchanged</div><input id=f type=file><script>window.actions=0;document.addEventListener('keydown',()=>actions++);document.addEventListener('mousedown',()=>actions++);document.addEventListener('input',()=>actions++);window.testURL='';</script>";
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: login=kept; Max-Age=3600; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
}

#[cfg(unix)]
async fn bridge(stream: tokio::net::UnixStream, mut screen: BrowserStream) {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let input = screen.input.clone();
    let upload = async {
        while let Some(output) = screen.output.recv().await {
            let bytes = output.wire().unwrap();
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
            screen.handed(&output, bytes.len() as u64).await;
        }
    };
    let download = async {
        let mut inbox = BytesMut::new();
        let mut bytes = [0; 16384];
        while let Ok(n) = reader.read(&mut bytes).await {
            if n == 0 {
                break;
            }
            inbox.extend_from_slice(&bytes[..n]);
            loop {
                let message = match ExecMessage::take(&mut inbox, MESSAGE_MAX) {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(_) => return,
                };
                if message.opcode != ExecOpcode::Text {
                    return;
                }
                let mut value: Json = serde_json::from_slice(&message.payload).unwrap();
                let control = value
                    .as_object_mut()
                    .and_then(|v| v.remove("control"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let frame = serde_json::from_value(value).unwrap();
                if input
                    .send(BrowserInput {
                        frame,
                        control_watch: control,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    };
    tokio::select! {_=upload=>{},_=download=>{}}
}
