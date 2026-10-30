//! The actual address example passes the same provider traits through the connector's production HTTP client.

use super::{AT, CANARY, ask, facts, fixture};
use crate::{PYTHON, example};
use async_trait::async_trait;
use orch_conformance::provider::{self, Sources};
use orch_conformance::report::{Outcome, Report};
use orch_connector_core::sources::address::{AddressCall, AddressClient, OpenAuthorization};
use orch_connector_core::upstream::Upstream;
use orch_link_contract::policy::LocalPolicy;
use orch_provider_contract::source::{
    FactsReport, FactsSource, ModelsReport, ModelsSource, ResetReport, ResetRequest, SourceAsk,
    SourceDescriptor, SourceError, SourceErrorCode,
};
use orch_types::{Json, SystemClock};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

// Process startup is separate from a source method's 8/30-second deadline. The ready pipe is causal.
const START_WITHIN: Duration = Duration::from_secs(30);
const TOKEN: &str = "address-suite-token-kept-out-of-errors";

// This adapter delegates every network operation to the real connector client. It contains no
// alternate HTTP/auth/redirect/bounds logic; the traits let the same suite exercise that binding.
struct Address {
    client: AddressClient,
    url: String,
    policy: LocalPolicy,
    descriptor: SourceDescriptor,
}

impl Address {
    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Json,
    ) -> Result<T, SourceError> {
        let secrets = vec![TOKEN.to_owned(), CANARY.to_owned()];
        let reply = self
            .client
            .call(AddressCall {
                url: &self.url,
                authorization: OpenAuthorization::Bearer(TOKEN),
                method,
                params: &params,
                allow_http: true,
                local_policy: Some(&self.policy),
                secrets: &secrets,
            })
            .await?;
        serde_json::from_value(reply).map_err(|_| {
            SourceError::new(
                SourceErrorCode::Malformed,
                "the controlled address report has another shape",
            )
        })
    }
}

fn sent(value: &impl Serialize) -> Json {
    serde_json::to_value(value).unwrap()
}

#[async_trait]
impl FactsSource for Address {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        self.call("facts.read", sent(&ask)).await
    }
    async fn spend_reset(
        &self,
        ask: SourceAsk,
        reset: ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        let mut params = sent(&ask);
        params
            .as_object_mut()
            .unwrap()
            .extend(sent(&reset).as_object().unwrap().clone());
        self.call("resets.spend", params).await
    }
}

#[async_trait]
impl ModelsSource for Address {
    fn describe(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }
    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        self.call("models.list", sent(&ask)).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bearer_address_example_passes_the_same_suite_and_rejects_wrong_auth_and_unpermitted_http()
 {
    let directory = tempfile::tempdir().unwrap();
    let token = directory.path().join("token");
    let ready = directory.path().join("ready.json");
    std::fs::write(&token, TOKEN).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let stderr = std::fs::File::create(directory.path().join("server.log")).unwrap();
    let mut process = tokio::process::Command::new(PYTHON)
        .arg(example("provider", "address.py"))
        .arg("--state-dir")
        .arg(directory.path().join("account"))
        .arg("--token-file")
        .arg(&token)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--port")
        .arg("0")
        .arg("--at")
        .arg(AT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut signals = BufReader::new(process.stdout.take().unwrap()).lines();
    let signal = tokio::time::timeout(START_WITHIN, signals.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let signal: Json = serde_json::from_str(&signal).unwrap();
    let published: Json = serde_json::from_slice(&std::fs::read(&ready).unwrap()).unwrap();
    assert_eq!(signal, published);
    let url = published["url"].as_str().unwrap().to_owned();
    assert!(!signal.to_string().contains(TOKEN));
    let client = AddressClient::new(Upstream::new(Arc::new(SystemClock)).unwrap());
    let params = json!({});
    let secrets = vec![TOKEN.into(), CANARY.into()];
    let denied = client
        .call(AddressCall {
            url: &url,
            authorization: OpenAuthorization::Bearer(TOKEN),
            method: "describe",
            params: &params,
            allow_http: false,
            local_policy: None,
            secrets: &secrets,
        })
        .await
        .unwrap_err();
    assert_eq!(denied.code, SourceErrorCode::Refused);
    let local = LocalPolicy::default();
    let denied = client
        .call(AddressCall {
            url: &url,
            authorization: OpenAuthorization::Bearer(TOKEN),
            method: "describe",
            params: &params,
            allow_http: true,
            local_policy: Some(&local),
            secrets: &secrets,
        })
        .await
        .unwrap_err();
    assert_eq!(denied.code, SourceErrorCode::UpstreamNotAllowed);
    let policy = LocalPolicy {
        provider_upstreams: vec![url.clone()],
        ..Default::default()
    };
    let wrong = client
        .call(AddressCall {
            url: &url,
            authorization: OpenAuthorization::Bearer("wrong-disposable-token"),
            method: "describe",
            params: &params,
            allow_http: true,
            local_policy: Some(&policy),
            secrets: &secrets,
        })
        .await
        .unwrap_err();
    assert_eq!(wrong.code, SourceErrorCode::Unauthorized);
    assert!(!wrong.to_string().contains(TOKEN));
    let descriptor = client
        .call(AddressCall {
            url: &url,
            authorization: OpenAuthorization::Bearer(TOKEN),
            method: "describe",
            params: &params,
            allow_http: true,
            local_policy: Some(&policy),
            secrets: &secrets,
        })
        .await
        .unwrap();
    let descriptor: SourceDescriptor = serde_json::from_value(descriptor).unwrap();
    assert!(descriptor.presents.is_none());
    let address = Address {
        client,
        url,
        policy,
        descriptor,
    };
    let mut report = Report::new("Acme, actual bearer address");
    let mut controlled = fixture(false);
    controlled.secrets.push(TOKEN.into());
    provider::run(
        Sources {
            descriptor: &address.descriptor,
            facts: Some(&address),
            models: Some(&address),
            presenter: None,
        },
        Some(&controlled),
        &mut report,
    )
    .await;
    assert!(report.passed(), "{report}");
    for rule in [
        "provider.descriptor",
        "provider.facts.shape",
        "provider.facts.states",
        "provider.models",
        "provider.resets.idempotent",
        "provider.quiet",
        "provider.bounds",
    ] {
        assert_eq!(report.outcome(rule), Some(&Outcome::Passed), "{report}");
    }
    assert!(
        matches!(
            report.outcome("provider.credential.present"),
            Some(Outcome::Skipped(_))
        ),
        "an address never claims presentation"
    );
    assert_eq!(
        serde_json::to_value(address.read_facts(ask("normal")).await.unwrap()).unwrap(),
        facts(true, false)
    );
    assert!(!report.to_string().contains(TOKEN));
    assert!(!report.to_string().contains(CANARY));
    process.kill().await.unwrap();
    process.wait().await.unwrap();
}
