//! The owned streaming exchange uses the forwarder's ordinary authorization path.

use super::*;
use orch_link_contract::commands::{
    CommandError, CommandKind, ErrorCode, ProviderGenerate, ProviderGenerateResult,
};
use orch_model_contract::generation::{GenerationDelta, GenerationOutput};
use serde_json::Value as Json;

pub(super) struct OwnReply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub usage: Option<ObservedUsage>,
    pub streamed: bool,
    pub first_output: Option<orch_types::Time>,
}

impl Forwarder {
    /// One tool-capable step. The provider's key stays in this runtime, and no
    /// unfinished function arguments are published or returned to the host.
    pub async fn generate(
        &self,
        provider: &ProviderEntry,
        request: &ProviderGenerate,
        command_id: Id,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<ProviderGenerateResult, CommandError> {
        let mut request = request.clone();
        let auxiliary = self.native_auxiliary(provider, &request)?;
        if let Some(endpoint) = provider.endpoint_for(&request.protocol) {
            request.model_parameters = self.declared_parameters(
                &request.protocol,
                endpoint,
                &request.model_name,
                &request.model_parameters,
            );
        }
        let result = crate::content::stoppable(&cancel, 300, async {
            let entry = self.protocols.get(&request.protocol).ok_or_else(|| {
                CommandError::new(ErrorCode::Unsupported, "generation interface unavailable")
            })?;
            let endpoint = provider.endpoint_for(&request.protocol).ok_or_else(|| {
                CommandError::new(ErrorCode::Unsupported, "generation endpoint unavailable")
            })?;
            materialize_images(
                &self.services.content,
                entry.protocol.as_ref(),
                endpoint.options(),
                auxiliary.as_ref(),
                &mut request,
                command_id,
                self.max_request_bytes,
            )
            .await?;
            if cancel.is_cancelled() {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "generation was stopped",
                ));
            }
            self.generate_ready(
                provider,
                &request,
                command_id,
                cancel.clone(),
                auxiliary.as_ref(),
            )
            .await
        })
        .await;
        self.services.content.finish(command_id).await;
        result
    }

    async fn generate_ready(
        &self,
        provider: &ProviderEntry,
        request: &ProviderGenerate,
        command_id: Id,
        cancel: tokio_util::sync::CancellationToken,
        auxiliary: Option<&Json>,
    ) -> Result<ProviderGenerateResult, CommandError> {
        if request.input.max_output_tokens <= 0 || request.model_name.trim().is_empty() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a model and positive output limit are required",
            ));
        }
        let call = OwnCall {
            capture_native_result: request.capture_native_result,
            provider,
            protocol: &request.protocol,
            model: &request.model_name,
            label: "generation",
            kind: CommandKind::ProviderGenerate,
            progress: Some(Arc::new(std::sync::Mutex::new(OwnProgress::default()))),
            cancel: Some(cancel),
        };
        let (mut output, usage, mut timing) = self
            .own_request(
                &call,
                |protocol, options| {
                    build_generation_with_auxiliary(protocol, options, request, auxiliary)
                },
                |_, body| read_generation(body, request.capture_native_result),
                Some(command_id),
            )
            .await?;
        if output.stop == orch_model_contract::generation::GenerationStop::Refused {
            timing.outcome = orch_api_contract::usage::UsageMeasurement::Told {
                value: "model_refused".into(),
            };
        }
        if !request.capture_native_result {
            output.native_result = serde_json::Value::Null;
        }
        if let Some(native) = &request.native_capability
            && let Some(binding) = request
                .model_parameters
                .native_tools
                .get(&request.protocol)
                .and_then(|tools| tools.get(&native.capability))
                .and_then(Option::as_ref)
        {
            let raw = output
                .native_result
                .get("response")
                .unwrap_or(&output.native_result);
            for (name, counter) in &binding.counters {
                if counter.items.is_some() && !counter.reported_cost {
                    timing.declared_counters.insert(
                        name.clone(),
                        counter
                            .count(raw)
                            .map_or(orch_api_contract::usage::UsageMeasurement::Empty, |value| {
                                orch_api_contract::usage::UsageMeasurement::Told { value }
                            }),
                    );
                }
            }
        }
        let failed_capture = |error| {
            measured_failure(
                error,
                request,
                usage.tokens,
                &timing,
                usage.request_ref.as_deref(),
            )
        };
        output.validate().map_err(|_| {
            failed_capture(CommandError::new(
                ErrorCode::Internal,
                "the model returned invalid tool calls",
            ))
        })?;
        let mut native_result_blob = None;
        if request.capture_native_result && !output.native_result.is_null() {
            let bytes = serde_json::to_vec(&output.native_result).map_err(|_| {
                failed_capture(CommandError::new(
                    ErrorCode::Internal,
                    "the captured native result could not be encoded",
                ))
            })?;
            if bytes.len() > orch_link_contract::version::MAX_FRAME_BYTES as usize / 2 {
                let directory = self.services.content.directory_of(command_id);
                tokio::fs::create_dir_all(&directory).await.map_err(|_| {
                    failed_capture(CommandError::new(
                        ErrorCode::Internal,
                        "the captured native result could not be retained",
                    ))
                })?;
                let path = directory.join("native-result.json");
                tokio::fs::write(&path, bytes).await.map_err(|_| {
                    failed_capture(CommandError::new(
                        ErrorCode::Internal,
                        "the captured native result could not be retained",
                    ))
                })?;
                native_result_blob = Some(
                    self.services
                        .content
                        .put(command_id, &path)
                        .await
                        .map_err(|_| {
                            failed_capture(CommandError::new(
                                ErrorCode::Internal,
                                "the captured native result could not be uploaded",
                            ))
                        })?,
                );
                output.native_result = serde_json::Value::Null;
            }
        }
        let answer = ProviderGenerateResult {
            native_result_blob,
            output,
            usage: usage.tokens,
            timing,
            request_ref: usage.request_ref,
        };
        let encoded_failure = |code, detail: &str| {
            measured_failure(
                CommandError::new(code, detail),
                request,
                answer.usage,
                &answer.timing,
                answer.request_ref.as_deref(),
            )
        };
        let frame = orch_link_contract::frames::Frame::from(
            orch_link_contract::commands::CmdRes::ok(command_id, &answer).map_err(|_| {
                encoded_failure(
                    ErrorCode::Internal,
                    "generation result could not be encoded",
                )
            })?,
        );
        if serde_json::to_vec(&frame)
            .map_err(|_| {
                encoded_failure(
                    ErrorCode::Internal,
                    "generation result could not be encoded",
                )
            })?
            .len()
            > orch_link_contract::version::MAX_FRAME_BYTES as usize
        {
            return Err(encoded_failure(
                ErrorCode::TooLarge,
                "the generation result exceeds the machine reply budget",
            ));
        }
        Ok(answer)
    }

    pub(super) async fn own_reply(
        &self,
        call: &OwnCall<'_>,
        request: UpstreamRequest,
        command: Option<Id>,
    ) -> Result<OwnReply, UpstreamError> {
        let Some(command_id) = command else {
            let (status, headers, body) = self
                .upstream
                .fetch(request, crate::upstream::MAX_WHOLE_REPLY_BYTES)
                .await?;
            return Ok(OwnReply {
                status,
                headers,
                body,
                usage: None,
                streamed: false,
                first_output: None,
            });
        };
        let entry = self
            .protocols
            .get(call.protocol)
            .ok_or_else(|| UpstreamError::Http("generation interface unavailable".into()))?;
        let response = self.upstream.send(request).await?;
        let streamed = response
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            });
        let mut body = response.body;
        let native = call
            .provider
            .endpoint_for(call.protocol)
            .map(|endpoint| {
                self.services
                    .builtin_sources
                    .endpoint_capabilities(call.protocol, endpoint.base_url(), call.model)
                    .native_tools
            })
            .unwrap_or_default();
        let mut observer = orch_model_contract::declared_usage::configured(
            entry.protocol.usage_observer(),
            &native,
        );
        if let Some(progress) = &call.progress {
            progress
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .streamed = Some(streamed);
        }
        if !response.status.is_success() || !streamed {
            let wire = Limited::new(body, crate::upstream::MAX_WHOLE_REPLY_BYTES)
                .collect()
                .await
                .map_err(|_| {
                    UpstreamError::Http(
                        "the generation response exceeded its bounded body or was cut".into(),
                    )
                })?
                .to_bytes();
            observer.observe(&wire);
            if let Some(progress) = &call.progress {
                progress
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .usage = observer.usage();
            }
            let usage = observer.usage();
            let answer = if response.status.is_success() {
                let mut decoded = entry.protocol.generation_output(&wire).map_err(|_| {
                    UpstreamError::Http("the generation response was incomplete".into())
                })?;
                if !call.capture_native_result {
                    decoded.native_result = serde_json::Value::Null;
                }
                serde_json::to_vec(&decoded).map(Bytes::from).map_err(|_| {
                    UpstreamError::Http("generation output could not be encoded".into())
                })?
            } else {
                wire
            };
            return Ok(OwnReply {
                status: response.status,
                headers: response.headers,
                body: answer,
                usage,
                streamed: false,
                first_output: None,
            });
        }
        let mut reader = entry
            .protocol
            .generation_stream()
            .ok_or_else(|| UpstreamError::Http("generation streaming unsupported".into()))?;
        let mut received = 0usize;
        let mut offset = 0i64;
        let mut first_output = None;
        let mut visible_bytes = 0usize;
        while let Some(frame) = tokio::time::timeout(IDLE_TIMEOUT, body.frame())
            .await
            .map_err(|_| UpstreamError::TimedOut("continue generation"))?
        {
            let frame =
                frame.map_err(|_| UpstreamError::Http("the generation stream was cut".into()))?;
            let Ok(bytes) = frame.into_data() else {
                continue;
            };
            received = received
                .checked_add(bytes.len())
                .ok_or_else(|| UpstreamError::Http("generation response too large".into()))?;
            if received > crate::upstream::MAX_WHOLE_REPLY_BYTES {
                return Err(UpstreamError::Http("generation response too large".into()));
            }
            observer.observe(&bytes);
            if let Some(progress) = &call.progress {
                progress
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .usage = observer.usage();
            }
            for delta in reader
                .push(&bytes)
                .map_err(|_| UpstreamError::Http("the generation stream was invalid".into()))?
            {
                let GenerationDelta::Text { text } = delta;
                visible_bytes = visible_bytes.saturating_add(text.len());
                if visible_bytes > orch_link_contract::version::MAX_FRAME_BYTES as usize / 2 {
                    return Err(UpstreamError::Http(
                        "generation text exceeds the machine reply budget".into(),
                    ));
                }
                if text.is_empty() {
                    continue;
                }
                first_output.get_or_insert_with(|| self.services.clock.time());
                let mut rest = text.as_str();
                while !rest.is_empty() {
                    let mut end = rest.len().min(crate::events::MAX_MERGED_DELTA_BYTES);
                    while !rest.is_char_boundary(end) {
                        end -= 1;
                    }
                    let fragment = &rest[..end];
                    self.services
                        .publisher
                        .publish(
                            Stream::Provider {
                                provider_id: call.provider.id(),
                            },
                            Event::ProviderGenerationDelta(
                                orch_link_contract::events::ProviderGenerationDelta {
                                    command_id,
                                    offset,
                                    text: fragment.to_owned(),
                                },
                            ),
                        )
                        .await
                        .map_err(|_| UpstreamError::Http("generation delivery ended".into()))?;
                    offset = offset.saturating_add(
                        i64::try_from(fragment.chars().count()).unwrap_or(i64::MAX),
                    );
                    rest = &rest[end..];
                }
            }
        }
        let mut output = reader.finish().map_err(|_| {
            UpstreamError::Http("the generation stream ended without a completed step".into())
        })?;
        if !call.capture_native_result {
            output.native_result = serde_json::Value::Null;
        }
        let body = serde_json::to_vec(&output)
            .map(Bytes::from)
            .map_err(|_| UpstreamError::Http("generation output could not be encoded".into()))?;
        Ok(OwnReply {
            status: response.status,
            headers: response.headers,
            body,
            usage: observer.usage(),
            streamed: true,
            first_output,
        })
    }
}

/// Admits referenced bytes against the existing wire allowance before any content transfer.
async fn materialize_images(
    content: &crate::content::Content,
    protocol: &dyn WireProtocol,
    options: &orch_model_contract::endpoint::EndpointOptions,
    auxiliary: Option<&Json>,
    request: &mut ProviderGenerate,
    command: Id,
    max_bytes: u64,
) -> Result<(), CommandError> {
    use base64::Engine as _;
    use orch_model_contract::generation::GenerationPart;
    let required = request.native_capability.is_some();
    for message in 0..request.input.messages.len() {
        for part in 0..request.input.messages[message].content.len() {
            let GenerationPart::ImageRef {
                blob,
                size,
                media_type,
                name,
                url,
            } = request.input.messages[message].content[part].clone()
            else {
                continue;
            };
            let fallback = GenerationPart::Text {
                text: format!("[{name}]({url})"),
            };
            // Keep the safe ordinary representation on every nonfatal exit below.
            request.input.messages[message].content[part] = fallback.clone();
            let unavailable = || {
                CommandError::new(
                    ErrorCode::Conflict,
                    "a required capability input image is unavailable",
                )
            };
            let too_large = || {
                CommandError::new(
                    ErrorCode::TooLarge,
                    "a required capability input image exceeds the request budget",
                )
            };
            let Ok(size_unsigned) = u64::try_from(size) else {
                if required {
                    return Err(unavailable());
                }
                continue;
            };
            // This cheap lower bound also excludes arithmetic/address-space overflow.
            let Some(encoded) = size_unsigned
                .div_ceil(3)
                .checked_mul(4)
                .filter(|encoded| *encoded <= max_bytes && usize::try_from(*encoded).is_ok())
            else {
                if required {
                    return Err(too_large());
                }
                continue;
            };
            request.input.messages[message].content[part] = GenerationPart::Image {
                media_type: media_type.clone(),
                data: String::new(),
                name: name.clone(),
            };
            let fits = build_generation_with_auxiliary(protocol, options, request, auxiliary)
                .ok()
                .and_then(|(_, _, body)| {
                    max_bytes.checked_sub(u64::try_from(body.len()).unwrap_or(u64::MAX))
                })
                .is_some_and(|remaining| encoded <= remaining);
            if !fits {
                request.input.messages[message].content[part] = fallback;
                if required {
                    return Err(too_large());
                }
                continue;
            }
            let bytes = match content.fetch(command, blob, size, &name).await {
                Ok(path) => read_image_bytes(&path, size_unsigned).await,
                Err(_) => None,
            };
            let Some(bytes) = bytes else {
                request.input.messages[message].content[part] = fallback;
                if required {
                    return Err(unavailable());
                }
                continue;
            };
            request.input.messages[message].content[part] = GenerationPart::Image {
                media_type,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                name,
            };
            // The protocol and final option overlays retain the definitive wire check.
            let fits = build_generation_with_auxiliary(protocol, options, request, auxiliary)
                .is_ok_and(|(_, _, body)| {
                    u64::try_from(body.len()).unwrap_or(u64::MAX) <= max_bytes
                });
            if !fits {
                request.input.messages[message].content[part] = fallback;
                if required {
                    return Err(too_large());
                }
            }
        }
    }
    Ok(())
}

async fn read_image_bytes(path: &std::path::Path, size: u64) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    // One extra byte detects a changed local file without loading an unbounded replacement.
    file.take(size.checked_add(1)?)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    (u64::try_from(bytes.len()).ok()? == size).then_some(bytes)
}

impl Forwarder {
    fn native_auxiliary(
        &self,
        provider: &ProviderEntry,
        request: &ProviderGenerate,
    ) -> Result<Option<Json>, CommandError> {
        use orch_model_contract::generation::{GenerationPart, GenerationRole};
        let Some(asked) = &request.native_capability else {
            return Ok(None);
        };
        let refused = || {
            CommandError::new(
                ErrorCode::Conflict,
                "the native capability request does not match its installed declaration",
            )
        };
        if !request.capture_native_result
            || !request.input.tools.is_empty()
            || !request.input.system.is_empty()
            || request.input.messages.len() != 1
        {
            return Err(refused());
        }
        let endpoint = provider
            .endpoint_for(&request.protocol)
            .ok_or_else(refused)?;
        let endpoint = orch_model_contract::services::generation_endpoint(
            &request.protocol,
            endpoint.base_url().as_str(),
            &serde_json::to_value(endpoint.options()).map_err(|_| refused())?,
        )
        .map_err(|_| refused())?;
        let binding = orch_provider_contract::services::native_binding(
            self.services.builtin_sources.capabilities().declarations(),
            &endpoint,
            &request.model_name,
            &request.model_parameters,
            &asked.capability,
        )
        .ok_or_else(refused)?;
        if orch_provider_contract::services::definition_hash(&endpoint, &binding).ok()
            != Some(asked.definition)
        {
            return Err(refused());
        }
        let rendered =
            orch_provider_contract::capabilities::render_auxiliary(&binding, &asked.input)
                .map_err(|_| refused())?;
        let prompt = rendered
            .get("prompt")
            .and_then(Json::as_str)
            .ok_or_else(refused)?;
        let message = &request.input.messages[0];
        if message.role != GenerationRole::User
            || !message.continuation.is_empty()
            || !matches!(message.content.first(), Some(GenerationPart::Text { text }) if text == prompt)
            || message.content.iter().skip(1).any(|part| {
                !matches!(
                    part,
                    GenerationPart::Image { .. } | GenerationPart::ImageRef { .. }
                )
            })
            || message.content.len() > 1 && request.model_parameters.input_images != Some(true)
        {
            return Err(refused());
        }
        if let Some(options) = rendered.get("options") {
            let options = options.as_object().ok_or_else(refused)?;
            if options.iter().any(|(key, value)| {
                request.params.as_ref().and_then(|params| params.get(key)) != Some(value)
            }) {
                return Err(refused());
            }
        }
        if let Some(body) = rendered.get("body") {
            let body = body.as_object().ok_or_else(refused)?;
            if body.keys().any(|key| {
                orch_provider_contract::calls::owned_option(key)
                    && !matches!(key.as_str(), "tools" | "tool_choice")
            }) {
                return Err(refused());
            }
        }
        Ok(Some(rendered))
    }
}

fn read_generation(body: &[u8], capture: bool) -> Result<GenerationOutput, &'static str> {
    let mut output: GenerationOutput =
        serde_json::from_slice(body).map_err(|_| "the generation step could not be read")?;
    if !capture {
        output.native_result = serde_json::Value::Null;
        let normalized =
            serde_json::to_vec(&output).map_err(|_| "the generation step could not be encoded")?;
        if normalized.len() > orch_link_contract::version::MAX_FRAME_BYTES as usize / 2 {
            return Err("the generated step exceeds the machine reply budget");
        }
    }
    Ok(output)
}

fn measured_failure(
    error: CommandError,
    request: &ProviderGenerate,
    tokens: orch_link_contract::foreign::TokenUsage,
    timing: &orch_api_contract::usage::RequestTiming,
    request_ref: Option<&str>,
) -> CommandError {
    let mut observed = orch_agent_contract::requests::UsageRequest {
        id: "generation".into(),
        model: Some(request.model_name.clone()),
        tokens: tokens.into(),
        asked: timing.asked,
        first_output: timing.first_output,
        end: timing.end,
        output_streamed: timing.output_streamed,
        provider_request_id: request_ref.map(str::to_owned),
        web_searches: timing.web_searches.value().copied(),
        web_fetches: timing.web_fetches.value().copied(),
        ..Default::default()
    };
    observed.finish("harness_error", Some(&error.detail), None);
    let mut timing = timing.clone();
    timing.outcome = orch_api_contract::usage::UsageMeasurement::Told {
        value: "harness_error".into(),
    };
    error.with_details(serde_json::json!({"model_request":observed,"request_timing":timing}))
}

#[cfg(test)]
fn build_generation(
    protocol: &dyn WireProtocol,
    options: &orch_model_contract::endpoint::EndpointOptions,
    request: &ProviderGenerate,
) -> Result<(Method, &'static str, Bytes), CommandError> {
    build_generation_with_auxiliary(protocol, options, request, None)
}

fn build_generation_with_auxiliary(
    protocol: &dyn WireProtocol,
    options: &orch_model_contract::endpoint::EndpointOptions,
    request: &ProviderGenerate,
    auxiliary: Option<&Json>,
) -> Result<(Method, &'static str, Bytes), CommandError> {
    let (method, path, body) = protocol
        .generation_request_at(options, &request.model_name, &request.input, true)
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "this interface does not support dialogue generation",
            )
        })?;
    let body = if let Some(extra) = auxiliary
        .and_then(|auxiliary| auxiliary.get("body"))
        .and_then(Json::as_object)
    {
        let mut payload: Json = serde_json::from_slice(&body).map_err(|_| {
            CommandError::new(ErrorCode::Internal, "generation request could not be read")
        })?;
        let members = payload.as_object_mut().ok_or_else(|| {
            CommandError::new(ErrorCode::Internal, "generation request must be an object")
        })?;
        members.extend(
            extra
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Bytes::from(serde_json::to_vec(&payload).map_err(|_| {
            CommandError::new(
                ErrorCode::Internal,
                "generation request could not be encoded",
            )
        })?)
    } else {
        body
    };
    // Installed auxiliary options may introduce their declared native tool through the
    // same protocol builder. Capture that intent before caller options or model constraints.
    let body = match auxiliary.and_then(|auxiliary| auxiliary.get("options")) {
        Some(options) => Bytes::from(
            orch_model_contract::parameters::apply_request_for_model(
                protocol,
                path,
                &body,
                options,
                false,
                &request.model_parameters,
            )
            .map_err(|reason| CommandError::new(ErrorCode::Conflict, reason))?,
        ),
        None => body,
    };
    let declared_tools = serde_json::from_slice::<serde_json::Value>(&body)
        .map_err(|_| {
            CommandError::new(ErrorCode::Internal, "generation request could not be read")
        })?
        .get("tools")
        .cloned();
    let body = match &request.params {
        Some(params) => Bytes::from(
            orch_model_contract::parameters::apply_request_for_model(
                protocol,
                path,
                &body,
                params,
                false,
                &request.model_parameters,
            )
            .map_err(|reason| CommandError::new(ErrorCode::Conflict, reason))?,
        ),
        None => body,
    };
    let body = Bytes::from(
        orch_model_contract::parameters::outgoing_at(
            protocol.name(),
            path,
            &request.model_parameters,
            &body,
        )
        .map_err(|reason| CommandError::new(ErrorCode::Conflict, reason))?,
    );
    if let Some(native) = &request.native_capability {
        let payload: Json = serde_json::from_slice(&body).map_err(|_| {
            CommandError::new(
                ErrorCode::Internal,
                "the native capability request could not be read",
            )
        })?;
        if let Some(binding) = request
            .model_parameters
            .native_tools
            .get(protocol.name())
            .and_then(|tools| tools.get(&native.capability))
            .and_then(Option::as_ref)
            .filter(|binding| binding.request.get("container").is_some())
        {
            let declared: orch_model_contract::web_search::WebSearchDeclaration =
                serde_json::from_value(binding.request.clone()).map_err(|_| {
                    CommandError::new(
                        ErrorCode::Unsupported,
                        "the native capability tool declaration is unavailable",
                    )
                })?;
            if !declared.selected(&payload) {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the provider ceiling disables this required native capability",
                ));
            }
        }
    }
    if serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|body| body.get("tools").cloned())
        != declared_tools
    {
        return Err(CommandError::new(
            ErrorCode::Conflict,
            "generation options cannot add or replace the caller's declared tools",
        ));
    }
    Ok((method, path, body))
}

#[cfg(test)]
#[path = "generation_image_refs.rs"]
mod image_refs;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
mod tests {
    use super::*;

    #[test]
    fn uncaptured_native_metadata_cannot_reduce_the_existing_dialogue_reply_allowance() {
        let limit = orch_link_contract::version::MAX_FRAME_BYTES as usize / 2;
        let mut output = GenerationOutput {
            native_result: serde_json::json!({"response":{"native_data":"x".repeat(limit+1)},"frames":[]}),
            message: orch_model_contract::generation::GenerationMessage {
                role: orch_model_contract::generation::GenerationRole::Assistant,
                content: vec![orch_model_contract::generation::GenerationPart::Text {
                    text: "Answer".into(),
                }],
                continuation: Vec::new(),
            },
            stop: orch_model_contract::generation::GenerationStop::Complete,
        };
        let bytes = serde_json::to_vec(&output).unwrap();
        assert!(bytes.len() > limit);
        let ordinary = read_generation(&bytes, false).unwrap();
        assert!(ordinary.native_result.is_null());
        assert_eq!(ordinary.message, output.message);
        assert_eq!(
            read_generation(&bytes, true).unwrap().native_result,
            output.native_result
        );
        output.message.content = vec![orch_model_contract::generation::GenerationPart::Text {
            text: "x".repeat(limit + 1),
        }];
        assert!(
            read_generation(&serde_json::to_vec(&output).unwrap(), false).is_err(),
            "ordinary output still keeps its actual prior bound"
        );
    }

    #[test]
    fn post_request_capture_failures_retain_native_quantities_and_exact_timing() {
        let mut timing = orch_api_contract::usage::RequestTiming {
            web_searches: orch_api_contract::usage::UsageMeasurement::Told { value: 2 },
            web_fetches: orch_api_contract::usage::UsageMeasurement::Told { value: 1 },
            requested_service_tier: Some("priority".into()),
            ..Default::default()
        };
        timing.declared_counters.insert(
            "actual_native_calls".into(),
            orch_api_contract::usage::UsageMeasurement::Told { value: 2 },
        );
        let error = measured_failure(
            CommandError::new(ErrorCode::Internal, "captured result upload failed"),
            &request(),
            orch_link_contract::foreign::TokenUsage {
                input_tokens: 5,
                output_tokens: 3,
                total_tokens: 8,
                ..Default::default()
            },
            &timing,
            Some("actual-request"),
        );
        let details = error.details.unwrap();
        assert_eq!(details["model_request"]["web_searches"], 2);
        assert_eq!(details["model_request"]["web_fetches"], 1);
        assert_eq!(
            details["model_request"]["provider_request_id"],
            "actual-request"
        );
        let retained: orch_api_contract::usage::RequestTiming =
            serde_json::from_value(details["request_timing"].clone()).unwrap();
        assert_eq!(retained.declared_counters, timing.declared_counters);
        assert_eq!(
            retained.requested_service_tier,
            timing.requested_service_tier
        );
        assert_eq!(
            retained.outcome,
            orch_api_contract::usage::UsageMeasurement::Told {
                value: "harness_error".into()
            }
        );
    }
    use orch_model_contract::endpoint::{EndpointOptions, OutputLimitField};
    use orch_model_contract::generation::{
        GenerationInput, GenerationMessage, GenerationPart, GenerationRole, GenerationTool,
    };
    use serde_json::json;

    fn request() -> ProviderGenerate {
        ProviderGenerate {
            native_capability: None,
            capture_native_result: false,
            provider_id: Id::nil(),
            protocol: "openai-chat".into(),
            model_name: "question-model".into(),
            params: Some(json!({"temperature":0.2})),
            model_parameters: Default::default(),
            input: GenerationInput {
                system: "Use the declared read tool.".into(),
                messages: vec![GenerationMessage {
                    role: GenerationRole::User,
                    content: vec![GenerationPart::Text {
                        text: "Read a source".into(),
                    }],
                    continuation: vec![],
                }],
                tools: vec![GenerationTool {
                    name: "read".into(),
                    description: "Read an admitted source".into(),
                    parameters: json!({"type":"object"}),
                }],
                max_output_tokens: 128,
            },
        }
    }

    #[test]
    fn installed_auxiliary_options_introduce_their_tool_before_final_overlays_are_checked() {
        let protocol = orch_protocol_openai_responses::OpenAiResponses;
        let mut request = request();
        request.protocol = "openai-responses".into();
        request.input.tools.clear();
        request.model_parameters.web_search = Some(vec!["live".into()]);
        request.params = Some(json!({"web_search":"live"}));
        let auxiliary = json!({"options":{"web_search":"live"},"body":{"include":["web_search_call.action.sources"]}});
        let (_, _, body) = build_generation_with_auxiliary(
            &protocol,
            &EndpointOptions::default(),
            &request,
            Some(&auxiliary),
        )
        .unwrap();
        let body: Json = serde_json::from_slice(&body).unwrap();
        assert!(
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["type"] == "web_search" && tool["external_web_access"] == true)
        );
        assert_eq!(body["include"], auxiliary["body"]["include"]);
        request.params = Some(json!({"web_search":"disabled"}));
        assert!(
            build_generation_with_auxiliary(
                &protocol,
                &EndpointOptions::default(),
                &request,
                Some(&auxiliary),
            )
            .is_err(),
            "caller options cannot remove the declared auxiliary tool"
        );
        request.params = Some(json!({"web_search":"live"}));
        request.model_parameters.web_search = Some(vec![]);
        assert!(
            build_generation_with_auxiliary(
                &protocol,
                &EndpointOptions::default(),
                &request,
                Some(&auxiliary),
            )
            .is_err(),
            "a closed model ceiling still refuses before any HTTP call"
        );
    }

    #[test]
    fn generation_uses_the_endpoints_output_field_and_final_request_options() {
        for field in [
            OutputLimitField::MaxTokens,
            OutputLimitField::MaxCompletionTokens,
        ] {
            let options = EndpointOptions {
                output_limit_field: Some(field),
                params: std::collections::BTreeMap::from([("temperature".into(), json!(0.9))]),
                ..Default::default()
            };
            let (_, path, bytes) = build_generation(
                &orch_protocol_openai_responses::OpenAiChat,
                &options,
                &request(),
            )
            .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(path, "/chat/completions");
            assert_eq!(body[field.as_str()], 128);
            let other = if field == OutputLimitField::MaxTokens {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            assert!(body.get(other).is_none());
            // The retained explicit choice already won over endpoint defaults at admission.
            assert_eq!(body["temperature"], json!(0.2));
            assert_eq!(body["tools"][0]["function"]["name"], "read");
            assert_eq!(body["stream"], true);
        }
    }

    #[test]
    fn generation_request_options_keep_the_native_model_ceiling() {
        let mut request = request();
        request.params = Some(json!({"temperature":0.8}));
        request.model_parameters.request_options = Some(std::collections::BTreeMap::from([(
            "openai-chat".into(),
            json!({"type":"object","properties":{"temperature":{"type":"number","maximum":0.3}}}),
        )]));
        let options = EndpointOptions {
            output_limit_field: Some(OutputLimitField::MaxCompletionTokens),
            ..Default::default()
        };
        assert!(
            build_generation(
                &orch_protocol_openai_responses::OpenAiChat,
                &options,
                &request
            )
            .is_err()
        );
    }
}
