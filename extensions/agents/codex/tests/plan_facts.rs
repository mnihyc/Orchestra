//! Provider plan observations, access-token imports and confirmed resets with a scripted app server and the pinned harness.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::Rig;
use common::fake::FakeLauncher;
use orch_adapter_codex::CodexAdapter;
use orch_adapter_codex::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use orch_adapter_codex::provider::{self, AccessTokenKind, LoginTracker};
use orch_agent_contract::adapter::{AgentAdapter, AgentStart, EventSink, EventStream};
use orch_agent_contract::facts::{AgentProfile, CredentialYield, ResetRequest};
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use orch_types::{Clock, Json, ManualClock, SystemClock, Time, new_id};
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;

fn now() -> Time {
    "2026-10-13T00:00:00.000Z".parse().unwrap()
}

fn token(chatgpt: bool, issued: i64, expires: i64) -> String {
    let mut claims = json!({ "sub":"made-up-agent", "iat":issued, "exp":expires, "email":"ada@example.invalid" });
    if chatgpt {
        claims["https://api.openai.com/auth"] =
            json!({"chatgpt_account_id":"account-plan", "chatgpt_plan_type":"plus"});
    }
    format!(
        "{}.{}.ZmFrZQ",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

fn login(token: &str) -> LoginRequest {
    LoginRequest {
        method: "access_token".into(),
        values: serde_json::from_value::<LoginValues>(json!({"access_token":token})).unwrap(),
        config: json!({}),
    }
}

#[test]
fn access_tokens_are_told_apart_and_their_own_lifetime_is_kept() {
    let issued = now().get().timestamp();
    let end = issued + 10 * 24 * 60 * 60;
    for (chatgpt, kind) in [
        (true, AccessTokenKind::Chatgpt),
        (false, AccessTokenKind::Workspace),
    ] {
        let info = provider::check_access_token(&token(chatgpt, issued, end), now()).unwrap();
        assert_eq!(info.kind, kind);
        assert_eq!(info.issued_at, Some(now()));
        assert_eq!(info.expires_at.unwrap().get().timestamp(), end);
    }
    let info = provider::check_access_token("at-scripted-workspace-token", now()).unwrap();
    assert_eq!(info.kind, AccessTokenKind::Workspace);
    assert_eq!(info.issued_at, None);
    assert_eq!(info.expires_at, None);
}

#[test]
fn a_token_that_is_unknown_or_expired_is_refused_without_its_text() {
    let expired = token(
        true,
        now().get().timestamp() - 864000,
        now().get().timestamp(),
    );
    for bad in ["secret-that-is-not-a-token".to_owned(), expired] {
        let error = provider::check_access_token(&bad, now()).unwrap_err();
        assert!(!error.to_string().contains(&bad));
        assert_eq!(error.code(), "login_refused");
        assert!(error.detail().contains("neither") || error.detail().contains("already lapsed"));
    }
}

#[test]
fn a_window_that_has_not_started_has_no_sliding_reset_time() {
    let tracker = LoginTracker::default();
    let snapshot = |used, reset| json!({"primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":reset}});
    let reset = now().get().timestamp() + 300 * 60;
    for seconds in [reset - 60, reset, reset + 60] {
        let facts = provider::facts_from_snapshot(&snapshot(0, seconds), &tracker, now());
        assert!(facts.windows[0].resets_at.is_none());
        assert_eq!(facts.windows[0].reset_not_started, Some(true));
    }
    let later = Time::new(now().get() + chrono::Duration::minutes(10));
    let sliding = provider::facts_from_snapshot(&snapshot(0, reset + 600), &tracker, later);
    assert!(sliding.windows[0].resets_at.is_none());
    assert_eq!(sliding.windows[0].reset_not_started, Some(true));
    let begun = provider::facts_from_snapshot(&snapshot(5, reset), &tracker, later);
    assert_eq!(begun.windows[0].resets_at.unwrap().get().timestamp(), reset);
    assert_eq!(begun.windows[0].reset_not_started, Some(false));
    assert!(
        provider::facts_from_snapshot(&snapshot(1, reset), &tracker, now()).windows[0]
            .resets_at
            .is_some()
    );
    assert!(
        provider::facts_from_snapshot(&snapshot(0, reset - 61), &tracker, now()).windows[0]
            .resets_at
            .is_some()
    );
}

struct Capture(Arc<Mutex<Vec<u8>>>);
impl AsyncWrite for Capture {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
struct Ended;
#[async_trait]
impl ProcessControl for Ended {
    async fn wait(&mut self) -> ProcessExit {
        ProcessExit {
            code: Some(0),
            detail: String::new(),
        }
    }
    fn kill(&mut self) {}
}
struct Commands {
    harness: FakeLauncher,
    input: Arc<Mutex<Vec<u8>>>,
    commands: Mutex<Vec<LaunchRequest>>,
}
#[async_trait]
impl Launcher for Commands {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        if request.args.first().is_some_and(|arg| arg == "login") {
            self.commands.lock().unwrap().push(request);
            return Ok(HarnessProcess {
                stdin: Box::new(Capture(Arc::clone(&self.input))),
                stdout: Box::new(std::io::Cursor::new(Vec::<u8>::new())),
                stderr: None,
                control: Box::new(Ended),
            });
        }
        self.harness.launch(request).await
    }
}

async fn boot(launcher: Arc<dyn Launcher>) -> (CodexAdapter, tempfile::TempDir, EventStream) {
    boot_at(launcher, Arc::new(SystemClock)).await
}

async fn boot_at(
    launcher: Arc<dyn Launcher>,
    clock: Arc<dyn Clock>,
) -> (CodexAdapter, tempfile::TempDir, EventStream) {
    let mut adapter = CodexAdapter::with_launcher(launcher).with_clock(clock);
    let home = tempfile::tempdir().unwrap();
    let (sink, events) = EventSink::channel();
    adapter
        .start(
            AgentStart {
                provider_id: new_id(),
                agent_id: new_id(),
                harness_kind: "codex".into(),
                profile: AgentProfile::default(),
                home: Some(home.path().to_string_lossy().into()),
                provider_config: json!({}),
                loopback: Loopback::new(41999, Secret::new("made-up")),
                binary: None,
            },
            sink,
        )
        .await
        .unwrap();
    (adapter, home, events)
}

#[tokio::test]
async fn a_workspace_token_goes_only_on_the_harness_commands_standard_input() {
    let current = SystemClock.time().get().timestamp();
    for (secret, expires) in [
        (
            token(false, current - 60, current + 864000),
            Some(current + 864000),
        ),
        ("at-scripted-workspace-token".to_owned(), None),
    ] {
        let launcher = Arc::new(Commands {
            harness: FakeLauncher::new(|method, _| match method {
                "getAuthStatus" => Ok(json!({"authMethod":"chatgpt"})),
                _ => Ok(json!({})),
            }),
            input: Arc::default(),
            commands: Mutex::default(),
        });
        let (mut adapter, home, _events) = boot(launcher.clone()).await;
        adapter.login(login(&secret)).await.unwrap();
        assert_eq!(
            launcher.input.lock().unwrap().as_slice(),
            format!("{secret}\n").as_bytes()
        );
        {
            let commands = launcher.commands.lock().unwrap();
            assert_eq!(commands.len(), 1);
            assert!(
                commands[0]
                    .args
                    .iter()
                    .any(|arg| arg == "--with-access-token")
            );
            assert!(!format!("{:?}", commands[0].args).contains(&secret));
        }
        assert_eq!(
            provider::read_login_note(home.path())
                .1
                .map(|time| time.get().timestamp()),
            expires
        );
        adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn a_partial_vendor_read_keeps_untold_windows_and_credits_dated_and_does_not_reauthorize_a_cached_reset()
 {
    let shape = Arc::new(Mutex::new(false));
    let current = shape.clone();
    let harness = Arc::new(FakeLauncher::new(move |method, _| match method {
        "getAuthStatus" => Ok(json!({"authMethod":"chatgpt"})),
        "account/read" => Ok(json!({"account":{"planType":"plus"}})),
        "account/rateLimits/read" if *current.lock().unwrap() => {
            Ok(json!({"rateLimits":{"planType":"team"}}))
        }
        "account/rateLimits/read" => Ok(json!({
            "rateLimits":{"planType":"plus","primary":{"usedPercent":20,"windowDurationMins":300},"credits":{"balance":"4","unlimited":false}},
            "rateLimitResetCredits":{"availableCount":3,"credits":null},"ordinaryUsageAllowed":false
        })),
        _ => Ok(json!({})),
    }));
    let clock = Arc::new(ManualClock::new(now().get()));
    let (mut adapter, _home, _events) = boot_at(harness, clock.clone()).await;
    let before = adapter.provider_facts().await.unwrap();
    clock.advance(chrono::Duration::minutes(5));
    *shape.lock().unwrap() = true;
    let after = adapter.provider_facts().await.unwrap();
    assert_eq!(after.plan.as_deref(), Some("team"));
    assert_eq!(after.field_times["plan"], clock.time());
    assert_eq!(after.windows, before.windows);
    assert_eq!(after.plan_credits, before.plan_credits);
    assert!(after.available_resets.is_none());
    assert!(!after.field_times.contains_key("available_resets"));
    assert_eq!(after.ordinary_usage_allowed, before.ordinary_usage_allowed);
    for field in [
        "windows",
        "windows.primary",
        "plan_credits",
        "ordinary_usage_allowed",
    ] {
        assert_eq!(after.field_times[field], before.field_times[field]);
    }
    assert_eq!(after.credential_verified, Some(true));
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_refusal_or_an_unknown_usage_shape_never_replaces_known_facts() {
    let shape = Arc::new(Mutex::new(0));
    let current = shape.clone();
    let harness = Arc::new(FakeLauncher::new(move |method, _| match method {
        "getAuthStatus" => Ok(json!({"authMethod":"chatgpt"})),
        "account/read" => Ok(json!({"account":{"planType":"plus"}})),
        "account/rateLimits/read" => match *current.lock().unwrap() {
            0 => Ok(
                json!({"rateLimits":{"planType":"plus","primary":{"usedPercent":20,"windowDurationMins":300}},"rateLimitResetCredits":{"availableCount":3,"credits":null}}),
            ),
            1 => Err((401, "refused".into())),
            _ => Ok(json!({"newVendorShape":{"usage":1}})),
        },
        _ => Ok(json!({})),
    }));
    let (mut adapter, _home, _events) = boot(harness).await;
    let facts = adapter.provider_facts().await.unwrap();
    assert_eq!(facts.credential_verified, Some(true));
    assert_eq!(facts.available_resets.unwrap().credits, None);
    for unknown in [1, 2] {
        *shape.lock().unwrap() = unknown;
        assert_eq!(
            adapter.provider_facts().await.unwrap_err().code(),
            "facts_not_told"
        );
    }
    *shape.lock().unwrap() = 0;
    assert_eq!(
        adapter.provider_facts().await.unwrap().windows[0].used_percent,
        20.0
    );
    adapter.stop().await.unwrap();
}

#[tokio::test]
async fn each_vendor_reset_outcome_is_returned_and_the_same_attempt_reuses_its_key() {
    for outcome in ["reset", "nothingToReset", "noCredit", "alreadyRedeemed"] {
        let harness = Arc::new(FakeLauncher::new(move |method, _| match method {
            "getAuthStatus" => Ok(json!({"authMethod":"chatgpt"})),
            "account/read" => Ok(json!({"account":{"planType":"plus"}})),
            "account/rateLimits/read" => Ok(
                json!({"rateLimits":{"planType":"plus","primary":{"usedPercent":20,"windowDurationMins":300}},"rateLimitResetCredits":{"availableCount":3,"credits":[{"id":"credit-1","resetType":"codexRateLimits","status":"available","grantedAt":1791849600,"expiresAt":1794441600,"title":"Full reset"}]}}),
            ),
            "account/rateLimitResetCredit/consume" => Ok(json!({"outcome":outcome})),
            _ => Ok(json!({})),
        }));
        let (mut adapter, _home, _events) = boot(harness.clone()).await;
        let request = ResetRequest {
            reset_id: "credit-1".into(),
            idempotency_key: "orch-attempt-1".into(),
        };
        for _ in 0..2 {
            let answered = adapter.consume_reset(request.clone()).await.unwrap();
            assert_eq!(answered.outcome, outcome);
            assert_eq!(answered.facts.available_resets.unwrap().available_count, 3);
        }
        let asked = harness.state.calls("account/rateLimitResetCredit/consume");
        assert_eq!(asked[0]["params"], asked[1]["params"]);
        assert_eq!(asked[0]["params"]["creditId"], "credit-1");
        adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn the_real_harness_accepts_a_chatgpt_access_token_without_renewal_and_names_its_plan() {
    assert!(
        common::image_present(),
        "the pinned image is required for this acceptance check"
    );
    let mut rig = Rig::start_with("plan-access", |launcher| launcher.without_network())
        .await
        .unwrap();
    rig.boot().await;
    let current = SystemClock.time().get().timestamp();
    let secret = token(true, current - 60, current + 864000);
    rig.adapter.login(login(&secret)).await.unwrap();
    let auth: Json =
        serde_json::from_str(&std::fs::read_to_string(rig.home_path().join("auth.json")).unwrap())
            .unwrap();
    assert_eq!(auth["tokens"]["refresh_token"], "");
    assert_eq!(auth["tokens"]["id_token"], auth["tokens"]["access_token"]);
    let facts = rig.adapter.provider_facts().await.unwrap();
    assert_eq!(facts.plan.as_deref(), Some("plus"));
    let credential = facts.credential.unwrap();
    assert_eq!(credential.yields, CredentialYield::Token);
    assert_eq!(
        credential.issued_at.unwrap().get().timestamp(),
        current - 60
    );
    assert_eq!(
        credential.expires_at.unwrap().get().timestamp(),
        current + 864000
    );
    let launcher = rig.launcher.clone();
    let mut program = launcher
        .launch(LaunchRequest {
            program: "codex".into(),
            args: vec!["login".into(), "status".into()],
            env: std::collections::BTreeMap::from([(
                "CODEX_HOME".into(),
                rig.home_path().to_string_lossy().into(),
            )]),
            cwd: None,
        })
        .await
        .unwrap();
    let mut status = String::new();
    use tokio::io::AsyncReadExt as _;
    program.stdout.read_to_string(&mut status).await.unwrap();
    if let Some(stderr) = program.stderr.as_mut() {
        stderr.read_to_string(&mut status).await.unwrap();
    }
    assert_eq!(program.control.wait().await.code, Some(0));
    assert!(status.contains("Logged in using ChatGPT"));
    assert!(!status.contains(&secret));
    let mut app = launcher
        .launch(LaunchRequest {
            program: "node".into(),
            args: vec!["-e".into(), r#"
                const http = require('http'), cp = require('child_process');
                const server = http.createServer((request,response) => {
                    response.setHeader('Content-Type','application/json');
                    response.end(JSON.stringify({accounts:[{id:'account-plan',plan_type:'plus',workspace_backend_origin:'https://chatgpt.com',account_routing_override:'NO_CONSTRAINT',name:'Test',structure:'personal'}],account_ordering:['account-plan'],default_account_id:'account-plan'}));
                });
                server.listen(41439,'127.0.0.1',() => {
                    const child = cp.spawn('codex',['app-server','-c','chatgpt_base_url="http://127.0.0.1:41439/backend-api/"'],{stdio:'inherit'});
                    child.on('exit',code => {server.close();process.exit(code || 0);});
                });
            "#.into()],
            env: std::collections::BTreeMap::from([(
                "CODEX_HOME".into(),
                rig.home_path().to_string_lossy().into(),
            )]),
            cwd: None,
        })
        .await
        .unwrap();
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
    let mut replies = BufReader::new(&mut app.stdout).lines();
    app.stdin.write_all(format!("{}\n",json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"orch-plan-check","version":"1"}}})).as_bytes()).await.unwrap();
    tokio::time::timeout(common::WAIT, async {
        loop {
            let frame: Json =
                serde_json::from_str(&replies.next_line().await.unwrap().unwrap()).unwrap();
            if frame["id"] == 1 {
                assert!(frame.get("result").is_some());
                break;
            }
        }
    })
    .await
    .unwrap();
    app.stdin
        .write_all(
            format!(
                "{}\n{}\n",
                json!({"method":"initialized"}),
                json!({"id":2,"method":"account/read","params":{"refreshToken":false}})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let account = tokio::time::timeout(common::WAIT, async {
        loop {
            let frame: Json =
                serde_json::from_str(&replies.next_line().await.unwrap().unwrap()).unwrap();
            if frame["id"] == 2 {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        account["result"]["account"]["type"], "chatgpt",
        "account metadata: {account}"
    );
    assert_eq!(account["result"]["account"]["planType"], "plus");
    app.control.kill();
    app.control.wait().await;
    rig.adapter.stop().await.unwrap();
}
