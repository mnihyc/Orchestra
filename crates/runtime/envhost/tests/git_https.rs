//! Real HTTPS Git authentication keeps source and environment credentials in their own repositories.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[cfg(unix)]
use base64::Engine as _;
#[cfg(unix)]
use bytes::Bytes;
#[cfg(unix)]
use http_body_util::{BodyExt, Full};
#[cfg(unix)]
use hyper::{Request, Response, body::Incoming, service::service_fn};
#[cfg(unix)]
use hyper_util::rt::TokioIo;
#[cfg(unix)]
use orch_api_contract::source::SourceDestination;
#[cfg(unix)]
use orch_envhost::{
    environment::{GateState, Gates, Limits},
    file_states::FileStates,
    files::Files,
    git::GitTool,
    git_actions, git_publication,
    ignore::IgnoreRules,
    index::HashCache,
    paths::{PathStyle, Workspace},
    state_store::ObjectStore,
};
#[cfg(unix)]
use orch_link_contract::{
    commands::Secret,
    source::{GitAuthor, GitCommit, GitPrepare, GitPush},
};
#[cfg(unix)]
use orch_types::new_id;
#[cfg(unix)]
use std::{
    convert::Infallible,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
#[cfg(unix)]
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinSet};

// Isolated disposable test identities, never credentials of a person or a provider.
#[cfg(unix)]
const SOURCE_PASSWORD: &str = "source-fixture-secret";
#[cfg(unix)]
const ENVIRONMENT_PASSWORD: &str = "environment-fixture-secret";
#[cfg(unix)]
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(unix)]
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn native(program: &Path, cwd: &Path, arguments: &[&str]) -> Vec<u8> {
    let reply = std::process::Command::new(program)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        reply.status.success(),
        "Git fixture: {}",
        String::from_utf8_lossy(&reply.stderr)
    );
    reply.stdout
}

#[cfg(unix)]
struct StopServer(tokio::task::JoinHandle<()>);
#[cfg(unix)]
impl Drop for StopServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
#[cfg(unix)]
struct Backend {
    program: PathBuf,
    root: PathBuf,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

#[cfg(unix)]
impl Backend {
    async fn reply(self, request: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
        let path = request.uri().path().to_owned();
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        self.seen
            .lock()
            .unwrap()
            .push((path.clone(), authorization.clone()));
        if path.starts_with("/redirect.git/") {
            return Ok(Response::builder()
                .status(302)
                .header("location", "/other.git/info/refs?service=git-upload-pack")
                .body(Full::new(Bytes::new()))
                .unwrap());
        }
        let password = if path.starts_with("/other.git/") {
            ENVIRONMENT_PASSWORD
        } else {
            SOURCE_PASSWORD
        };
        let username = if path.starts_with("/other.git/") {
            "environment"
        } else {
            "source"
        };
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
        );
        if authorization != expected {
            return Ok(Response::builder()
                .status(401)
                .header("www-authenticate", "Basic realm=\"Git fixture\"")
                .body(Full::new(Bytes::new()))
                .unwrap());
        }
        let method = request.method().as_str().to_owned();
        let query = request.uri().query().unwrap_or_default().to_owned();
        let content_type = request
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let protocol = request
            .headers()
            .get("git-protocol")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = request.into_body().collect().await.unwrap().to_bytes();
        let mut child = tokio::process::Command::new(&self.program)
            .args(["-c", "core.hooksPath=/dev/null", "http-backend"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_PROJECT_ROOT", &self.root)
            .env("GIT_HTTP_EXPORT_ALL", "1")
            .env("PATH_INFO", path)
            .env("REQUEST_METHOD", method)
            .env("QUERY_STRING", query)
            .env("CONTENT_TYPE", content_type)
            .env("CONTENT_LENGTH", body.len().to_string())
            .env("HTTP_GIT_PROTOCOL", protocol)
            .env("REMOTE_USER", username)
            .env("REMOTE_ADDR", "127.0.0.1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&body).await.unwrap();
        stdin.shutdown().await.unwrap();
        drop(stdin);
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "Git backend: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let split = output
            .stdout
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&output.stdout[..split]).unwrap();
        let mut response = Response::builder();
        for header in headers.lines() {
            let (key, value) = header.split_once(':').unwrap();
            if key.eq_ignore_ascii_case("status") {
                response = response.status(
                    value
                        .split_whitespace()
                        .next()
                        .unwrap()
                        .parse::<u16>()
                        .unwrap(),
                );
            } else {
                response = response.header(key, value.trim());
            }
        }
        Ok(response
            .body(Full::new(Bytes::copy_from_slice(
                &output.stdout[split + 4..],
            )))
            .unwrap())
    }
}

#[cfg(unix)]
async fn listener() -> TcpListener {
    // Use only the VM's explicitly assigned test ports; every bind is owned by this fixture.
    for port in 41000..=41999 {
        if let Ok(listener) = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await {
            return listener;
        }
    }
    panic!("no owned Git HTTPS fixture port available");
}

#[cfg(unix)]
async fn authentication() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("git"))
        .find(|program| program.is_file())
        .expect("Git is installed in the fixture image");
    let original = fixture.path().join("original");
    std::fs::create_dir(&original).unwrap();
    native(&real, &original, &["init", "-b", "work"]);
    std::fs::write(original.join("code"), b"original\n").unwrap();
    native(&real, &original, &["add", "code"]);
    native(
        &real,
        &original,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@orch.invalid",
            "commit",
            "-m",
            "first",
        ],
    );
    let served = fixture.path().join("served");
    std::fs::create_dir(&served).unwrap();
    for name in ["source.git", "other.git"] {
        native(
            &real,
            &served,
            &["clone", "--bare", original.to_str().unwrap(), name],
        );
    }
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(signing_key.serialize_der().into()),
    )
    .unwrap();
    let listener = listener().await;
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let backend = Backend {
        program: real.clone(),
        root: served.clone(),
        seen: seen.clone(),
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let _server = StopServer(tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let acceptor = acceptor.clone(); let backend = backend.clone();
                    connections.spawn(async move {
                        let Ok(stream) = acceptor.accept(stream).await else { return };
                        let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream),
                            service_fn(move |request| backend.clone().reply(request))).await;
                    });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
    }));
    let ca = fixture.path().join("ca.pem");
    std::fs::write(&ca, cert.pem()).unwrap();
    let global = fixture.path().join("global.gitconfig");
    let helper = fixture.path().join("environment-credential");
    std::fs::write(&helper, format!("#!/bin/sh\n[ \"$1\" = get ] || exit 0\nprintf 'username=environment\\npassword={ENVIRONMENT_PASSWORD}\\n'\n")).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let helper_config = format!("!exec {} \"$@\"", quote(helper.to_str().unwrap()))
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    std::fs::write(
        &global,
        format!("[credential]\nhelper = \"{helper_config}\"\nuseHttpPath = true\n"),
    )
    .unwrap();
    let bin = fixture.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let arguments = fixture.path().join("arguments");
    // This executable changes only owned TLS trust/global auth and records argv. No transport
    // address or native Git operation is substituted; clone and push go through actual HTTPS.
    std::fs::write(bin.join("git"), format!("#!/bin/sh\nexport GIT_SSL_CAINFO={} GIT_CONFIG_GLOBAL={}\nexport HTTP_PROXY= HTTPS_PROXY= ALL_PROXY= http_proxy= https_proxy= all_proxy= NO_PROXY=localhost no_proxy=localhost\nprintf '%s\\0' \"$@\" >> {}\nexec {} \"$@\"\n",
        quote(ca.to_str().unwrap()), quote(global.to_str().unwrap()), quote(arguments.to_str().unwrap()), quote(real.to_str().unwrap()))).unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o700)).unwrap();
    let tool = GitTool::find_in(&OsString::from(&bin));
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let files = Files::workspace(
        Workspace::open(&workspace, &[], PathStyle::Posix, true).unwrap(),
        Arc::new(IgnoreRules::new(&[], true)),
        Arc::new(HashCache::new()),
        Limits::default(),
        Arc::new(GateState::new(Gates::OPEN)),
    );
    let states =
        FileStates::new(ObjectStore::open(&fixture.path().join("kept"), &workspace).unwrap());
    let source_url = format!("https://localhost:{port}/source.git");
    let request = GitPrepare {
        environment_id: new_id(),
        job_id: new_id(),
        directory: "attempt".into(),
        repository_url: source_url.clone(),
        branch: Some("work".into()),
        destination: SourceDestination::MissingDirectory,
        credential: Some(Secret::Ref(new_id())),
    };
    let secret = serde_json::json!({"username":"source","password":SOURCE_PASSWORD}).to_string();
    let ready = git_actions::prepare(&tool, &files, &states, &request, 1, false, Some(&secret))
        .await
        .unwrap();
    assert_eq!(ready.branch.as_deref(), Some("work"));
    assert_eq!(
        std::fs::read(workspace.join("attempt/code")).unwrap(),
        b"original\n"
    );
    let wrong = GitPrepare {
        job_id: new_id(),
        directory: "wrong".into(),
        ..request.clone()
    };
    assert!(
        git_actions::prepare(
            &tool,
            &files,
            &states,
            &wrong,
            1,
            false,
            Some(r#"{"username":"source","password":"wrong"}"#)
        )
        .await
        .is_err()
    );
    assert!(
        !workspace.join("wrong").exists(),
        "failed authentication publishes no directory"
    );
    let redirect = GitPrepare {
        job_id: new_id(),
        directory: "redirect".into(),
        repository_url: format!("https://localhost:{port}/redirect.git"),
        ..request.clone()
    };
    assert!(
        git_actions::prepare(&tool, &files, &states, &redirect, 1, false, Some(&secret))
            .await
            .is_err()
    );
    let checkout = workspace.join("attempt");
    let other_url = format!("https://localhost:{port}/other.git");
    native(&real, &checkout, &["remote", "add", "other", &other_url]);
    let marker = fixture.path().join("repository-helper-ran");
    native(
        &real,
        &checkout,
        &[
            "config",
            "credential.helper",
            &format!("!touch {}", quote(marker.to_str().unwrap())),
        ],
    );
    std::fs::write(checkout.join("code"), b"published\n").unwrap();
    let status = files
        .git_status_at(&tool, Some(&files.parse("attempt").unwrap()))
        .await
        .unwrap();
    let result = git_publication::commit(
        &tool,
        &files,
        &states,
        &GitCommit {
            environment_id: request.environment_id,
            job_id: new_id(),
            path: "attempt".into(),
            paths: vec!["code".into()],
            message: "Reviewed".into(),
            expected_head: status.head,
            expected_status: status.status_hash.unwrap(),
            author: GitAuthor {
                name: "Reviewer".into(),
                email: "reviewer@orch.invalid".into(),
            },
        },
    )
    .await
    .unwrap();
    let pushed = git_publication::push(
        &tool,
        &files,
        &states,
        &GitPush {
            environment_id: request.environment_id,
            job_id: new_id(),
            path: "attempt".into(),
            remote: "other".into(),
            expected_remote_url: other_url,
            branch: "work".into(),
            expected_head: result.head.clone(),
            set_upstream: true,
            credential: None,
            credential_repository_url: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(pushed.status.ahead, Some(0));
    assert!(!marker.exists(), "repository-controlled helpers never run");
    assert_eq!(
        String::from_utf8(native(
            &real,
            &served.join("other.git"),
            &["rev-parse", "refs/heads/work"]
        ))
        .unwrap()
        .trim(),
        result.head
    );
    let seen = seen.lock().unwrap();
    let source_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("source:{SOURCE_PASSWORD}"))
    );
    let environment_auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD
            .encode(format!("environment:{ENVIRONMENT_PASSWORD}"))
    );
    assert!(
        seen.iter()
            .any(|(path, auth)| path.starts_with("/source.git/") && auth == &source_auth)
    );
    assert!(
        seen.iter()
            .any(|(path, auth)| path.starts_with("/other.git/") && auth == &environment_auth)
    );
    assert!(
        !seen
            .iter()
            .any(|(path, auth)| path.starts_with("/other.git/") && auth == &source_auth),
        "redirects and other remotes receive no source secret"
    );
    for password in [SOURCE_PASSWORD, ENVIRONMENT_PASSWORD] {
        assert!(!String::from_utf8_lossy(&std::fs::read(&arguments).unwrap()).contains(password));
        assert!(
            !std::fs::read_to_string(checkout.join(".git/config"))
                .unwrap()
                .contains(password)
        );
        assert!(!serde_json::to_string(&ready).unwrap().contains(password));
    }
    assert!(
        std::fs::read_dir(states.store().root())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("source-credential-"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn authenticated_clone_and_another_remote_push_never_share_the_source_credential() {
    tokio::time::timeout(TEST_TIMEOUT, authentication())
        .await
        .unwrap();
}
