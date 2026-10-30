//! The CLI's provider address checks through the connector's bounded and authorized address client.
//!
//! Network requests, authorization, declarations, policy, bounds and redaction belong to the connector's
//! production AddressSource. The command supplies one operator-selected address and a disposable fixture.

use std::sync::Arc;

use orch_conformance::{provider, report::Report};
use orch_connector_core::policy::PolicyStore;
use orch_connector_core::sources::address::{AddressClient, AddressOptions, AddressSource};
use orch_connector_core::upstream::Upstream;
use orch_plugin_contract::{PluginKind, Secret};
use orch_provider_contract::address::SourceAddress;
use orch_provider_contract::source::{FactsSource, ModelsSource};
use orch_types::{Clock, SystemClock};

use super::CheckArgs;

/// Runs the shared source suite against the operator's selected address and explicit fixture.
pub(super) async fn checked(args: &CheckArgs, named: Option<PluginKind>) -> Result<Report, String> {
    if named.is_some_and(|kind| kind != PluginKind::Provider) {
        return Err(
            "an address implements the provider source contract; use --kind provider".to_owned(),
        );
    }
    if args.transport_only {
        return Err("--transport-only checks a program's plugin transport; an address uses provider methods".to_owned());
    }
    let token = args
        .token_file
        .as_deref()
        .map(|file| {
            let secret = orch_config::read_secret(file).map_err(|error| error.to_string())?;
            let token = std::str::from_utf8(secret.expose())
                .map_err(|_| "the address token is not UTF-8".to_owned())?;
            Ok::<_, String>(Secret::new(token))
        })
        .transpose()?;
    let address = SourceAddress {
        url: args
            .address
            .clone()
            .ok_or("a provider address check needs --address URL")?,
        auth: if token.is_none() {
            orch_provider_contract::AddressAuth::None
        } else if args.auth_header.is_some() {
            orch_provider_contract::AddressAuth::Header
        } else {
            orch_provider_contract::AddressAuth::Bearer
        },
        header: args.auth_header.clone(),
        send_credential: args.send_credential,
    };
    let url = address
        .check(args.allow_http)
        .map_err(|_| "the source address, authorization or scheme is not allowed".to_owned())?;
    let mut fixture = super::provider_fixture(args)?;
    fixture
        .secrets
        .extend(token.iter().map(|token| token.expose().to_owned()));
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let client = AddressClient::new(
        Upstream::new(Arc::clone(&clock))
            .map_err(|_| "the address client could not start".to_owned())?,
    );
    // The operator explicitly named this one address for the check; it is the only permitted upstream.
    let mut config = orch_config::ConnectorConfig::default();
    config.provider.upstreams = vec![url.as_str().to_owned()];
    config.policy.local_keys = Some(vec!["provider.upstreams".into()]);
    let source = AddressSource::connect(
        client,
        AddressOptions {
            address,
            authorization: token,
            allow_http: args.allow_http,
            policy: Arc::new(
                PolicyStore::connected(config, None).map_err(|error| error.to_string())?,
            ),
            clock,
            known_secrets: fixture.secrets.clone(),
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let descriptor = FactsSource::describe(&source);
    let mut report = Report::new(format!("provider address {url}"));
    provider::run(
        provider::Sources {
            descriptor: &descriptor,
            facts: descriptor
                .facts
                .is_some()
                .then_some(&source as &dyn FactsSource),
            models: descriptor
                .models
                .is_some()
                .then_some(&source as &dyn ModelsSource),
            presenter: None,
        },
        Some(&fixture),
        &mut report,
    )
    .await;
    Ok(report)
}
