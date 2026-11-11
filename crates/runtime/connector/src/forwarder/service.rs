//! Fixed extension-declared service requests keep credentials and ordinary transport policy here.
use super::*;
mod materialized;
use orch_link_contract::commands::{
    CommandError, CommandKind, ErrorCode, ProviderService, ProviderServiceResult,
};
use orch_model_contract::endpoint::{AuthMode, EndpointOptions};
use orch_provider_contract::capabilities::{Binding, Endpoint, render_request};
use orch_provider_contract::services::{catalogue, definition_hash};
use serde_json::Value;
use std::collections::BTreeMap;

impl Forwarder {
    /// Only a controller-admitted command can reach this transport. Its declaration hash is no Use proof.
    pub async fn service(
        &self,
        provider: &ProviderEntry,
        request: &ProviderService,
        command: Id,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<ProviderServiceResult, CommandError> {
        let result = crate::content::stoppable(
            &cancel,
            u64::from(CommandKind::ProviderService.deadline_secs()),
            async {
                self.service_ready(provider, request, command, &cancel)
                    .await
            },
        )
        .await;
        self.services.content.finish(command).await;
        result
    }

    async fn service_ready(
        &self,
        provider: &ProviderEntry,
        request: &ProviderService,
        command: Id,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<ProviderServiceResult, CommandError> {
        if provider.id() != request.provider_id || cancel.is_cancelled() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the service command no longer names an active provider request",
            ));
        }
        let endpoints = provider
            .endpoints()
            .iter()
            .map(|endpoint| {
                (
                    endpoint.protocol().to_owned(),
                    Endpoint {
                        interface: endpoint.protocol().to_owned(),
                        address: endpoint.base_url().to_string(),
                        ..Endpoint::default()
                    },
                )
            })
            .collect();
        let declarations = self.services.builtin_sources.capabilities().declarations();
        let services = catalogue(declarations, &endpoints, &BTreeMap::new());
        let offering = services
            .offerings
            .get(&request.offering)
            .filter(|offering| {
                offering
                    .capabilities
                    .get(&request.capability)
                    .is_some_and(|support| support.enabled)
            })
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the requested service capability is not declared for this account",
                )
            })?;
        let endpoint = services.endpoints.get(&offering.endpoint).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "the service endpoint is not declared",
            )
        })?;
        let _interface = declarations
            .interfaces
            .get(&endpoint.interface)
            .filter(|interface| {
                orch_provider_contract::services::json_implementation(&interface.implementation)
            })
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "this runtime does not implement the service interface",
                )
            })?;
        let binding = orch_provider_contract::services::service_binding(
            declarations,
            endpoint,
            offering,
            &request.capability,
        )
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "the requested service binding is closed",
            )
        })?;
        if definition_hash(endpoint, binding).ok() != Some(request.definition) {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the service definition changed before execution",
            ));
        }
        let rendered = render_request(binding, &request.input).map_err(|_| {
            CommandError::new(
                ErrorCode::Conflict,
                "the service input is outside its declared schema",
            )
        })?;
        let method = rendered
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("POST")
            .parse::<Method>()
            .map_err(|_| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the declared service method is invalid",
                )
            })?;
        let base = Url::parse(&endpoint.address).map_err(|_| {
            CommandError::new(
                ErrorCode::Unsupported,
                "the declared service address is invalid",
            )
        })?;
        let path = rendered
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the declared service path is missing",
                )
            })?;
        let mut target = base.join(path).map_err(|_| {
            CommandError::new(
                ErrorCode::Unsupported,
                "the declared service path is invalid",
            )
        })?;
        if !matches!(base.scheme(), "http" | "https")
            || target.origin() != base.origin()
            || !target.username().is_empty()
            || target.password().is_some()
            || target.fragment().is_some()
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a service request must stay on its declared credential origin",
            ));
        }
        self.service_address_allowed(&endpoint.address)?;
        let (body, content_type) = self
            .materialize_service_body(&rendered, &request.input_images, command)
            .await?;
        let (options, uses_model_key) = service_authentication(endpoint)?;
        let credential = uses_model_key
            .then(|| provider.credential(&self.store))
            .flatten();
        if uses_model_key && credential.is_none() {
            return Err(CommandError::new(
                ErrorCode::LoginRequired,
                "the service has no model credential",
            ));
        }
        let native_endpoint = EndpointEntry::new(&endpoint.interface, base, options);
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if let Some(declared) = rendered.get("headers").and_then(Value::as_object) {
            for (name, value) in declared {
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                    CommandError::new(ErrorCode::Unsupported, "a service header name is invalid")
                })?;
                let value = value
                    .as_str()
                    .and_then(|value| HeaderValue::from_str(value).ok())
                    .ok_or_else(|| {
                        CommandError::new(
                            ErrorCode::Unsupported,
                            "a service header value is invalid",
                        )
                    })?;
                headers.insert(name, value);
            }
        }
        headers.insert(hyper::header::CONTENT_TYPE, content_type);
        let extra = if uses_model_key && provider.presenter().is_some() {
            self.authorize(
                provider,
                &native_endpoint,
                None,
                &mut headers,
                credential.as_ref(),
            )
            .await
            .map_err(|_| {
                CommandError::new(
                    ErrorCode::LoginRequired,
                    "the service credential could not be presented",
                )
            })?
        } else {
            // Anonymous services never consult an account's key, presenter or secret headers.
            let no_secrets = BTreeMap::new();
            let secrets = if uses_model_key {
                provider.secrets()
            } else {
                &no_secrets
            };
            orch_model_contract::endpoint::authorize(
                &mut headers,
                credential.as_ref(),
                native_endpoint.options(),
                secrets,
                if uses_model_key {
                    AuthMode::Bearer
                } else {
                    AuthMode::None
                },
            )
            .map_err(|_| {
                CommandError::new(
                    ErrorCode::LoginRequired,
                    "the service credential could not be presented",
                )
            })?
        };
        if let Some(query) = rendered.get("query").and_then(Value::as_object) {
            for (name, value) in query {
                let value = value.as_str().ok_or_else(|| {
                    CommandError::new(ErrorCode::Unsupported, "a service query value is invalid")
                })?;
                target.query_pairs_mut().append_pair(name, value);
            }
        }
        // Local policy is checked again after any credential presenter awaited its own work.
        self.service_address_allowed(&endpoint.address)?;
        for (name, value) in extra {
            target.query_pairs_mut().append_pair(&name, &value);
        }
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > self.max_request_bytes {
            return Err(CommandError::new(
                ErrorCode::TooLarge,
                "the service request exceeds this runtime's request budget",
            ));
        }
        if cancel.is_cancelled() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "the service request was stopped before execution",
            ));
        }
        let asked = self.services.clock.time();
        let exchange = self.upstream.fetch(
            UpstreamRequest {
                method,
                url: target,
                headers,
                body: Bytes::from(body),
            },
            orch_link_contract::commands::PROVIDER_SERVICE_RESULT_MAX_BYTES,
        );
        let reply = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            reply = tokio::time::timeout(
                std::time::Duration::from_secs(u64::from(CommandKind::ProviderService.deadline_secs())),
                exchange,
            ) => reply.ok(),
        };
        let (status, bytes) = match reply {
            Some(Ok((status, _, bytes))) => (Some(status), bytes),
            _ => (None, Bytes::new()),
        };
        let parsed: Option<Value> = serde_json::from_slice(&bytes).ok();
        let success = status.is_some_and(|status| allowed_status(binding, status));
        let complete = success && parsed.is_some();
        let counters = binding
            .counters
            .iter()
            .filter(|(_, counter)| !counter.reported_cost)
            .map(|(name, counter)| {
                let quantity = if status.is_some() && !success && counter.successful_only {
                    Some(0)
                } else {
                    parsed.as_ref().and_then(|body| counter.count(body))
                };
                (name.clone(), quantity)
            })
            .collect::<BTreeMap<_, _>>();
        let mut observed = orch_agent_contract::requests::UsageRequest {
            id: "service".into(),
            asked: Some(asked),
            end: Some(self.services.clock.time()),
            upstream_status: status.map(|status| i64::from(status.as_u16())),
            output_streamed: Some(false),
            ..Default::default()
        };
        observed.finish(
            if complete { "done" } else { "harness_error" },
            Some("The declared service request did not complete."),
            None,
        );
        let mut timing = observed.measured();
        timing.declared_counters = counters
            .iter()
            .map(|(name, quantity)| {
                (
                    name.clone(),
                    quantity.map_or(orch_api_contract::usage::UsageMeasurement::Empty, |value| {
                        orch_api_contract::usage::UsageMeasurement::Told { value }
                    }),
                )
            })
            .collect();
        timing.reported_cost = reported_cost(binding, &bytes);
        let mut result = ProviderServiceResult {
            output: parsed.unwrap_or(Value::Null),
            output_blob: None,
            timing,
            counters,
        };
        if !complete {
            return Err(service_failure(
                &result,
                "the declared service did not return a successful JSON result",
            ));
        }
        let encoded = serde_json::to_vec(&result.output)
            .map_err(|_| service_failure(&result, "the service result could not be encoded"))?;
        if encoded.len() > orch_link_contract::version::MAX_FRAME_BYTES as usize / 2 {
            let directory = self.services.content.directory_of(command);
            tokio::fs::create_dir_all(&directory).await.map_err(|_| {
                service_failure(&result, "the service result could not be retained")
            })?;
            let path = directory.join("service-result.json");
            tokio::fs::write(&path, encoded).await.map_err(|_| {
                service_failure(&result, "the service result could not be retained")
            })?;
            result.output_blob = Some(self.services.content.put(command, &path).await.map_err(
                |_| service_failure(&result, "the service result could not be uploaded"),
            )?);
            result.output = Value::Null;
        }
        let frame_bytes = serde_json::to_vec(&result)
            .map_err(|_| service_failure(&result, "the service result could not be encoded"))?;
        if frame_bytes.len() > orch_link_contract::version::MAX_FRAME_BYTES as usize / 2 {
            return Err(service_failure(
                &result,
                "the service result metadata exceeds the command frame budget",
            ));
        }
        Ok(result)
    }

    // Local upstream policy lists exact declared bases, before request paths/query are rendered.
    // The request separately remains bound to that base's credential origin.
    fn service_address_allowed(&self, declared_base: &str) -> Result<(), CommandError> {
        if self
            .services
            .policy
            .policy()
            .is_some_and(|policy| policy.check_upstream(declared_base).is_err())
        {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "provider.upstreams does not allow this service address",
            ));
        }
        Ok(())
    }
}

fn service_authentication(endpoint: &Endpoint) -> Result<(EndpointOptions, bool), CommandError> {
    let uses_model_key = match endpoint.credential_role.as_deref() {
        None => false,
        Some("model") => true,
        Some(_) => {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "the service requests an unavailable credential role",
            ));
        }
    };
    let default_auth = if uses_model_key {
        AuthMode::Bearer
    } else {
        AuthMode::None
    };
    let options = EndpointOptions::parse(&endpoint.options).map_err(|_| {
        CommandError::new(
            ErrorCode::Unsupported,
            "the service endpoint options are invalid",
        )
    })?;
    options.check(default_auth).map_err(|_| {
        CommandError::new(
            ErrorCode::Unsupported,
            "the service endpoint authentication is invalid",
        )
    })?;
    if !uses_model_key
        && (options.auth.unwrap_or(default_auth) != AuthMode::None
            || !options.secret_headers.is_empty())
    {
        return Err(CommandError::new(
            ErrorCode::Unsupported,
            "an anonymous service cannot request account credentials",
        ));
    }
    Ok((options, uses_model_key))
}

fn allowed_status(binding: &Binding, status: StatusCode) -> bool {
    binding
        .request
        .get("success_statuses")
        .and_then(Value::as_array)
        .map_or_else(
            || status.is_success(),
            |allowed| {
                allowed
                    .iter()
                    .any(|value| value.as_u64() == Some(u64::from(status.as_u16())))
            },
        )
}

fn reported_cost(
    binding: &Binding,
    body: &[u8],
) -> orch_api_contract::usage::UsageMeasurement<orch_api_contract::usage::ReportedCost> {
    use orch_api_contract::usage::{ReportedCost, UsageMeasurement};
    let mut declarations = binding
        .counters
        .values()
        .filter(|counter| counter.reported_cost);
    let Some(counter) = declarations.next() else {
        return UsageMeasurement::Unsupported;
    };
    if declarations.next().is_some() {
        return UsageMeasurement::Empty;
    }
    let amount = std::str::from_utf8(body)
        .ok()
        .and_then(|body| orch_model_contract::declared_usage::raw_pointer(body, &counter.pointer))
        .and_then(|raw| {
            if raw.get().starts_with('"') {
                serde_json::from_str::<String>(raw.get())
                    .ok()
                    .filter(|amount| orch_provider_contract::limits::is_decimal(amount))
            } else {
                orch_provider_contract::limits::decimal_number(raw.get())
            }
        })
        .filter(|amount| !amount.starts_with('-'));
    amount.map_or(UsageMeasurement::Empty, |amount| UsageMeasurement::Told {
        value: ReportedCost {
            amount,
            unit: (!counter.unit.is_empty()).then(|| counter.unit.clone()),
        },
    })
}

fn service_failure(result: &ProviderServiceResult, message: &str) -> CommandError {
    let mut measured = result.clone();
    measured.output = Value::Null;
    measured.output_blob = None;
    measured.timing.outcome = orch_api_contract::usage::UsageMeasurement::Told {
        value: "harness_error".into(),
    };
    CommandError::new(ErrorCode::Internal, message)
        .with_details(serde_json::json!({"service_result":measured}))
}
