//! Images use the selected common paid runner; accepted pixels outlive optional file delivery.
use super::{
    Answer, CallContext,
    definitions::{Code, Refusal},
    source,
};
use crate::{Engine, EngineError};
use futures::{StreamExt, future::BoxFuture};
use orch_blob_contract::store::{Verifier, stream_of};
use orch_db::{
    Conn,
    queries::{resources, storage, threads},
};
use orch_image_backends::{DecodeLimits, Image};
use orch_types::{Hash, Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

pub(crate) mod delivery;
pub(crate) mod inputs;
mod limits;
const SLOT: &str = "generated_images";
const DELIVERY_UNCONFIRMED: &str = "The image is retained, but its workspace file was not confirmed. Check the path and retry delivery.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    prompt: Option<String>,
    output_paths: Vec<String>,
    options: Option<serde_json::Map<String, Json>>,
    #[serde(default)]
    images: Vec<String>,
    mask: Option<String>,
    delivery_ref: Option<Id>,
    #[serde(rename = "max_bytes")]
    _max_bytes: Option<usize>,
}

struct PreparedInput {
    value: Json,
    inline_images: Option<orch_provider_contract::capabilities::Binding>,
}
impl PreparedInput {
    fn plain(value: Json) -> Self {
        Self {
            value,
            inline_images: None,
        }
    }
}

fn inline_image_input(
    binding: &orch_provider_contract::capabilities::Binding,
    input: Json,
    pixels: &[orch_image_backends::InputImage],
) -> Result<Json, EngineError> {
    orch_image_backends::materialize_inline_inputs(binding, input, pixels)
        .map_err(EngineError::unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;
    type Proof = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn inline_image_inputs_larger_than_eight_megabytes_are_prepared_for_the_provider() -> Proof {
        let binding = orch_provider_contract::capabilities::Binding {
            request: json!({"body":{
                "images":{"from_input":"/images"}, "prompt":{"from_input":"/prompt"}
            }}),
            extensions: BTreeMap::from([(
                "x_input_images".into(),
                json!({
                    "encoding":"inline-json", "field":"images", "item":{"url":{"from_input":"/url"}}
                }),
            )]),
            ..Default::default()
        };
        // Nine MiB of inline text fits the shipped message allowance. The verified
        // binary pixels are smaller because base64 adds one third.
        let image = orch_image_backends::InputImage {
            name: "source-1".into(),
            media_type: "image/png".into(),
            bytes: vec![17; 9 * 1024 * 1024 / 4 * 3],
        };
        let input = inline_image_input(&binding, json!({"prompt":"An orchard"}), &[image])?;
        let url = input["images"][0]["url"]
            .as_str()
            .ok_or("missing inline image")?;
        assert!(url.len() > 9 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn the_native_image_request_uses_only_declared_neutral_options_and_immutable_tool_identity()
    -> Proof {
        let declarations = orch_provider_contract::capabilities::catalogue()?;
        let binding = declarations
            .interfaces
            .get("openai-responses")
            .and_then(|interface| interface.native_tools.get("image_generation"))
            .ok_or("missing native image declaration")?;
        let options = serde_json::from_value(
            json!({"quality":"high","background":"transparent","output_format":"png","model":"fixture-image-model"}),
        )?;
        let input = native_image_input(binding, "Make an orchard", Some(&options))?;
        let rendered = orch_provider_contract::capabilities::render_auxiliary(binding, &input)?;
        assert_eq!(rendered["prompt"], "Make an orchard");
        assert_eq!(rendered["body"]["tools"][0]["type"], "image_generation");
        assert_eq!(rendered["body"]["tools"][0]["quality"], "high");
        assert_eq!(rendered["body"]["tools"][0]["model"], "fixture-image-model");
        assert!(input.get("output_paths").is_none());
        let overridden = serde_json::from_value(json!({"type":"function"}))?;
        assert!(native_image_input(binding, "Make an orchard", Some(&overridden)).is_err());
        let bad = serde_json::from_value(json!({"quality":"not-a-declared-quality"}))?;
        assert!(native_image_input(binding, "Make an orchard", Some(&bad)).is_err());
        let default = native_image_input(binding, "Make an orchard", None)?;
        let rendered = orch_provider_contract::capabilities::render_auxiliary(binding, &default)?;
        assert_eq!(
            rendered["body"]["tools"],
            json!([{"type":"image_generation"}])
        );
        Ok(())
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Output {
    blob: Hash,
    size: i64,
    media_type: String,
    item: Id,
    native_ref: Option<String>,
    revised_prompt: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    reference: Id,
    thread: Id,
    turn: Id,
    receipt: super::capability_service::CapabilityResultReceipt,
    images: Vec<Output>,
    released: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper: Option<Json>,
}

fn accepted_pending(
    call: &CallContext,
    reference: Id,
    error: &EngineError,
) -> Result<Answer, EngineError> {
    if matches!(
        error,
        EngineError::StaleToolSource | EngineError::ToolCallIdentity
    ) {
        return Err(EngineError::StaleToolSource);
    }
    tracing::debug!(%error,"Accepted image output is retained for a materialization retry.");
    let body = json!({"state":"accepted","delivery_ref":reference,"reason":"The backend returned accepted output, but its images could not yet be stored. Retry this delivery_ref within the current turn with output_paths, without a new prompt. This never generates again."});
    if !orch_agent_contract::tool_answers::fits("image_generation", &call.arguments, &body) {
        return Err(EngineError::unreadable(
            "The retained image outcome exceeds the reply budget.",
        ));
    }
    Ok(Answer::Done(body))
}

async fn optional_record(
    conn: &mut Conn,
    thread: Id,
    reference: Id,
) -> Result<Option<Record>, EngineError> {
    let value: Option<Json> = sqlx::query_scalar(
        "SELECT plan->$3 FROM thread_tool_calls WHERE answer_id=$1 AND thread_id=$2 AND plan ? $3",
    )
    .bind(reference)
    .bind(thread)
    .bind(SLOT)
    .fetch_optional(conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let record: Record = serde_json::from_value(value)?;
    if record.reference != reference || record.thread != thread || record.images.is_empty() {
        return Err(EngineError::ToolCallIdentity);
    }
    Ok(Some(record))
}

pub(crate) async fn load_record(
    conn: &mut Conn,
    thread: Id,
    reference: Id,
) -> Result<Record, EngineError> {
    optional_record(conn, thread, reference)
        .await?
        .ok_or(EngineError::StaleToolSource)
}

async fn result_read(
    engine: &Engine,
    call: &CallContext,
    record: &Record,
) -> Result<(), EngineError> {
    let mut conn = engine.db().acquire().await?;
    source::check_call(&mut conn, call).await?;
    let origins = source::original_callers(
        &mut conn,
        call,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    result_access(
        &mut conn,
        call,
        record,
        &origins,
        engine.inner.clock.as_ref(),
    )
    .await?;
    if !retained_images(&mut conn, call.thread, record).await? {
        return Err(EngineError::StaleToolSource);
    }
    Ok(())
}

async fn result_access(
    conn: &mut Conn,
    call: &CallContext,
    record: &Record,
    origins: &source::CallOrigins,
    clock: &dyn orch_types::Clock,
) -> Result<(), EngineError> {
    if record.turn != call.turn {
        for caller in origins.callers() {
            let principal = orch_authz::Principal::load(conn, caller.clone(), clock.time()).await?;
            if !orch_authz::nodes::check(
                conn,
                &principal,
                call.thread,
                orch_access_contract::permissions::Permission::ThreadView,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(EngineError::StaleToolSource);
            }
        }
    }
    Ok(())
}

async fn retained_images(
    conn: &mut Conn,
    thread: Id,
    record: &Record,
) -> Result<bool, EngineError> {
    if record.thread != thread {
        return Ok(false);
    }
    for (index, output) in record.images.iter().enumerate() {
        let known:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE id=$1 AND thread_id=$2 AND turn_id=$3 AND kind='tool_call' AND payload->'tool'->>'kind'='image' AND payload->'tool'->>'blob'=$4 AND native_ref=$5)")
            .bind(output.item).bind(thread).bind(record.turn).bind(output.blob.to_string()).bind(format!("orch.image_generation:{}:{index}",record.reference)).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !known {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Data preparation uses the core's exact stored backend selector; it chooses no route or fallback.
async fn neutral_input(
    engine: &Engine,
    conn: &mut Conn,
    call: &CallContext,
    args: &Arguments,
) -> Result<PreparedInput, EngineError> {
    let sources = engine
        .inner
        .capability_services
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or(EngineError::StaleToolSource)?;
    let declarations = sources.capabilities().declarations();
    let selected = super::capability_service::selection::load(
        conn,
        call.thread,
        declarations,
        "image_generation",
    )
    .await?
    .ok_or(EngineError::StaleToolSource)?;
    let mut input = args.options.clone().unwrap_or_default();
    if ["prompt", "n", "images", "image", "mask", "input_references"]
        .iter()
        .any(|key| input.contains_key(*key))
    {
        return Err(EngineError::unreadable(
            "Image model, prompt and count are selected by the offering and output paths. Input images require their native materializer; they are never omitted.",
        ));
    }
    if input.get("action").and_then(Json::as_str) == Some("edit") && args.images.is_empty() {
        return Err(EngineError::unreadable(
            "Image editing needs the selected backend's input-image materializer before it can spend.",
        ));
    }
    input.insert(
        "prompt".into(),
        json!(
            args.prompt
                .as_deref()
                .ok_or(EngineError::ToolCallIdentity)?
        ),
    );
    if selected.executor == "api-backed" {
        if input.contains_key("model") {
            return Err(EngineError::unreadable(
                "The image API model belongs to the selected offering, not an option override.",
            ));
        }
        let reference = selected
            .reference
            .offering
            .as_ref()
            .ok_or(EngineError::StaleToolSource)?;
        let row = resources::routes::providers(conn, &[reference.provider])
            .await?
            .into_iter()
            .next()
            .ok_or(EngineError::StaleToolSource)?;
        let endpoints = super::capability_service::endpoints(&row.upstreams)?;
        let configs: BTreeMap<String, orch_provider_contract::services::ServiceConfig> =
            serde_json::from_value(row.service_offerings)?;
        let catalog =
            orch_provider_contract::services::catalogue(declarations, &endpoints, &configs);
        let offering = catalog
            .offerings
            .get(&reference.offering)
            .ok_or(EngineError::StaleToolSource)?;
        let model = offering
            .parameters
            .get("model")
            .and_then(Json::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                EngineError::unreadable("The selected image offering does not declare its model.")
            })?;
        input.insert("model".into(), json!(model));
        input.insert("n".into(), json!(args.output_paths.len()));
        let endpoint = catalog
            .endpoints
            .get(&offering.endpoint)
            .ok_or(EngineError::StaleToolSource)?;
        let binding = orch_provider_contract::services::service_binding(
            declarations,
            endpoint,
            offering,
            "image_generation",
        )
        .ok_or(EngineError::StaleToolSource)?;
        let inline = orch_image_backends::has_inline_inputs(binding);
        if !args.images.is_empty() && !inline {
            input.insert(
                "image".into(),
                json!(
                    (0..args.images.len())
                        .map(|index| format!("source-{}", index + 1))
                        .collect::<Vec<_>>()
                ),
            );
            if args.mask.is_some() {
                input.insert("mask".into(), json!("mask-1"));
            }
        }
        if inline && args.mask.is_some() {
            return Err(EngineError::unreadable(
                "The selected inline image binding does not declare a mask input.",
            ));
        }
        let rendered = orch_provider_contract::capabilities::render_request(
            binding,
            &Json::Object(input.clone()),
        )
        .map_err(EngineError::unreadable)?;
        if !args.images.is_empty() && rendered["encoding"] != "multipart" && !inline {
            return Err(EngineError::unreadable(
                "This image service has no declared input-image materializer for its request encoding.",
            ));
        }
        Ok(PreparedInput {
            value: Json::Object(input),
            inline_images: (!args.images.is_empty() && inline).then(|| binding.clone()),
        })
    } else if selected.executor == "program-session" {
        if args.mask.is_some() {
            input.insert("mask".into(), json!("mask-1"));
        }
        input.insert("n".into(), json!(args.output_paths.len()));
        Ok(PreparedInput::plain(Json::Object(input)))
    } else if selected.executor == "model-backed" {
        let reference = selected
            .reference
            .offering
            .as_ref()
            .ok_or(EngineError::StaleToolSource)?;
        let provider = resources::routes::providers(conn, &[reference.provider])
            .await?
            .into_iter()
            .next()
            .ok_or(EngineError::StaleToolSource)?;
        let model = resources::thread_engine::model(conn, reference.provider, &reference.offering)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let facts = super::capability_service::model_backend_facts(
            &sources,
            &provider.upstreams,
            &model,
            "image_generation",
        )?
        .ok_or(EngineError::StaleToolSource)?;
        if args.mask.is_some() {
            return Err(EngineError::unreadable(
                "This model's image binding does not declare a mask input; choose a service that does.",
            ));
        }
        if !args.images.is_empty() && facts.parameters.input_images != Some(true) {
            return Err(EngineError::unreadable(
                "The selected image model does not accept image inputs.",
            ));
        }
        native_image_input(
            &facts.binding,
            args.prompt
                .as_deref()
                .ok_or(EngineError::ToolCallIdentity)?,
            args.options.as_ref(),
        )
        .map(PreparedInput::plain)
    } else {
        Err(EngineError::unreadable(
            "The selected image executor is not installed.",
        ))
    }
}

fn native_image_input(
    binding: &orch_provider_contract::capabilities::Binding,
    prompt: &str,
    options: Option<&serde_json::Map<String, Json>>,
) -> Result<Json, EngineError> {
    let options = options.cloned().unwrap_or_default();
    let schema = &binding.request["options_schema"];
    orch_types::schema::validate(schema, &Json::Object(options.clone()))
        .map_err(EngineError::unreadable)?;
    let mut tool = binding.request["tool"]
        .as_object()
        .cloned()
        .ok_or(EngineError::ToolCallIdentity)?;
    for (name, value) in options {
        if tool.contains_key(&name) {
            return Err(EngineError::ToolCallIdentity);
        }
        tool.insert(name, value);
    }
    let input = json!({"prompt":prompt,"native_tool":tool});
    orch_provider_contract::capabilities::render_auxiliary(binding, &input)
        .map_err(EngineError::unreadable)?;
    Ok(input)
}

fn media(image: &Image) -> Result<String, EngineError> {
    if let Ok(format) = image::guess_format(&image.bytes) {
        return Ok(format.to_mime_type().into());
    }
    image.media_type.clone().ok_or_else(|| {
        EngineError::unreadable("The accepted image has no declared or recognizable media type.")
    })
}

impl Engine {
    pub(crate) fn image_generation_call<'a>(
        &'a self,
        call: &'a CallContext,
    ) -> BoxFuture<'a, Result<Answer, EngineError>> {
        Box::pin(async move {
            let args: Arguments = match serde_json::from_value(call.arguments.clone()) {
                Ok(args) => args,
                Err(error) => return Ok(Refusal::invalid(error.to_string()).into()),
            };
            if args.output_paths.is_empty()
                || args
                    .output_paths
                    .iter()
                    .any(|path| !super::workspace_files::relative_file(path))
            {
                return Ok(Refusal::invalid("output_paths names files under the thread's workspace; absolute paths, parent traversal and empty paths are not accepted.").into());
            }
            if args.mask.is_some() && args.images.is_empty() {
                return Ok(Refusal::invalid("A mask requires at least one source image.").into());
            }
            if args
                .images
                .iter()
                .chain(args.mask.iter())
                .any(|path| !super::workspace_files::relative_file(path))
            {
                return Ok(Refusal::invalid(
                    "Input images and masks must be relative files under the thread's workspace.",
                )
                .into());
            }
            let preview = json!({"delivery_ref":Id::nil(),"images":args.output_paths.iter().map(|path|json!({"blob":"0".repeat(64),"item_id":Id::nil(),"path":path,"media_type":"image/octet-stream","size":i64::MAX,"status":"failed","reason":DELIVERY_UNCONFIRMED})).collect::<Vec<_>>()});
            let unique: std::collections::BTreeSet<_> = args.output_paths.iter().collect();
            if unique.len() != args.output_paths.len() {
                return Ok(Refusal::invalid(
                    "Each output image needs a distinct workspace destination.",
                )
                .into());
            }
            if !orch_agent_contract::tool_answers::fits(
                "image_generation",
                &call.arguments,
                &preview,
            ) {
                return Ok(Refusal::new(Code::TooLarge,"The complete image destinations and outcomes exceed max_bytes; shorten output_paths or increase the ordinary reply budget before generating.").into());
            }
            let answer = crate::ids::tool_result(call.thread, &call.call_ref);
            let mut record = if let Some(reference) = args.delivery_ref {
                if args.prompt.is_some()
                    || args.options.is_some()
                    || !args.images.is_empty()
                    || args.mask.is_some()
                {
                    return Ok(Refusal::invalid("A delivery retry has delivery_ref and output_paths, without a new prompt or options.").into());
                }
                let mut conn = self.db().acquire().await?;
                let record = optional_record(&mut conn, call.thread, reference).await?;
                let pending: Option<Json> = if record.is_none() {
                    sqlx::query_scalar("SELECT plan->'accepted_service_result' FROM thread_tool_calls WHERE answer_id=$1 AND thread_id=$2 AND turn_id=$3 AND tool='image_generation' AND NOT retired")
                        .bind(reference).bind(call.thread).bind(call.turn).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.flatten()
                } else {
                    None
                };
                drop(conn);
                if let Some(record) = record {
                    result_read(self, call, &record).await?;
                    record
                } else {
                    let receipt: super::capability_service::CapabilityResultReceipt =
                        serde_json::from_value(pending.ok_or(EngineError::StaleToolSource)?)?;
                    let accepted = self.read_accepted_capability_result(call, &receipt).await?;
                    match self
                        .materialize_image_result(
                            call,
                            reference,
                            &accepted,
                            args.output_paths.len(),
                        )
                        .await
                    {
                        Ok(record) => record,
                        Err(error) => return accepted_pending(call, reference, &error),
                    }
                }
            } else {
                if args
                    .prompt
                    .as_deref()
                    .is_none_or(|prompt| prompt.trim().is_empty())
                {
                    return Ok(
                        Refusal::invalid("A new generation needs prompt and output_paths.").into(),
                    );
                }
                let existing = {
                    let mut conn = self.db().acquire().await?;
                    source::check_call(&mut conn, call).await?;
                    sqlx::query_scalar::<_, Option<Json>>(
                        "SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1",
                    )
                    .bind(answer)
                    .bind(SLOT)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?
                };
                if let Some(existing) = existing {
                    let record: Record = serde_json::from_value(existing)?;
                    result_read(self, call, &record).await?;
                    record
                } else {
                    let pending = {
                        let mut conn = self.db().acquire().await?;
                        source::check_call(&mut conn, call).await?;
                        sqlx::query_scalar::<_,Option<Json>>("SELECT plan->'accepted_service_result' FROM thread_tool_calls WHERE answer_id=$1").bind(answer).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?
                    };
                    let accepted = if let Some(pending) = pending {
                        let receipt: super::capability_service::CapabilityResultReceipt =
                            serde_json::from_value(pending)?;
                        self.read_accepted_capability_result(call, &receipt).await?
                    } else {
                        let input = {
                            let mut conn = self.db().acquire().await?;
                            source::check_call(&mut conn, call).await?;
                            neutral_input(self, &mut conn, call, &args).await?
                        };
                        let mut images = self.image_inputs(call, &args).await?;
                        let mut input = input;
                        if let Some(binding) = input.inline_images {
                            let mut pixels = Vec::with_capacity(images.len());
                            for part in &images {
                                let orch_model_contract::generation::GenerationPart::ImageRef {
                                    blob,
                                    size,
                                    media_type,
                                    name,
                                    ..
                                } = part
                                else {
                                    return Err(EngineError::ToolCallIdentity);
                                };
                                pixels.push(orch_image_backends::InputImage {
                                    name: name.clone(),
                                    media_type: media_type.clone(),
                                    bytes: self.image_blob_bytes(*blob, *size).await?,
                                });
                            }
                            input.value = inline_image_input(&binding, input.value, &pixels)?;
                            // The verified pixels are now part of neutral JSON itself. No unresolved
                            // image reference is discarded or delegated to a JSON-only keeper.
                            images.clear();
                        }
                        self.execute_materialized_capability_service(
                            call,
                            "image_generation",
                            input.value,
                            images,
                        )
                        .await?
                    };
                    match self
                        .materialize_image_result(call, answer, &accepted, args.output_paths.len())
                        .await
                    {
                        Ok(record) => record,
                        Err(error) => return accepted_pending(call, answer, &error),
                    }
                }
            };
            if record.images.len() != args.output_paths.len() {
                return Ok(Answer::Done(
                    json!({"state":"retained","delivery_ref":record.reference,"count":record.images.len(),"reason":"output_paths must name one destination for each retained image; retry this delivery_ref with that many paths. No image was regenerated."}),
                ));
            }
            // Each pixel blob now has its normal durable image-item pin. A retry reads that
            // owned artifact, rather than keeping the provider's larger temporary packet alive.
            if !record.released && record.turn == call.turn {
                match self
                    .release_accepted_capability_result(call, &record.receipt)
                    .await
                {
                    Ok(()) => {
                        record.released = true;
                        let mut tx = self.db().begin().await?;
                        source::check_call(&mut tx, call).await?;
                        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1").bind(record.reference).bind(SLOT).bind(serde_json::to_value(&record)?).execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
                        tx.commit().await.map_err(orch_db::DbError::from)?;
                    }
                    Err(error) => {
                        tracing::debug!(%error,"The retained images still hold their accepted provider packet.")
                    }
                }
            }
            let mut files = Vec::new();
            for (index, (output, path)) in record.images.iter().zip(&args.output_paths).enumerate()
            {
                let outcome = self
                    .deliver_generated_image(call, &record, index, path)
                    .await;
                let (status, reason) = match outcome {
                    Ok(crate::sharing::FileWritten::Ready) => ("ready", None),
                    Ok(crate::sharing::FileWritten::Downloading { .. }) => ("downloading", None),
                    Ok(crate::sharing::FileWritten::Differs) => (
                        "differs",
                        Some(
                            "Another file already occupies this path; choose a different destination.",
                        ),
                    ),
                    Err(_) => ("failed", Some(DELIVERY_UNCONFIRMED)),
                };
                files.push(json!({"blob":output.blob,"item_id":output.item,"path":path,"media_type":output.media_type,"size":output.size,"status":status,"reason":reason}));
            }
            result_read(self, call, &record).await?;
            let mut body = json!({"delivery_ref":record.reference,"images":files});
            if let Some(helper) = &record.helper {
                body["helper"] = helper.clone();
            }
            Ok(Answer::Done(body))
        })
    }

    async fn image_limits(
        &self,
        call: &CallContext,
        count: usize,
    ) -> Result<DecodeLimits, EngineError> {
        let mut conn = self.db().acquire().await?;
        source::check_call(&mut conn, call).await?;
        let largest = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::STORAGE_MAX_BLOB_BYTES,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("The image storage budget is not configured."))?;
        let largest = usize::try_from(largest).map_err(EngineError::unreadable)?;
        Ok(DecodeLimits {
            images: count,
            image_bytes: largest,
            total_bytes: largest,
        })
    }

    async fn materialize_image_result(
        &self,
        call: &CallContext,
        reference: Id,
        accepted: &super::capability_service::AcceptedCapabilityResult,
        count: usize,
    ) -> Result<Record, EngineError> {
        if !accepted.native_images().is_empty() {
            let images = self.native_retained_images(call, accepted, count).await?;
            return self
                .record_image_outputs(call, reference, accepted, images)
                .await;
        }
        let decoded = orch_image_backends::decode(
            accepted.binding(),
            accepted.input(),
            accepted.raw_result(),
            self.image_limits(call, count).await?,
        )
        .map_err(EngineError::unreadable)?;
        self.record_image_outputs(call, reference, accepted, decoded.images)
            .await
    }

    async fn native_retained_images(
        &self,
        call: &CallContext,
        accepted: &super::capability_service::AcceptedCapabilityResult,
        count: usize,
    ) -> Result<Vec<Image>, EngineError> {
        let limits = self.image_limits(call, count).await?;
        if accepted.native_images().len() > limits.images {
            return Err(EngineError::unreadable(
                "The accepted native result needs more output paths; retry its delivery reference without generating again.",
            ));
        }
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("The image store is unavailable."))?;
        let mut images = Vec::new();
        let mut total = 0_usize;
        for (ordinal, image) in accepted.native_images().iter().enumerate() {
            let size = usize::try_from(image.size).map_err(EngineError::unreadable)?;
            total = total
                .checked_add(size)
                .ok_or(EngineError::ToolCallIdentity)?;
            if size == 0 || size > limits.image_bytes || total > limits.total_bytes {
                return Err(EngineError::unreadable(
                    "The retained native image exceeds the configured image storage budget.",
                ));
            }
            {
                let mut conn = self.db().acquire().await?;
                source::check_call(&mut conn, call).await?;
                source::original_callers(
                    &mut conn,
                    call,
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?;
            }
            let read = store
                .get(&image.blob, None)
                .await
                .map_err(EngineError::unreadable)?;
            if read.meta.hash != image.blob || read.meta.size != image.size {
                return Err(EngineError::ToolCallIdentity);
            }
            let mut verifier = Verifier::new(image.blob, Some(image.size));
            let mut bytes = Vec::new();
            let mut stream = read.body;
            while let Some(piece) = stream.next().await {
                let piece = piece.map_err(EngineError::unreadable)?;
                if bytes.len().saturating_add(piece.len()) > size {
                    return Err(EngineError::ToolCallIdentity);
                }
                verifier.update(&piece);
                bytes.extend_from_slice(&piece);
            }
            verifier.finish().map_err(EngineError::unreadable)?;
            images.push(Image {
                ordinal,
                bytes,
                media_type: Some(image.media_type.clone()),
                native_ref: Some(image.native_ref.clone()),
                revised_prompt: image.revised_prompt.clone(),
            });
        }
        Ok(images)
    }

    async fn record_image_outputs(
        &self,
        call: &CallContext,
        reference: Id,
        accepted: &super::capability_service::AcceptedCapabilityResult,
        images: Vec<Image>,
    ) -> Result<Record, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("The image store is unavailable."))?;
        let mut values = Vec::with_capacity(images.len());
        for image in images {
            let media_type = media(&image)?;
            let blob = Hash::of(&image.bytes);
            let size = i64::try_from(image.bytes.len()).map_err(EngineError::unreadable)?;
            store
                .put(
                    &blob,
                    Some(size),
                    stream_of(vec![bytes::Bytes::from(image.bytes)]),
                )
                .await
                .map_err(EngineError::unreadable)?;
            values.push((
                blob,
                size,
                media_type,
                image.native_ref,
                image.revised_prompt,
            ));
        }
        let mut raw_blobs = Vec::new();
        let raw_input = self.image_raw_text(&call.arguments, &mut raw_blobs).await?;
        let raw_output = self
            .image_raw_text(accepted.raw_result(), &mut raw_blobs)
            .await?;
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_tool_calls WHERE answer_id=$1 AND thread_id=$2 AND turn_id=$3 AND tool='image_generation' AND NOT retired)")
            .bind(reference).bind(call.thread).bind(call.turn).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if !owned {
            return Err(EngineError::ToolCallIdentity);
        }
        let turn = threads::turn(&mut tx, call.turn)
            .await?
            .filter(|turn| turn.thread_id == call.thread)
            .ok_or(EngineError::StaleToolSource)?;
        let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
        for (hash, size) in raw_blobs {
            storage::register_blob(unit.conn, &hash, size, self.inner.clock.now()).await?;
        }
        let mut outputs = Vec::with_capacity(values.len());
        for (index, (blob, size, media_type, native_ref, revised_prompt)) in
            values.into_iter().enumerate()
        {
            storage::register_blob(unit.conn, &blob, size, self.inner.clock.now()).await?;
            let native = format!("orch.image_generation:{reference}:{index}");
            let payload = json!({"title":"Generated image","started_at":self.inner.clock.time(),"duration_ms":null,
                "tool":{"kind":"image","blob":blob,"media_type":media_type,"name":format!("image-{}",index+1),"caption":revised_prompt},
                "raw":{"name":"image_generation","input":raw_input,"output":raw_output,"is_error":false}});
            let item = crate::ingest::add_item(
                &mut unit,
                &crate::ingest::ItemSpec {
                    thread: call.thread,
                    segment: turn.segment_id,
                    turn: Some(call.turn),
                    stream: "main",
                    parent: None,
                    kind: "tool_call",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: Some(&native),
                },
            )
            .await?;
            let item = match item {
                Some(item) => item.id,
                None => {
                    sqlx::query_scalar("SELECT id FROM items WHERE segment_id=$1 AND native_ref=$2")
                        .bind(turn.segment_id)
                        .bind(&native)
                        .fetch_one(&mut *unit.conn)
                        .await
                        .map_err(orch_db::DbError::from)?
                }
            };
            outputs.push(Output {
                blob,
                size,
                media_type,
                item,
                native_ref,
                revised_prompt,
            });
        }
        let record = Record {
            reference,
            thread: call.thread,
            turn: call.turn,
            receipt: accepted.receipt().clone(),
            images: outputs,
            released: false,
            helper: accepted.raw_result().get("helper").cloned(),
        };
        sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,ARRAY[$2::text],$3) WHERE answer_id=$1 AND NOT(plan ? $2)").bind(reference).bind(SLOT).bind(serde_json::to_value(&record)?).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        let mut conn = self.db().acquire().await?;
        load_record(&mut conn, call.thread, reference).await
    }

    async fn image_raw_text(
        &self,
        value: &Json,
        metadata: &mut Vec<(Hash, i64)>,
    ) -> Result<orch_api_contract::threads::OutputRef, EngineError> {
        let text = serde_json::to_string(value)?;
        let mut preview = orch_api_contract::threads::OutputRef::of(&text);
        if preview.truncated {
            let store = self
                .blobs()
                .ok_or_else(|| EngineError::unreadable("The image result store is unavailable."))?;
            let hash = Hash::of(text.as_bytes());
            let size = i64::try_from(text.len()).map_err(EngineError::unreadable)?;
            store
                .put(&hash, Some(size), stream_of(vec![bytes::Bytes::from(text)]))
                .await
                .map_err(EngineError::unreadable)?;
            metadata.push((hash, size));
            preview = preview.with_blob(hash);
        }
        Ok(preview)
    }

    async fn image_bytes(&self, output: &Output) -> Result<Vec<u8>, EngineError> {
        self.image_blob_bytes(output.blob, output.size).await
    }

    async fn image_blob_bytes(&self, blob: Hash, size: i64) -> Result<Vec<u8>, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("The image store is unavailable."))?;
        let read = store
            .get(&blob, None)
            .await
            .map_err(EngineError::unreadable)?;
        if read.meta.hash != blob || read.meta.size != size {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut bytes = Vec::new();
        let mut verifier = Verifier::new(blob, Some(size));
        let mut stream = read.body;
        while let Some(piece) = stream.next().await {
            let piece = piece.map_err(EngineError::unreadable)?;
            if i64::try_from(bytes.len().saturating_add(piece.len()))
                .map_err(EngineError::unreadable)?
                > size
            {
                return Err(EngineError::ToolCallIdentity);
            }
            verifier.update(&piece);
            bytes.extend_from_slice(&piece);
        }
        verifier.finish().map_err(EngineError::unreadable)?;
        Ok(bytes)
    }

    async fn deliver_generated_image(
        &self,
        call: &CallContext,
        record: &Record,
        index: usize,
        path: &str,
    ) -> Result<crate::sharing::FileWritten, EngineError> {
        result_read(self, call, record).await?;
        let output = record
            .images
            .get(index)
            .ok_or(EngineError::ToolCallIdentity)?;
        let bytes = self.image_bytes(output).await?;
        let proof =
            delivery::Proof::prepare(self, call, record.reference, index, path, output).await?;
        let writer = self
            .parcels()
            .ok_or_else(|| EngineError::unreadable("The workspace file writer is unavailable."))?;
        let mut conn = self.db().acquire().await?;
        delivery::scope(
            proof,
            writer.write_file(&mut conn, call.thread, path, &bytes),
        )
        .await
        .map_err(EngineError::unreadable)
    }
}
